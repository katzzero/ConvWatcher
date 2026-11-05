//! End-to-end test for conversion progress + FFmpeg output.
//!
//! Spawns a real `convwatcher` subprocess with a `video` watcher and a real
//! ffmpeg/ffprobe. Generates a ~20s testsrc video (1280x720, libx265, ultrafast)
//! OUTSIDE the watch folder, then copies it in (already stable) and verifies:
//!   - `/api/queue` reports `progress` with `percent > 0` while converting
//!   - `/api/ffmpeg` is non-empty while converting
//!   - after completion, `/api/ffmpeg` is empty again
//!
//! Skips gracefully if ffmpeg is missing locally; in CI the convwatcher binary
//! and ffmpeg are required (panic if missing).

use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

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

fn ffmpeg_path() -> Option<String> {
    let f = std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string());
    let ok = std::path::Path::new(&f).exists();
    if !ok && std::env::var("CI").is_ok() {
        panic!("ffmpeg is required in CI");
    }
    ok.then_some(f)
}

fn ffprobe_path(ffmpeg: &str) -> String {
    std::path::Path::new(ffmpeg)
        .parent()
        .map(|p| p.join("ffprobe").to_string_lossy().to_string())
        .unwrap_or_else(|| "/usr/bin/ffprobe".to_string())
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

/// High port derived from PID to avoid collisions between parallel test runs.
fn high_port() -> u16 {
    let pid = std::process::id();
    (49152u32 + (pid % 1000)).try_into().unwrap_or(50000)
}

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
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return true;
        }
        std::thread::sleep(tick);
    }
    false
}

fn write_presets(config_dir: &std::path::Path) {
    let content = r#"presets:
  libx265:
    codec: libx265
    quality: crf 23
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
"#;
    let path = config_dir.join("video_codecs.yaml");
    if !path.exists() {
        let _ = std::fs::write(&path, content);
    }
    for (name, content) in [
        ("audio_codecs.yaml", "presets: {}\n"),
        ("image_codecs.yaml", "presets: {}\n"),
        ("pdf_presets.yaml", "presets: {}\n"),
        ("document_presets.yaml", "presets: {}\n"),
        ("custom_presets.yaml", "presets: {}\n"),
    ] {
        let path = config_dir.join(name);
        if !path.exists() {
            let _ = std::fs::write(&path, content);
        }
    }
}

#[derive(Deserialize)]
struct QueueResp {
    progress: HashMap<String, HashMap<String, ProgressInfo>>,
}

#[derive(Deserialize, Default)]
#[allow(dead_code)]
struct ProgressInfo {
    percent: Option<f64>,
    out_time: Option<f64>,
    speed: Option<f64>,
    eta: Option<f64>,
}

#[derive(Deserialize, Default)]
struct FfmpegResp {
    jobs: HashMap<String, HashMap<String, Vec<String>>>,
}

/// Gera um vídeo de teste com testsrc (~20s, 1280x720, libx265 ultrafast) fora
/// da watch folder, para que chegue já estável ao monitor.
fn make_test_video(path: &std::path::Path, ffmpeg: &str) {
    let status = std::process::Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=20:size=1280x720:rate=30",
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-crf",
            "28",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            path.to_str().unwrap(),
        ])
        .status()
        .expect("ffmpeg deve estar disponível para gerar o vídeo de teste");
    assert!(
        status.success(),
        "falha ao gerar vídeo de teste com testsrc"
    );
}

fn config_yaml(
    ffmpeg: &str,
    ffprobe: &str,
    watch: &str,
    output: &str,
    logs: &str,
    port: u16,
) -> String {
    format!(
        r#"global:
  file_check_interval: 200ms
  stable_time: 100ms
  ffmpeg_path: {ffmpeg}
  ffprobe_path: {ffprobe}
  max_concurrent: 1
  refresh_interval: 1s
  embedded_secret: ""
  embedded_scan_interval: 0
  timeout: 60

  log:
    errors_file: {logs}/errors.log
    app_log_file: {logs}/app.log

  healthcheck:
    http_port: {port}
    bind_address: 127.0.0.1

  disk_space:
    check_interval: 60s
    threshold: 500

  history:
    persistent: false
    file: {logs}/history.json
    max_records: 50

watchers:
- name: progress_watcher
  watch_folder: {watch}
  output_folder: {output}
  type: video
  rules:
    - input_extensions: [.mp4]
      preset: libx265
"#
    )
}

#[test]
fn progress_and_ffmpeg_output_e2e() {
    let bin = match convwatcher_bin() {
        Some(p) => p,
        None => {
            eprintln!("convwatcher binary not found — skipping progress e2e test");
            return;
        }
    };
    let ffmpeg = match ffmpeg_path() {
        Some(p) => p,
        None => {
            eprintln!("ffmpeg unavailable — skipping progress e2e test");
            return;
        }
    };
    if !curl_available() {
        eprintln!("curl not available — skipping progress e2e test");
        return;
    }

    let port = high_port();
    let base = format!("http://127.0.0.1:{}", port);

    let tmp = std::env::temp_dir().join(format!("cw-progress-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let config_dir = tmp.join("config");
    let watch = tmp.join("watch");
    let output = tmp.join("output");
    let logs = tmp.join("logs");
    let staged = tmp.join("staged");
    for d in [&config_dir, &watch, &output, &logs, &staged] {
        std::fs::create_dir_all(d).unwrap();
    }
    write_presets(&config_dir);

    let config_path = config_dir.join("config.yaml");
    std::fs::write(
        &config_path,
        config_yaml(
            &ffmpeg,
            &ffprobe_path(&ffmpeg),
            watch.to_str().unwrap(),
            output.to_str().unwrap(),
            logs.to_str().unwrap(),
            port,
        ),
    )
    .unwrap();

    // Vídeo estável fora da watch folder (será copiado para dentro depois de subir).
    let staged_video = staged.join("input.mp4");
    make_test_video(&staged_video, &ffmpeg);

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

    let up = poll(
        || curl_code(&format!("{}/health", base), &[]).is_some_and(|c| c == "200"),
        Duration::from_secs(20),
        Duration::from_millis(300),
    );
    assert!(up, "health server did not start on {} within timeout", base);

    // Coloca o vídeo já estável na watch folder.
    let input = watch.join("input.mp4");
    std::fs::copy(&staged_video, &input).expect("failed to copy staged video into watch folder");

    // Enquanto converte, /api/queue deve reportar progress.percent > 0 e
    // /api/ffmpeg deve estar não vazio.
    let started = Instant::now();
    let mut saw_progress = false;
    let mut saw_ffmpeg = false;
    let observe_until = started + Duration::from_secs(60);
    while Instant::now() < observe_until {
        let queue_body = curl_body(&format!("{}/api/queue", base), &[]);
        if let Some(body) = &queue_body {
            if let Ok(q) = serde_json::from_str::<QueueResp>(body) {
                for files in q.progress.values() {
                    for p in files.values() {
                        if let Some(pct) = p.percent {
                            if pct > 0.0 {
                                saw_progress = true;
                            }
                        }
                    }
                }
            }
        }
        let ff_body = curl_body(&format!("{}/api/ffmpeg", base), &[]);
        if let Some(body) = &ff_body {
            if let Ok(f) = serde_json::from_str::<FfmpegResp>(body) {
                if !f.jobs.is_empty() {
                    saw_ffmpeg = true;
                }
            }
        }
        if saw_progress && saw_ffmpeg {
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
    }

    // Aguarda a conversão concluir (output file aparece).
    let output_file = output.join("input_libx265_0.mp4");
    let finished = poll(
        || output_file.exists(),
        Duration::from_secs(120),
        Duration::from_millis(500),
    );
    if !finished {
        let _ = std::fs::read_to_string(logs.join("errors.log"))
            .ok()
            .map(|c| eprintln!("errors.log:\n{}", c));
    }
    assert!(
        finished,
        "conversão não produziu output file dentro do timeout"
    );

    // Após concluir, /api/ffmpeg deve estar vazio (clear_processing limpou).
    let ff_empty = poll(
        || {
            curl_body(&format!("{}/api/ffmpeg", base), &[])
                .and_then(|b| serde_json::from_str::<FfmpegResp>(&b).ok())
                .map(|f| f.jobs.is_empty())
                .unwrap_or(false)
        },
        Duration::from_secs(10),
        Duration::from_millis(300),
    );
    assert!(
        ff_empty,
        "/api/ffmpeg should be empty after conversion completes"
    );

    assert!(
        saw_progress,
        "never observed progress.percent > 0 during conversion"
    );
    assert!(
        saw_ffmpeg,
        "never observed non-empty /api/ffmpeg during conversion"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&tmp);
}
