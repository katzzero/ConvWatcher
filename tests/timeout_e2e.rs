//! End-to-end test proving `global.timeout_s` is applied by the runner.

use std::time::{Duration, Instant};

fn convwatcher_bin() -> Option<std::path::PathBuf> {
    let mut p = std::env::current_exe().unwrap();
    p.pop();
    p.pop();
    p.push("convwatcher");
    p.exists().then_some(p)
}

fn ffmpeg_path() -> Option<String> {
    let f = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());
    let ok = std::path::Path::new(&f).exists();
    if !ok && std::env::var("CI").is_ok() {
        panic!("ffmpeg is required in CI");
    }
    ok.then_some(f)
}

fn write_presets(config_dir: &std::path::Path) {
    let presets = [
        ("video_codecs.yaml", "presets: {}\n"),
        ("audio_codecs.yaml", "presets: {}\n"),
        ("image_codecs.yaml", "presets: {}\n"),
        ("pdf_presets.yaml", "presets: {}\n"),
        ("document_presets.yaml", "presets: {}\n"),
        (
            "custom_presets.yaml",
            r#"presets:
  slow_ffmpeg:
    command: "ffmpeg -re -f lavfi -i testsrc=duration=10:size=64x64:rate=1 -y {output}"
    output_ext: .mp4
    description: "10s realtime encode; must be killed by a 1s timeout"
"#,
        ),
    ];
    for (name, content) in presets {
        let path = config_dir.join(name);
        if !path.exists() {
            std::fs::write(path, content).unwrap();
        }
    }
}

fn poll<F: FnMut() -> bool>(mut f: F, timeout: Duration, tick: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return true;
        }
        std::thread::sleep(tick);
    }
    false
}

fn config_yaml(ffmpeg: &str, watch: &str, output: &str, logs: &str) -> String {
    format!(
        r#"global:
  file_check_interval: 200ms
  stable_time: 100ms
  ffmpeg_path: {ffmpeg}
  max_concurrent: 1
  refresh_interval: 1s
  embedded_secret: ""
  embedded_scan_interval: 0
  timeout: 1

  log:
    errors_file: {logs}/errors.log
    app_log_file: {logs}/app.log

  healthcheck:
    http_port: 0

  disk_space:
    check_interval: 60s
    threshold: 500

  history:
    persistent: false
    file: {logs}/history.json
    max_records: 50

watchers:
- name: timeout_watcher
  watch_folder: {watch}
  output_folder: {output}
  type: custom
  rules:
    - input_extensions: [.txt]
      preset: slow_ffmpeg
"#
    )
}

#[test]
fn timeout_s_is_applied_to_custom_conversion() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping timeout test");
            return;
        }
    };
    let ffmpeg = match ffmpeg_path() {
        Some(p) => p,
        None => {
            eprintln!("ffmpeg unavailable — skipping timeout test");
            return;
        }
    };

    let tmp = std::env::temp_dir().join(format!("cw-timeout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let config_dir = tmp.join("config");
    let watch = tmp.join("watch");
    let output = tmp.join("output");
    let logs = tmp.join("logs");
    for d in [&config_dir, &watch, &output, &logs] {
        std::fs::create_dir_all(d).unwrap();
    }
    write_presets(&config_dir);

    let config_path = config_dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(
            &ffmpeg,
            watch.to_str().unwrap(),
            output.to_str().unwrap(),
            logs.to_str().unwrap(),
        ),
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

    let started = Instant::now();
    let input = watch.join("input.txt");
    std::fs::write(&input, b"dummy").unwrap();

    let error_marker = watch.join("input.txt.error");
    let timed_out = poll(
        || error_marker.exists(),
        Duration::from_secs(15),
        Duration::from_millis(200),
    );
    let elapsed = started.elapsed();

    let _ = child.kill();
    let _ = child.wait();

    assert!(timed_out, "input should be marked .error after timeout");
    // Reject the false positive where the command fails immediately (e.g.
    // allowlist rejection / bad ffmpeg args): a 1s timeout must take ~1s.
    assert!(
        elapsed >= Duration::from_millis(900),
        "timeout fired too early ({elapsed:?}); command likely failed before the timer"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
