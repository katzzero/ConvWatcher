//! End-to-end integration tests: subfolder scan and `input_file_action: none`.
//!
//! Spawns a real `convwatcher` subprocess and verifies:
//! 1. Subfolder: a `video` watcher with `subfolders: [{name: gpu}]` and a rule
//!    `subfolder: gpu` converts a file dropped in `watch/->gpu/` (the `->*`
//!    scan path works).
//! 2. `action=none`: with `input_file_action: none`, the input file remains
//!    after conversion and is NOT reprocessed across several scan cycles
//!    (only one output file, no `_1`).
//!
//! Skips gracefully when the convwatcher binary or ffmpeg is missing.
//! Uses temp dirs, polling loops, graceful skips and cleanup with remove_dir_all.

use std::time::Duration;

fn convwatcher_bin() -> Option<std::path::PathBuf> {
    // From target/debug/deps/test_binary → pop×2 → target/debug/convwatcher
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // deps/
    p.pop(); // debug/
    p.push("convwatcher");
    if p.exists() {
        return Some(p);
    }
    if std::env::var("CI").is_ok() {
        panic!("convwatcher binary is required in CI (build it before running tests)");
    }
    None
}

fn ffmpeg_available() -> bool {
    let f = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());
    let ok = std::path::Path::new(&f).exists();
    if !ok && std::env::var("CI").is_ok() {
        panic!("ffmpeg is required in CI");
    }
    ok
}

fn ffmpeg_path() -> String {
    std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string())
}

/// Generate the minimal preset files required by `load_config`.
fn write_presets(config_dir: &std::path::Path) {
    let presets = [
        (
            "video_codecs.yaml",
            r#"presets:
  libx264:
    codec: libx264
    quality: crf 23
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
"#,
        ),
        ("audio_codecs.yaml", "presets: {}\n"),
        ("image_codecs.yaml", "presets: {}\n"),
        ("pdf_presets.yaml", "presets: {}\n"),
        ("document_presets.yaml", "presets: {}\n"),
        ("custom_presets.yaml", "presets: {}\n"),
    ];
    for (name, content) in &presets {
        let path = config_dir.join(name);
        if !path.exists() {
            let _ = std::fs::write(&path, content);
        }
    }
}

fn poll<F: FnMut() -> bool>(mut f: F, timeout: Duration, tick: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return true;
        }
        std::thread::sleep(tick);
    }
    false
}

fn child_alive(child: &mut std::process::Child) -> bool {
    matches!(child.try_wait(), Ok(None))
}

fn spawn_convwatcher(
    bin: &std::path::Path,
    config_path: &std::path::Path,
    tmp: &std::path::Path,
) -> std::process::Child {
    std::process::Command::new(bin)
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--no-daemon",
            "--level",
            "warn",
        ])
        .current_dir(tmp)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn convwatcher")
}

/// Generate a 1s test video with ffmpeg `testsrc`. Returns true on success.
fn generate_test_video(ffmpeg: &str, out: &std::path::Path) -> bool {
    let status = std::process::Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=320x240:rate=25",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-y",
            out.to_str().unwrap(),
        ])
        .status();
    match status {
        Ok(s) => s.success(),
        Err(_) => false,
    }
}

/// Minimal config with fast scan intervals for a video watcher.
fn build_config(
    ffmpeg: &str,
    _watch: &str,
    _output: &str,
    logs: &str,
    input_file_action: &str,
    watcher_yaml: &str,
) -> String {
    format!(
        r#"global:
  file_check_interval: 200ms
  stable_time: 500ms
  ffmpeg_path: {}
  max_concurrent: 1
  refresh_interval: 1s
  embedded_secret: ""
  embedded_scan_interval: 0

  log:
    errors_file: {}/errors.log
    app_log_file: {}/app.log

  healthcheck:
    http_port: 0

  disk_space:
    check_interval: 60s
    threshold: 500

  history:
    persistent: false
    file: {}/history.json
    max_records: 50

  input_file_action: {}

watchers:
{}
"#,
        ffmpeg, logs, logs, logs, input_file_action, watcher_yaml
    )
}

fn count_output_files(output_folder: &std::path::Path) -> usize {
    std::fs::read_dir(output_folder)
        .ok()
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .count()
        })
        .unwrap_or(0)
}

#[test]
fn subfolder_scan_converts_file_in_gpu() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping subfolder scan test");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping subfolder scan test");
        return;
    }

    let ffmpeg = ffmpeg_path();
    let tmp = std::env::temp_dir().join(format!("cw-subfolder-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    let config_dir = tmp.join("config");
    let watch_folder = tmp.join("watch_sub");
    let output_folder = tmp.join("output_sub");
    let logs_dir = tmp.join("logs");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&watch_folder).unwrap();
    std::fs::create_dir_all(&output_folder).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();

    write_presets(&config_dir);

    let watch = watch_folder.to_str().unwrap();
    let output = output_folder.to_str().unwrap();
    let logs_p = logs_dir.to_str().unwrap();

    // `->gpu/` is created automatically by create_folders from `subfolders`.
    let watcher_yaml = format!(
        r#"- name: sub_watcher
  watch_folder: {}
  output_folder: {}
  type: video
  subfolders:
    - name: gpu
  rules:
    - input_extensions: [.mp4]
      preset: libx264
      subfolder: gpu
"#,
        watch, output
    );

    let config_path = config_dir.join("config.yaml");
    std::fs::write(
        &config_path,
        build_config(&ffmpeg, watch, output, logs_p, "mark", &watcher_yaml),
    )
    .unwrap();

    let mut child = spawn_convwatcher(&bin, &config_path, &tmp);

    // Let it start up and create the ->gpu/ subfolder.
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        child_alive(&mut child),
        "convwatcher should still be running after startup"
    );
    assert!(
        watch_folder.join("->gpu").exists(),
        "->gpu/ subfolder should be created automatically"
    );

    // Generate a real test video and drop it into ->gpu/.
    let src_video = tmp.join("source.mp4");
    assert!(
        generate_test_video(&ffmpeg, &src_video),
        "ffmpeg testsrc should generate a test video"
    );
    let gpu_file = watch_folder.join("->gpu").join("clip.mp4");
    std::fs::copy(&src_video, &gpu_file).expect("failed to drop file into ->gpu/");

    // Poll for the converted output to appear.
    let converted = poll(
        || count_output_files(&output_folder) >= 1,
        Duration::from_secs(20),
        Duration::from_millis(300),
    );

    let alive = child_alive(&mut child);
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        converted,
        "File dropped in ->gpu/ should be converted. \
        Output folder contents: {:?}",
        std::fs::read_dir(&output_folder)
            .ok()
            .map(|d| d.filter_map(|e| e.ok()).collect::<Vec<_>>())
    );
    assert!(
        alive,
        "Process should still be running after subfolder conversion"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn action_none_does_not_reprocess_input() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping action=none test");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping action=none test");
        return;
    }

    let ffmpeg = ffmpeg_path();
    let tmp = std::env::temp_dir().join(format!("cw-action-none-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    let config_dir = tmp.join("config");
    let watch_folder = tmp.join("watch_none");
    let output_folder = tmp.join("output_none");
    let logs_dir = tmp.join("logs");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&watch_folder).unwrap();
    std::fs::create_dir_all(&output_folder).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();

    write_presets(&config_dir);

    let watch = watch_folder.to_str().unwrap();
    let output = output_folder.to_str().unwrap();
    let logs_p = logs_dir.to_str().unwrap();

    let watcher_yaml = format!(
        r#"- name: none_watcher
  watch_folder: {}
  output_folder: {}
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
"#,
        watch, output
    );

    let config_path = config_dir.join("config.yaml");
    std::fs::write(
        &config_path,
        build_config(&ffmpeg, watch, output, logs_p, "none", &watcher_yaml),
    )
    .unwrap();

    let mut child = spawn_convwatcher(&bin, &config_path, &tmp);

    // Let it start up.
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        child_alive(&mut child),
        "convwatcher should still be running after startup"
    );

    // Generate a real test video and drop it in the watch folder root.
    let src_video = tmp.join("source_none.mp4");
    assert!(
        generate_test_video(&ffmpeg, &src_video),
        "ffmpeg testsrc should generate a test video"
    );
    let input_file = watch_folder.join("clip_none.mp4");
    std::fs::copy(&src_video, &input_file).expect("failed to drop input file");

    // Wait for the first conversion to complete (one output file appears).
    let first_converted = poll(
        || count_output_files(&output_folder) >= 1,
        Duration::from_secs(20),
        Duration::from_millis(300),
    );
    assert!(
        first_converted,
        "First conversion should produce an output file. \
        Output folder contents: {:?}",
        std::fs::read_dir(&output_folder)
            .ok()
            .map(|d| d.filter_map(|e| e.ok()).collect::<Vec<_>>())
    );

    // Remember input mtime/size and output count right after first conversion.
    let input_metadata = std::fs::metadata(&input_file).expect("input file should exist");
    let input_size = input_metadata.len();
    let input_mtime = input_metadata.modified().ok();
    let output_count_after_first = count_output_files(&output_folder);

    // Wait several scan cycles (file_check_interval=200ms, stable_time=500ms).
    // With action=none the input stays on disk and must NOT be re-enqueued.
    std::thread::sleep(Duration::from_secs(6));

    let output_count_after_wait = count_output_files(&output_folder);
    let input_still_exists = input_file.exists();
    let input_unchanged = std::fs::metadata(&input_file)
        .ok()
        .map(|m| m.len() == input_size && m.modified().ok() == input_mtime)
        .unwrap_or(false);

    let alive = child_alive(&mut child);
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        input_still_exists,
        "With input_file_action=none the input file must remain on disk"
    );
    assert!(
        input_unchanged,
        "Input file must not be modified (size/mtime stable)"
    );
    assert_eq!(
        output_count_after_wait,
        output_count_after_first,
        "With action=none there must be no reprocessing: expected {} output file(s), found {} \
         (no `_1` duplicate). Output folder contents: {:?}",
        output_count_after_first,
        output_count_after_wait,
        std::fs::read_dir(&output_folder)
            .ok()
            .map(|d| d.filter_map(|e| e.ok()).collect::<Vec<_>>())
    );
    assert!(
        alive,
        "Process should still be running after action=none test"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
