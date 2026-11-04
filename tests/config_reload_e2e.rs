//! End-to-end integration tests for config hot-reload functionality.
//!
//! Spawns a real `convwatcher` subprocess and verifies that modifying the
//! config file triggers a hot-reload (new folders created) without restart,
//! and that writing broken YAML does NOT crash the process.

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
        (
            "audio_codecs.yaml",
            r#"presets: {}
"#,
        ),
        (
            "image_codecs.yaml",
            r#"presets:
  jpeg_80:
    output_ext: .jpg
    quality: 80
"#,
        ),
        (
            "pdf_presets.yaml",
            r#"presets: {}
"#,
        ),
        (
            "document_presets.yaml",
            r#"presets: {}
"#,
        ),
        (
            "custom_presets.yaml",
            r#"presets: {}
"#,
        ),
    ];
    for (name, content) in &presets {
        let path = config_dir.join(name);
        if !path.exists() {
            std::fs::write(&path, content).unwrap();
        }
    }
}

/// Build a minimal config.yaml with one watcher, fast intervals.
fn build_initial_config(ffmpeg_path: &str, watch1: &str, output1: &str, logs: &str) -> String {
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

watchers:
- name: initial_watcher
  watch_folder: {}
  output_folder: {}
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
"#,
        ffmpeg_path, logs, logs, logs, watch1, output1
    )
}

/// Build a config.yaml with TWO watchers (adds a second one to the initial).
fn build_reloaded_config(
    ffmpeg_path: &str,
    watch1: &str,
    output1: &str,
    watch2: &str,
    output2: &str,
    logs: &str,
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

watchers:
- name: initial_watcher
  watch_folder: {}
  output_folder: {}
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
- name: added_watcher
  watch_folder: {}
  output_folder: {}
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
"#,
        ffmpeg_path, logs, logs, logs, watch1, output1, watch2, output2
    )
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

fn write_sample_video(path: &std::path::Path) {
    let ffmpeg =
        std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());
    let status = std::process::Command::new(ffmpeg)
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=64x64:rate=1",
            "-y",
            path.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run ffmpeg for sample video");
    assert!(status.success(), "ffmpeg failed to create sample video");
}

fn has_output(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok()).any(|e| e.path().is_file()))
        .unwrap_or(false)
}

#[test]
fn config_hot_reload_creates_new_folders() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping config reload test");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping config reload test");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("cw-reload-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    let ffmpeg_path =
        std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());

    let config_dir = tmp.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    write_presets(&config_dir);

    let config_path = config_dir.join("config.yaml");

    // Write initial config with ONE watcher.
    let watch1 = tmp.join("watch_1").to_string_lossy().to_string();
    let output1 = tmp.join("output_1").to_string_lossy().to_string();
    let logs_dir = tmp.join("logs").to_string_lossy().to_string();

    std::fs::write(
        &config_path,
        build_initial_config(&ffmpeg_path, &watch1, &output1, &logs_dir),
    )
    .unwrap();

    let new_watch_folder = tmp.join("watch_2");
    let new_output_folder = tmp.join("output_2");

    let mut child = std::process::Command::new(&bin)
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--no-daemon",
            "--level",
            "warn",
        ])
        .current_dir(&tmp)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn convwatcher");

    // Let it start up and load the initial config.
    std::thread::sleep(Duration::from_secs(2));

    // Verify the process is alive before modifying config.
    assert!(
        child_alive(&mut child),
        "convwatcher should still be running before config change"
    );

    // Ensure new folders don't exist yet.
    assert!(!new_watch_folder.exists(), "watch_2 should not exist yet");
    assert!(!new_output_folder.exists(), "output_2 should not exist yet");

    // Write the reloaded config with an additional watcher.
    let watch2 = tmp.join("watch_2").to_string_lossy().to_string();
    let output2 = tmp.join("output_2").to_string_lossy().to_string();
    std::fs::write(
        &config_path,
        build_reloaded_config(
            &ffmpeg_path,
            &watch1,
            &output1,
            &watch2,
            &output2,
            &logs_dir,
        ),
    )
    .unwrap();

    // Poll for the new folders to be created by the hot-reload monitor_manager.
    // The reload interval is 1s, so this should take ~2-3 seconds.
    let reloaded = poll(
        || new_watch_folder.exists() && new_output_folder.exists(),
        Duration::from_secs(15),
        Duration::from_millis(300),
    );

    // Also verify the process is still alive after reload (didn't crash).
    let alive = child_alive(&mut child);

    assert!(
        reloaded,
        "Hot-reload should create new watcher folders. \
           watch_2 exists: {}, output_2 exists: {}",
        new_watch_folder.exists(),
        new_output_folder.exists()
    );
    assert!(
        alive,
        "Process should still be running after config hot-reload"
    );

    // Folder existence alone is not enough: load_config creates folders on its
    // own during reload polling. Prove the new monitor actually converts.
    let sample = tmp.join("sample_2.mp4");
    write_sample_video(&sample);
    std::fs::copy(&sample, new_watch_folder.join("sample_2.mp4")).unwrap();
    let converted = poll(
        || has_output(&new_output_folder),
        Duration::from_secs(20),
        Duration::from_millis(300),
    );

    let _ = child.kill();
    let _ = child.wait();
    assert!(
        converted,
        "new watcher must actually convert a file after hot-reload"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn invalid_config_does_not_crash_process() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping invalid config test");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping invalid config test");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("cw-reload-invalid-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    let ffmpeg_path =
        std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());

    let config_dir = tmp.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    write_presets(&config_dir);

    let config_path = config_dir.join("config.yaml");

    // Write a valid initial config.
    let watch1 = tmp.join("watch_1").to_string_lossy().to_string();
    let output1 = tmp.join("output_1").to_string_lossy().to_string();
    let logs_dir = tmp.join("logs").to_string_lossy().to_string();

    std::fs::write(
        &config_path,
        build_initial_config(&ffmpeg_path, &watch1, &output1, &logs_dir),
    )
    .unwrap();

    let mut child = std::process::Command::new(&bin)
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--no-daemon",
            "--level",
            "warn",
        ])
        .current_dir(&tmp)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn convwatcher");

    // Let it start up.
    std::thread::sleep(Duration::from_secs(2));

    // Verify alive before modification.
    assert!(
        child_alive(&mut child),
        "convwatcher should still be running before config break"
    );

    // Write BROKEN YAML to the config file.
    let broken_yaml = "this is [[[ not valid yaml content: {broken}\n";
    std::fs::write(&config_path, broken_yaml).unwrap();

    // Poll that the process stays alive despite the broken config.
    // The hot-reloader logs a warning and continues with old config.
    // Give it enough time for at least 2 reload cycles (refresh_interval=1s).
    let survived = poll(
        || child_alive(&mut child),
        Duration::from_secs(8),
        Duration::from_millis(500),
    );

    // Also verify the original watch folder still exists (old config still in effect).
    let old_watch_folder = tmp.join("watch_1");
    let old_output_folder = tmp.join("output_1");

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        survived,
        "Process should survive being served an invalid config file"
    );
    assert!(
        old_watch_folder.exists(),
        "Old watcher folders should persist after broken config is written"
    );
    assert!(
        old_output_folder.exists(),
        "Old output folders should persist after broken config is written"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[allow(clippy::too_many_arguments)]
fn build_three_watcher_config(
    ffmpeg_path: &str,
    watch1: &str,
    output1: &str,
    watch2: &str,
    output2: &str,
    watch3: &str,
    output3: &str,
    logs: &str,
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

watchers:
- name: initial_watcher
  watch_folder: {}
  output_folder: {}
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
- name: added_watcher
  watch_folder: {}
  output_folder: {}
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
- name: third_watcher
  watch_folder: {}
  output_folder: {}
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
"#,
        ffmpeg_path, logs, logs, logs, watch1, output1, watch2, output2, watch3, output3
    )
}

#[test]
fn config_hot_reload_twice_creates_third_folder() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping double reload test");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping double reload test");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("cw-reload-twice-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    let ffmpeg_path =
        std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());

    let config_dir = tmp.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    write_presets(&config_dir);
    let config_path = config_dir.join("config.yaml");

    let watch1 = tmp.join("watch_1").to_string_lossy().to_string();
    let output1 = tmp.join("output_1").to_string_lossy().to_string();
    let logs_dir = tmp.join("logs").to_string_lossy().to_string();
    std::fs::write(
        &config_path,
        build_initial_config(&ffmpeg_path, &watch1, &output1, &logs_dir),
    )
    .unwrap();

    let watch2 = tmp.join("watch_2").to_string_lossy().to_string();
    let output2 = tmp.join("output_2").to_string_lossy().to_string();
    let watch3 = tmp.join("watch_3").to_string_lossy().to_string();
    let output3 = tmp.join("output_3").to_string_lossy().to_string();

    let mut child = std::process::Command::new(&bin)
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--no-daemon",
            "--level",
            "warn",
        ])
        .current_dir(&tmp)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn convwatcher");

    std::thread::sleep(Duration::from_secs(2));

    // First reload: add watcher 2.
    std::fs::write(
        &config_path,
        build_reloaded_config(
            &ffmpeg_path,
            &watch1,
            &output1,
            &watch2,
            &output2,
            &logs_dir,
        ),
    )
    .unwrap();
    assert!(
        poll(
            || std::path::Path::new(&watch2).exists() && std::path::Path::new(&output2).exists(),
            Duration::from_secs(15),
            Duration::from_millis(300)
        ),
        "first hot-reload should create watcher 2 folders"
    );
    assert!(
        child_alive(&mut child),
        "process must survive the first reload"
    );

    // Second reload: add watcher 3. This hung before the shutdown_tx fix.
    std::fs::write(
        &config_path,
        build_three_watcher_config(
            &ffmpeg_path,
            &watch1,
            &output1,
            &watch2,
            &output2,
            &watch3,
            &output3,
            &logs_dir,
        ),
    )
    .unwrap();
    let second_reload = poll(
        || std::path::Path::new(&watch3).exists() && std::path::Path::new(&output3).exists(),
        Duration::from_secs(15),
        Duration::from_millis(300),
    );
    let alive = child_alive(&mut child);
    assert!(
        second_reload,
        "second hot-reload should create watcher 3 folders"
    );
    assert!(alive, "process must survive the second reload");

    let sample3 = tmp.join("sample_3.mp4");
    write_sample_video(&sample3);
    std::fs::copy(&sample3, std::path::Path::new(&watch3).join("sample_3.mp4")).unwrap();
    let converted = poll(
        || has_output(std::path::Path::new(&output3)),
        Duration::from_secs(20),
        Duration::from_millis(300),
    );

    let _ = child.kill();
    let _ = child.wait();
    assert!(
        converted,
        "watcher 3 must actually convert a file after the second reload"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[cfg(unix)]
#[test]
fn sigterm_shuts_down_process_within_timeout() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping SIGTERM test");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping SIGTERM test");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("cw-sigterm-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    let ffmpeg_path =
        std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());

    let config_dir = tmp.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    write_presets(&config_dir);
    let config_path = config_dir.join("config.yaml");

    let watch1 = tmp.join("watch_1").to_string_lossy().to_string();
    let output1 = tmp.join("output_1").to_string_lossy().to_string();
    let logs_dir = tmp.join("logs").to_string_lossy().to_string();
    std::fs::write(
        &config_path,
        build_initial_config(&ffmpeg_path, &watch1, &output1, &logs_dir),
    )
    .unwrap();

    let mut child = std::process::Command::new(&bin)
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--no-daemon",
            "--level",
            "warn",
        ])
        .current_dir(&tmp)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn convwatcher");

    std::thread::sleep(Duration::from_secs(2));
    assert!(
        child_alive(&mut child),
        "process should be running before SIGTERM"
    );

    let status = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("failed to send SIGTERM");
    assert!(status.success(), "kill -TERM should succeed");

    let exited = poll(
        || !child_alive(&mut child),
        Duration::from_secs(15),
        Duration::from_millis(200),
    );

    if !exited {
        let _ = child.kill();
    }
    let _ = child.wait();

    assert!(
        exited,
        "process must exit after SIGTERM (shutdown deadlock regression)"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[cfg(unix)]
#[test]
fn reload_and_sigterm_concurrently_shuts_down() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping reload+shutdown test");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping reload+shutdown test");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("cw-reload-term-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let config_dir = tmp.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    write_presets(&config_dir);
    let config_path = config_dir.join("config.yaml");

    let ffmpeg_path =
        std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());
    let watch1 = tmp.join("watch_1").to_string_lossy().to_string();
    let output1 = tmp.join("output_1").to_string_lossy().to_string();
    let logs_dir = tmp.join("logs").to_string_lossy().to_string();
    std::fs::write(
        &config_path,
        build_initial_config(&ffmpeg_path, &watch1, &output1, &logs_dir),
    )
    .unwrap();

    let watch2 = tmp.join("watch_2").to_string_lossy().to_string();
    let output2 = tmp.join("output_2").to_string_lossy().to_string();

    let mut child = std::process::Command::new(&bin)
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--no-daemon",
            "--level",
            "warn",
        ])
        .current_dir(&tmp)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn convwatcher");

    std::thread::sleep(Duration::from_secs(2));

    // Change config and immediately request shutdown; this exercised the
    // reload/shutdown race that previously hung in monitor_manager.
    std::fs::write(
        &config_path,
        build_reloaded_config(
            &ffmpeg_path,
            &watch1,
            &output1,
            &watch2,
            &output2,
            &logs_dir,
        ),
    )
    .unwrap();
    assert!(
        poll(
            || std::path::Path::new(&watch2).exists(),
            Duration::from_secs(10),
            Duration::from_millis(100)
        ),
        "reload should start before SIGTERM"
    );

    let status = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("failed to send SIGTERM");
    assert!(status.success(), "kill -TERM should succeed");

    let exited = poll(
        || !child_alive(&mut child),
        Duration::from_secs(15),
        Duration::from_millis(200),
    );
    if !exited {
        let _ = child.kill();
    }
    let _ = child.wait();
    assert!(exited, "process must exit when reload and SIGTERM race");
    let _ = std::fs::remove_dir_all(&tmp);
}
