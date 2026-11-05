//! Startup must fail fast with a clear message when ffmpeg is missing.

use std::time::{Duration, Instant};

fn convwatcher_bin() -> Option<std::path::PathBuf> {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    p.pop();
    p.push("convwatcher");
    if p.exists() {
        return Some(p);
    }
    if std::env::var("CI").is_ok() {
        panic!("convwatcher binary is required in CI (build it before running tests)");
    }
    None
}

fn write_presets(config_dir: &std::path::Path) {
    let presets = [
        (
            "video_codecs.yaml",
            "presets:\n  libx264:\n    codec: libx264\n    quality: crf 23\n    audio_codec: aac\n    audio_bitrate: 128k\n    output_ext: .mp4\n",
        ),
        ("audio_codecs.yaml", "presets: {}\n"),
        ("image_codecs.yaml", "presets: {}\n"),
        ("pdf_presets.yaml", "presets: {}\n"),
        ("document_presets.yaml", "presets: {}\n"),
        ("custom_presets.yaml", "presets: {}\n"),
    ];
    for (name, content) in presets {
        std::fs::write(config_dir.join(name), content).unwrap();
    }
}

#[test]
fn missing_ffmpeg_fails_at_startup_with_clear_message() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping startup validation test");
            return;
        }
    };

    let tmp = std::env::temp_dir().join(format!("cw-startup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let config_dir = tmp.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    write_presets(&config_dir);

    let config = format!(
        r#"global:
  file_check_interval: 1s
  stable_time: 1s
  ffmpeg_path: /definitely/not/ffmpeg
  max_concurrent: 1
  refresh_interval: 1s
  embedded_secret: ""
  embedded_scan_interval: 0

  log:
    errors_file: {tmp}/logs/errors.log
    app_log_file: {tmp}/logs/app.log

  healthcheck:
    http_port: 0

  history:
    persistent: false
    file: {tmp}/logs/history.json
    max_records: 50

watchers:
- name: startup
  watch_folder: {tmp}/in
  output_folder: {tmp}/out
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
"#,
        tmp = tmp.to_string_lossy()
    );
    let config_path = config_dir.join("config.yaml");
    std::fs::write(&config_path, config).unwrap();

    let mut child = std::process::Command::new(&bin)
        .args([
            "--config",
            config_path.to_str().unwrap(),
            "--no-daemon",
            "--level",
            "error",
        ])
        .current_dir(&tmp)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn convwatcher");

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("convwatcher did not exit on missing ffmpeg");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !status.success(),
        "missing ffmpeg must be a fatal startup error"
    );
    assert!(
        stderr.contains("ffmpeg not found"),
        "stderr should explain the missing ffmpeg, got: {}",
        stderr
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
