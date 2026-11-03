//! End-to-end integration tests for embedded override promotion flow.
//!
//! Spawns a real `convwatcher` subprocess, drops YAML overrides into watch
//! folders, and asserts the promotion pipeline (valid → config/watchs/ in
//! the config directory, invalid type/secret → rejected with .invalid marker).
//! Uses temp dirs under `std::env::temp_dir().join(format!("cw-…-{pid}"))`,
//! polling loops, graceful skips and cleanup with remove_dir_all.

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
    None
}

fn ffmpeg_available() -> bool {
    let f = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());
    std::path::Path::new(&f).exists()
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

/// Build a minimal `config.yaml` with one video watcher, empty embedded_secret,
/// fast intervals. The YAML is deliberately formatted to match serde_yaml
/// expectations (list items at column 0 with sub-fields indented +2).
fn build_config_no_secret(watch: &str, output: &str, logs: &str) -> String {
    let ffmpeg = ffmpeg_path();
    format!(
        r#"global:
  file_check_interval: 200ms
  stable_time: 500ms
  ffmpeg_path: {ffmpeg}
  max_concurrent: 1
  refresh_interval: 1s
  embedded_secret: ""
  embedded_scan_interval: 1

  log:
    errors_file: {logs_p}/errors.log
    app_log_file: {logs_p}/app.log

  healthcheck:
    http_port: 0

  disk_space:
    check_interval: 60s
    threshold: 500

  history:
    persistent: false
    file: {logs_p}/history.json
    max_records: 50

watchers:
- name: test_watcher
  watch_folder: {watch}
  output_folder: {output}
  type: video
  rules:
    - input_extensions: [.mp4, .mkv]
      preset: libx264
"#,
        logs_p = logs,
    )
}

/// Build config with a non-empty embedded_secret.
fn build_config_with_secret(
    secret: &str,
    watcher_name: &str,
    watch: &str,
    output: &str,
    logs: &str,
) -> String {
    let ffmpeg = ffmpeg_path();
    let logs_p = logs;
    format!(
        r#"global:
  file_check_interval: 200ms
  stable_time: 500ms
  ffmpeg_path: {ffmpeg}
  max_concurrent: 1
  refresh_interval: 1s
  embedded_secret: "{secret}"
  embedded_scan_interval: 1

  log:
    errors_file: {logs_p}/errors.log
    app_log_file: {logs_p}/app.log

  healthcheck:
    http_port: 0

  disk_space:
    check_interval: 60s
    threshold: 500

  history:
    persistent: false
    file: {logs_p}/history.json
    max_records: 50

watchers:
- name: {name}
  watch_folder: {watch}
  output_folder: {output}
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
"#,
        name = watcher_name,
    )
}

/// A valid embedded override YAML matching a video watcher.
fn valid_video_override(secret: &str) -> String {
    format!(
        r#"secret: "{secret}"
video:
  rules:
    - input_extensions: [.mov]
      preset: libx264
"#,
    )
}

/// An invalid embedded override: type mismatch (image instead of video).
fn invalid_type_override() -> &'static str {
    r#"secret: ""
video_typo:
  rules:
    - input_extensions: [.png]
      preset: jpeg_80
"#
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

/// Test: A valid override with matching secret is promoted from the watch folder
/// into config/watchs/. The embedded scanner then merges it onto main configs
/// and sends updated watchers via mpsc.
#[test]
fn embedded_override_valid_promoted_to_watchs() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping embedded override promotion test");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping embedded override promotion test");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("cw-override-promote-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    let config_dir = tmp.join("config");
    let watch_folder = tmp.join("watch_promote");
    let output_folder = tmp.join("output_promote");
    let logs_dir = tmp.join("logs");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&watch_folder).unwrap();
    std::fs::create_dir_all(&output_folder).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();

    let watch = watch_folder.to_str().unwrap();
    let output = output_folder.to_str().unwrap();
    let logs_p = logs_dir.to_str().unwrap();

    write_presets(&config_dir);

    let config_yaml = build_config_no_secret(watch, output, logs_p);
    let config_path = config_dir.join("config.yaml");
    std::fs::write(&config_path, &config_yaml).unwrap();

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

    // Give the process time to load config and start monitoring.
    std::thread::sleep(Duration::from_secs(2));

    // Drop a valid override into the watch folder.
    let override_path = watch_folder.join("test_watcher.yaml");
    std::fs::write(&override_path, valid_video_override(""))
        .expect("failed to write override file");

    // Poll for promotion: file should appear in config/watchs/.
    let promoted_file = config_dir.join("watchs").join("test_watcher.yaml");
    let promoted = poll(
        || promoted_file.exists(),
        Duration::from_secs(10),
        Duration::from_millis(300),
    );

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        promoted,
        "Valid override was not promoted to config/watchs/. \
        Expected at: {:?}. \
      Watch folder contents: {:?}",
        &promoted_file,
        std::fs::read_dir(&watch_folder)
            .ok()
            .map(|d| d.filter_map(|e| e.ok()).collect::<Vec<_>>())
    );

    // Also verify the original was renamed to .yaml.old
    assert!(
        override_path.with_extension("yaml.old").exists(),
        "Original override file should be renamed to .yaml.old after promotion"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Test: An override with a wrong secret is rejected. The config/file remains
/// in the watch folder and an .invalid marker is created beside it. It should
/// NOT appear in config/watchs/.
#[test]
fn embedded_override_wrong_secret_rejected() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping wrong-secret override test");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping wrong-secret override test");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("cw-override-secret-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    let config_dir = tmp.join("config");
    let watch_folder = tmp.join("watch_secret");
    let output_folder = tmp.join("output_secret");
    let logs_dir = tmp.join("logs");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&watch_folder).unwrap();
    std::fs::create_dir_all(&output_folder).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();

    write_presets(&config_dir);

    let watch = watch_folder.to_str().unwrap();
    let output = output_folder.to_str().unwrap();
    let logs_p = logs_dir.to_str().unwrap();

    let watcher_name = "sec_watcher";
    let config_yaml = build_config_with_secret("mysecret42", watcher_name, watch, output, logs_p);
    let config_path = config_dir.join("config.yaml");
    std::fs::write(&config_path, &config_yaml).unwrap();

    // Create the watcher's watch/output folders referenced in config.
    let _ = std::fs::create_dir_all(&watch_folder);
    let _ = std::fs::create_dir_all(&output_folder);

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

    // Give the process time to load config and start monitoring.
    std::thread::sleep(Duration::from_secs(2));

    // Drop an override with a WRONG secret (secret doesn't match config).
    let override_path = watch_folder.join(format!("{watcher_name}.yaml"));
    std::fs::write(&override_path, valid_video_override("wrongsecret"))
        .expect("failed to write override file");

    // Poll for rejection: nothing in config/watchs/, and check for .invalid marker.
    let promoted_file = config_dir
        .join("watchs")
        .join(format!("{watcher_name}.yaml"));
    let invalid_marker = watch_folder.join(format!("{watcher_name}.invalid"));

    let not_promoted = poll(
        || !promoted_file.exists(),
        Duration::from_secs(5),
        Duration::from_millis(200),
    );

    let has_marker = poll(
        || invalid_marker.exists(),
        Duration::from_secs(10),
        Duration::from_millis(300),
    );

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        not_promoted,
        "Override with wrong secret should NOT be promoted to config/watchs/"
    );
    assert!(
        has_marker,
        "An .invalid marker should exist when an override is rejected. \
           Checked: {:?}",
        &invalid_marker
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Test: An override with a type mismatch (image override for a video watcher)
/// is rejected, even when secret matches. The .invalid marker should be created
/// and no file should appear in config/watchs/.
#[test]
fn embedded_override_type_mismatch_rejected() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping type-mismatch override test");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping type-mismatch override test");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("cw-override-typem-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    let config_dir = tmp.join("config");
    let watch_folder = tmp.join("watch_typem");
    let output_folder = tmp.join("output_typem");
    let logs_dir = tmp.join("logs");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&watch_folder).unwrap();
    std::fs::create_dir_all(&output_folder).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();

    write_presets(&config_dir);

    let watch = watch_folder.to_str().unwrap();
    let output = output_folder.to_str().unwrap();
    let logs_p = logs_dir.to_str().unwrap();

    let watcher_name = "type_watcher";
    let config_yaml = build_config_with_secret("", watcher_name, watch, output, logs_p);
    let config_path = config_dir.join("config.yaml");
    std::fs::write(&config_path, &config_yaml).unwrap();

    // Create the watcher's watch/output folders referenced in config.
    let _ = std::fs::create_dir_all(&watch_folder);
    let _ = std::fs::create_dir_all(&output_folder);

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

    // Give the process time to start.
    std::thread::sleep(Duration::from_secs(2));

    // Drop an override with type mismatch (wrong key in YAML = no video rule).
    let override_path = watch_folder.join(format!("{watcher_name}.yaml"));
    std::fs::write(&override_path, invalid_type_override()).expect("failed to write override file");

    // Poll: nothing appears in config/watchs/ and .invalid marker exists.
    let promoted_file = config_dir
        .join("watchs")
        .join(format!("{watcher_name}.yaml"));
    let invalid_marker = watch_folder.join(format!("{watcher_name}.invalid"));

    let not_promoted = poll(
        || !promoted_file.exists(),
        Duration::from_secs(8),
        Duration::from_millis(200),
    );

    let has_marker = poll(
        || invalid_marker.exists(),
        Duration::from_secs(8),
        Duration::from_millis(300),
    );

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        not_promoted,
        "Type-mismatched override should NOT be promoted to config/watchs/"
    );
    assert!(
        has_marker,
        "An .invalid marker should exist when a type-mismatch override is rejected. \
            Checked: {:?}",
        &invalid_marker
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
