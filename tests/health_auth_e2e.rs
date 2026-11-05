//! End-to-end test for dashboard auth.
//!
//! Spawns a real `convwatcher` subprocess with `healthcheck.auth_password` set and
//! verifies:
//!   - `/health` returns 200 without a password (container healthcheck stays open)
//!   - `/api/watchers` returns 401 without a password
//!   - `/api/watchers` returns 200 with `Authorization: Bearer testpassword`
//!   - `/health` body contains `version`
//!   - `/logs/errors` returns 200 (empty) with password
//!
//! Uses `curl`. Skips gracefully if `curl` is unavailable locally; in CI the
//! convwatcher binary is required (panic if missing).

use std::time::Duration;

fn convwatcher_bin() -> Option<std::path::PathBuf> {
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

fn curl_available() -> bool {
    std::process::Command::new("curl")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

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

/// High port derived from PID to avoid collisions between parallel test runs.
fn high_port() -> u16 {
    let pid = std::process::id();
    (49152u32 + (pid % 1000)).try_into().unwrap_or(50000)
}

/// Run `curl -s -o /dev/null -w "%{http_code}" <url> [extra args...]` and return
/// the HTTP status code as a string (e.g. "200", "401").
fn curl_code(url: &str, extra: &[&str]) -> Option<String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-s")
        .arg("-o")
        .arg("/dev/null")
        .arg("-w")
        .arg("%{http_code}");
    cmd.arg(url);
    for a in extra {
        cmd.arg(a);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Run `curl -s <url> [extra args...]` and return the response body.
fn curl_body(url: &str, extra: &[&str]) -> Option<String> {
    let mut cmd = std::process::Command::new("curl");
    cmd.arg("-s").arg(url);
    for a in extra {
        cmd.arg(a);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
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

#[test]
fn health_auth_requires_bearer_token() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping health auth test");
            return;
        }
    };
    if !curl_available() {
        eprintln!("curl not available — skipping health auth test");
        return;
    }

    let port = high_port();
    let base = format!("http://127.0.0.1:{}", port);

    let tmp = std::env::temp_dir().join(format!("cw-health-auth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::create_dir_all(&tmp);

    let config_dir = tmp.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    write_presets(&config_dir);

    let config_path = config_dir.join("config.yaml");
    let watch1 = tmp.join("watch_1").to_string_lossy().to_string();
    let output1 = tmp.join("output_1").to_string_lossy().to_string();
    let logs_dir = tmp.join("logs").to_string_lossy().to_string();
    std::fs::create_dir_all(&watch1).unwrap();
    std::fs::create_dir_all(&output1).unwrap();
    std::fs::create_dir_all(&logs_dir).unwrap();

    let ffmpeg_path =
        std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());

    std::fs::write(
        &config_path,
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
    http_port: {}
    bind_address: 127.0.0.1
    auth_password: "testpassword"

  disk_space:
    check_interval: 60s
    threshold: 500

  history:
    persistent: false
    file: {}/history.json
    max_records: 50

watchers:
- name: auth_watcher
  watch_folder: {}
  output_folder: {}
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx264
"#,
            ffmpeg_path, logs_dir, logs_dir, port, logs_dir, watch1, output1
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

    // Wait for the health server to bind and answer.
    let up = poll(
        || curl_code(&format!("{}/health", base), &[]).is_some_and(|c| c == "200"),
        Duration::from_secs(20),
        Duration::from_millis(300),
    );
    assert!(up, "health server did not start on {} within timeout", base);

    // /health and the static dashboard are open without a password.
    let code = curl_code(&format!("{}/health", base), &[]).unwrap_or_default();
    assert_eq!(code, "200", "/health should return 200 without password");

    let dashboard_code = curl_code(&format!("{}/dashboard", base), &[]).unwrap_or_default();
    assert_eq!(
        dashboard_code, "200",
        "/dashboard must load without a password so the user can enter one"
    );

    let body = curl_body(&format!("{}/health", base), &[]).unwrap_or_default();
    assert!(
        body.contains("version"),
        "/health body should contain version, got: {}",
        body
    );

    // /api/watchers requires auth: 401 without password.
    let code_no_token = curl_code(&format!("{}/api/watchers", base), &[]).unwrap_or_default();
    assert_eq!(
        code_no_token, "401",
        "/api/watchers should return 401 without password, got {}",
        code_no_token
    );

    // /api/watchers returns 200 with Bearer password.
    let code_with_token = curl_code(
        &format!("{}/api/watchers", base),
        &["-H", "Authorization: Bearer testpassword"],
    )
    .unwrap_or_default();
    assert_eq!(
        code_with_token, "200",
        "/api/watchers should return 200 with Bearer password, got {}",
        code_with_token
    );

    // A missing error log must be an empty 200, not a 500 (dashboard U6).
    let empty_log_code = curl_code(
        &format!("{}/logs/errors", base),
        &["-H", "Authorization: Bearer testpassword"],
    )
    .unwrap_or_default();
    assert_eq!(
        empty_log_code, "200",
        "/logs/errors should return 200 when the file does not exist yet"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&tmp);
}
