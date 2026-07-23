//! End-to-end integration test for the remote worker system.
//!
//! Spins up an in-process coordinator (`WorkerPool`) and connects a real
//! `convwatcher-agent` subprocess, then runs a real video conversion across the
//! wire (input streamed to the agent, output streamed back). Validates the full
//! discovery→register→job→stream→result path plus local fallback when no agent
//! is present.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use convwatcher::config::watch::VideoRule;
use convwatcher::health::server::HealthServer;
use convwatcher::logs::error_logger::ErrorLogger;
use convwatcher::processor::job::{ConversionJob, MatchedRule};
use convwatcher::worker::coordinator::WorkerPool;
use convwatcher::worker::dispatch::route_job;
use convwatcher_common::protocol::{JobKind, WireVideoRule};

fn agent_bin() -> std::path::PathBuf {
    // Built by `cargo test` before integration tests run.
    let mut p = std::env::current_exe().unwrap();
    p.pop(); // debug/
    p.pop(); // deps/ -> debug/
    p.push("convwatcher-agent");
    if !p.exists() {
        // `cargo test` runs tests from target/debug/deps; the binary sits one
        // level up in target/debug.
        p.pop();
        p.pop();
        p.push("convwatcher-agent");
    }
    assert!(p.exists(), "agent binary not found at {:?}", p);
    p
}

fn ffmpeg() -> String {
    std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "/opt/homebrew/bin/ffmpeg".to_string())
}

fn sibling_ffprobe(ffmpeg_path: &str) -> String {
    std::path::Path::new(ffmpeg_path)
        .parent()
        .map(|p| p.join("ffprobe").to_string_lossy().to_string())
        .unwrap_or_else(|| "/usr/bin/ffprobe".to_string())
}

fn ffprobe() -> String {
    sibling_ffprobe(&ffmpeg())
}

/// Path to the local-only test asset (HEVC .mov). Returns `None` when the
/// file is absent so tests can skip gracefully — the asset is never committed.
fn test_mov() -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-sample/test.mov");
    if p.exists() { Some(p) } else { None }
}

/// Check whether ffmpeg/ffprobe are available. Returns `true` if both exist.
fn ffmpeg_available() -> bool {
    let f = ffmpeg();
    let fp = ffprobe();
    std::path::Path::new(&f).exists() && std::path::Path::new(&fp).exists()
}

async fn wait_for_agent(pool: &Arc<WorkerPool>, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if pool.agent_count().await > 0 {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn remote_video_job_streams_back() {
    let secret = "test-secret";
    let discovery_port = 18687;
    let coordinator_port = 18688;

    let pool = WorkerPool::new(secret.to_string());
    pool.spawn(
        "127.0.0.1".to_string(),
        "127.0.0.1".to_string(),
        Some(discovery_port),
        Some(coordinator_port),
    );

    let tmp = std::env::temp_dir().join(format!("cw-e2e-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    // Build a tiny test input video with ffmpeg (if available).
    let input = tmp.join("in.webm");
    let out = tmp.join("out.mp4");
    let mk = Command::new(ffmpeg())
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=128x96:rate=10",
            "-pix_fmt",
            "yuv420p",
            input.to_str().unwrap(),
        ])
        .output();
    let ffmpeg_ok = matches!(mk, Ok(ref o) if o.status.success());
    if !ffmpeg_ok {
        eprintln!("ffmpeg unavailable or failed — skipping remote e2e test");
        return;
    }

    // Launch the agent, connecting directly to the coordinator (no discovery).
    let mut agent = Command::new(agent_bin())
        .args([
            "--coordinator-addr",
            &format!("127.0.0.1:{coordinator_port}"),
            "--secret",
            secret,
            "--ffmpeg-path",
            &ffmpeg(),
            "--ffprobe-path",
            &ffprobe(),
            "--temp-dir",
            tmp.to_str().unwrap(),
            "--io-mode",
            "temp",
            "--level",
            "debug",
        ])
        .spawn()
        .expect("spawn agent");

    assert!(
        wait_for_agent(&pool, Duration::from_secs(10)).await,
        "agent did not register"
    );

    let rule = WireVideoRule {
        codec: Some("libx264".into()),
        quality: Some("crf 23".into()),
        audio_codec: Some("aac".into()),
        audio_bitrate: Some("128k".into()),
        check_duration: Some(false),
        min_duration_ratio: Some(0.9),
    };

    // Retry dispatch a few times in case the connection is still settling.
    let mut dispatched = false;
    for _ in 0..20 {
        let job = convwatcher::worker::coordinator::RemoteJob::new(
            JobKind::Video,
            Some(rule.clone()),
            None,
            "mp4",
            &input,
            &out,
        );
        match pool.dispatch(job).await {
            Ok(true) => {
                dispatched = true;
                break;
            }
            Ok(false) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => panic!("remote job failed: {e:#}"),
        }
    }
    assert!(dispatched, "agent never accepted the job");
    assert!(out.exists(), "output file not produced: {:?}", out);
    let size = std::fs::metadata(&out).unwrap().len();
    assert!(size > 0, "output file is empty");

    let _ = std::process::Command::new("kill")
        .arg(agent.id().to_string())
        .status();
    let _ = agent.kill();
    let _ = agent.wait();
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn no_agent_falls_back_to_local() {
    // No coordinator/agent running — dispatch must report "no agent" (Ok(false))
    // so the caller can process locally.
    let pool = WorkerPool::new("x".to_string());
    let tmp = std::env::temp_dir().join(format!("cw-e2e-fb-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let input = tmp.join("in.webm");
    let out = tmp.join("out.mp4");

    let job = convwatcher::worker::coordinator::RemoteJob::new(
        JobKind::Video,
        Some(WireVideoRule::default()),
        None,
        "mp4",
        &input,
        &out,
    );

    let res = pool.dispatch(job).await.unwrap();
    assert!(!res, "expected Ok(false) when no agent is connected");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn route_job_falls_back_to_local() {
    // Drive route_job with no agent and a ConversionJob; local processing
    // must produce output even though the matched_rule is Video.
    let ffmpeg = ffmpeg();
    let ffprobe = ffmpeg.replace("ffmpeg", "ffprobe");
    let pool = WorkerPool::new("x".to_string());
    let tmp = std::env::temp_dir().join(format!("cw-e2e-rjfb-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);

    // Build a tiny test input.
    let input = tmp.join("in.webm");
    let out_dir = tmp.join("outputs");
    let _ = std::fs::create_dir_all(&out_dir);
    let mk = Command::new(&ffmpeg)
        .args([
            "-y", "-f", "lavfi", "-i", "testsrc=duration=1:size=128x96:rate=10",
            "-pix_fmt", "yuv420p",
            input.to_str().unwrap(),
        ])
        .output();
    let ffmpeg_ok = matches!(mk, Ok(ref o) if o.status.success());
    if !ffmpeg_ok {
        eprintln!("ffmpeg unavailable or failed — skipping route_job fallback test");
        let _ = std::fs::remove_dir_all(&tmp);
        return;
    }

    let health = Arc::new(
        HealthServer::new(0, "127.0.0.1".to_string(), 100)
    );
    let err_log_path = tmp.join("errors.log");
    let global_cfg = convwatcher::config::global::GlobalConfig {
        log: convwatcher::config::global::LogConfig {
            errors_file: err_log_path.to_string_lossy().to_string(),
            max_error_log_size_mb: 50,
            ..Default::default()
        },
        ..Default::default()
    };
    let error_logger = Arc::new(ErrorLogger::new(&global_cfg).unwrap());

    let rule = VideoRule {
        preset: "libx264".to_string(),
        subfolder: None,
        input_extensions: vec![".webm".into()],
        output_ext: Some(".mp4".to_string()),
        codec: Some("libx264".to_string()),
        quality: Some("crf 28".to_string()),
        audio_codec: Some("aac".to_string()),
        audio_bitrate: Some("128k".to_string()),
        output_name: None,
        check_duration: Some(false),
        min_duration_ratio: None,
    };

    let job = ConversionJob {
        watcher_name: "test".to_string(),
        file_name: "in.webm".to_string(),
        file_path: input.clone(),
        matched_rule: MatchedRule::Video(rule),
        output_folder: out_dir.to_string_lossy().to_string(),
        watch_folder: tmp.to_string_lossy().to_string(),
        input_file_action: convwatcher::config::global::InputFileAction::Mark,
    };

    let disk_config = convwatcher::config::global::DiskSpaceConfig::default();

    route_job(
        job,
        pool,
        health,
        error_logger,
        disk_config,
        ffmpeg,
        ffprobe,
    )
    .await;

    // Local fallback should have produced at least one .mp4 in the output dir.
    let has_output = std::fs::read_dir(&out_dir)
        .ok()
        .map(|entries| entries.filter_map(|e| e.ok()).any(|e| e.path().extension().is_some_and(|ext| ext == "mp4")))
        .unwrap_or(false);
    assert!(has_output, "local fallback must produce output when no agent is available");

    let _ = std::fs::remove_dir_all(&tmp);
}

// ── Real-asset integration tests (require test-sample/test.mov) ─────────

#[tokio::test]
async fn route_job_converts_real_video_locally() {
    // T2 — real HEVC→H.264 conversion via route_job local fallback.
    let mov = match test_mov() {
        Some(p) => p,
        None => {
            eprintln!("test.mov not found — skipping route_job_converts_real_video_locally");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping route_job_converts_real_video_locally");
        return;
    }

    let pool = WorkerPool::new("x".to_string());
    let tmp = std::env::temp_dir().join(format!("cw-e2e-rjrv-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let out_dir = tmp.join("outputs");
    let _ = std::fs::create_dir_all(&out_dir);

    // Copy the asset into the temp dir so handle_input_file's in-place
    // mark_done operates on the copy, not the local-only original.
    let input_copy = tmp.join("test.mov");
    std::fs::copy(&mov, &input_copy)
        .expect("failed to stage test.mov copy for local fallback test");

    let health = Arc::new(HealthServer::new(0, "127.0.0.1".to_string(), 100));
    let err_log_path = tmp.join("errors.log");
    let global_cfg = convwatcher::config::global::GlobalConfig {
        log: convwatcher::config::global::LogConfig {
            errors_file: err_log_path.to_string_lossy().to_string(),
            max_error_log_size_mb: 50,
            ..Default::default()
        },
        ..Default::default()
    };
    let error_logger = Arc::new(ErrorLogger::new(&global_cfg).unwrap());

    let rule = VideoRule {
        preset: "libx264".to_string(),
        subfolder: None,
        input_extensions: vec![".mov".into()],
        output_ext: Some(".mp4".to_string()),
        codec: Some("libx264".to_string()),
        quality: Some("crf 28".to_string()),
        audio_codec: Some("aac".to_string()),
        audio_bitrate: Some("128k".to_string()),
        output_name: None,
        check_duration: Some(false),
        min_duration_ratio: None,
    };

    let job = ConversionJob {
        watcher_name: "test".to_string(),
        file_name: "test.mov".to_string(),
        file_path: input_copy.clone(),
        matched_rule: MatchedRule::Video(rule),
        output_folder: out_dir.to_string_lossy().to_string(),
        watch_folder: tmp.to_string_lossy().to_string(),
        input_file_action: convwatcher::config::global::InputFileAction::Mark,
    };

    let disk_config = convwatcher::config::global::DiskSpaceConfig::default();

    route_job(
        job,
        pool,
        health,
        error_logger,
        disk_config,
        ffmpeg(),
        ffprobe(),
    )
    .await;

    // Find the output .mp4.
    let output = std::fs::read_dir(&out_dir)
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .find(|e| e.path().extension().is_some_and(|ext| ext == "mp4"))
                .map(|e| e.path())
        })
        .expect("local fallback should produce an .mp4 output from real test.mov");

    let meta = std::fs::metadata(&output).unwrap();
    assert!(meta.len() > 0, "output file must not be empty");

    // Verify ffprobe confirms h264 video stream in a valid mp4.
    let probe_out = tokio::process::Command::new(ffprobe())
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=codec_name",
            "-of", "default=noprint_wrappers=1:nokey=1",
            output.to_str().unwrap(),
        ])
        .output()
        .await
        .expect("ffprobe failed");
    let probe_stdout = String::from_utf8_lossy(&probe_out.stdout);
    assert!(
        probe_stdout.trim() == "h264",
        "expected h264 stream, got: {probe_stdout}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

/// Helper: spawn an agent, wait for registration, run a remote job, verify
/// output, clean up. Used by both temp-mode and pipe-mode tests.
async fn run_remote_agent_job(
    pool: &Arc<WorkerPool>,
    io_mode: &str,
    input: &std::path::Path,
    output: &std::path::Path,
    coordinator_port: u16,
    secret: &str,
    tmp: &std::path::Path,
) {
    let mut agent = std::process::Command::new(agent_bin())
        .args([
            "--coordinator-addr",
            &format!("127.0.0.1:{coordinator_port}"),
            "--secret",
            secret,
            "--ffmpeg-path",
            &ffmpeg(),
            "--ffprobe-path",
            &ffprobe(),
            "--temp-dir",
            tmp.to_str().unwrap(),
            "--io-mode",
            io_mode,
            "--level",
            "debug",
        ])
        .spawn()
        .expect("spawn agent");

    assert!(
        wait_for_agent(pool, Duration::from_secs(10)).await,
        "agent did not register"
    );

    let rule = WireVideoRule {
        codec: Some("libx264".into()),
        quality: Some("crf 23".into()),
        audio_codec: Some("aac".into()),
        audio_bitrate: Some("128k".into()),
        check_duration: Some(false),
        min_duration_ratio: Some(0.9),
    };

    let mut dispatched = false;
    for _ in 0..20 {
        let job = convwatcher::worker::coordinator::RemoteJob::new(
            JobKind::Video,
            Some(rule.clone()),
            None,
            "mp4",
            input,
            output,
        );
        match pool.dispatch(job).await {
            Ok(true) => {
                dispatched = true;
                break;
            }
            Ok(false) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                let _ = agent.kill();
                let _ = agent.wait();
                panic!("remote job failed: {e:#}");
            }
        }
    }
    assert!(dispatched, "agent never accepted the job");

    // Allow a generous timeout for the conversion to finish.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut produced = false;
    while tokio::time::Instant::now() < deadline {
        if output.exists() {
            produced = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let _ = std::process::Command::new("kill")
        .arg(agent.id().to_string())
        .status();
    let _ = agent.kill();
    let _ = agent.wait();

    assert!(produced, "output file not produced within 30s (io_mode={io_mode})");
    let size = std::fs::metadata(output).unwrap().len();
    assert!(size > 0, "output file is empty (io_mode={io_mode})");
}

#[tokio::test]
async fn remote_agent_streams_real_video_temp() {
    // T3a — temp mode with real HEVC input.
    let mov = match test_mov() {
        Some(p) => p,
        None => {
            eprintln!("test.mov not found — skipping remote_agent_streams_real_video_temp");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping remote_agent_streams_real_video_temp");
        return;
    }

    let secret = "test-secret-rv";
    let coordinator_port = 18689;
    let pool = WorkerPool::new(secret.to_string());
    pool.spawn(
        "127.0.0.1".to_string(),
        "127.0.0.1".to_string(),
        Some(18690),
        Some(coordinator_port),
    );

    let tmp = std::env::temp_dir().join(format!("cw-e2e-rv-temp-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let out = tmp.join("out.mp4");

    run_remote_agent_job(&pool, "temp", &mov, &out, coordinator_port, secret, &tmp).await;

    // Verify the output is a valid h264 mp4 via ffprobe.
    let probe_out = tokio::process::Command::new(ffprobe())
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=codec_name",
            "-of", "default=noprint_wrappers=1:nokey=1",
            out.to_str().unwrap(),
        ])
        .output()
        .await
        .expect("ffprobe failed");
    let probe_stdout = String::from_utf8_lossy(&probe_out.stdout);
    assert!(
        probe_stdout.trim() == "h264",
        "expected h264 stream from remote temp conversion, got: {probe_stdout}"
    );

    // Also check duration is roughly preserved (~1.8s ± 0.2).
    let dur_out = tokio::process::Command::new(ffprobe())
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            out.to_str().unwrap(),
        ])
        .output()
        .await
        .expect("ffprobe failed");
    let dur_stdout = String::from_utf8_lossy(&dur_out.stdout);
    let dur: f64 = dur_stdout.trim().parse().unwrap_or(0.0);
    assert!(
        (dur - 1.835).abs() < 0.2,
        "expected duration ~1.835s, got {dur}s"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn remote_agent_streams_real_video_pipe() {
    // T3b — pipe mode with real HEVC input (exercises 04 §H1 concurrent
    // stderr drain and 04 §H3 overshoot guard).
    let mov = match test_mov() {
        Some(p) => p,
        None => {
            eprintln!("test.mov not found — skipping remote_agent_streams_real_video_pipe");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping remote_agent_streams_real_video_pipe");
        return;
    }

    let secret = "test-secret-rvp";
    let coordinator_port = 18691;
    let pool = WorkerPool::new(secret.to_string());
    pool.spawn(
        "127.0.0.1".to_string(),
        "127.0.0.1".to_string(),
        Some(18692),
        Some(coordinator_port),
    );

    let tmp = std::env::temp_dir().join(format!("cw-e2e-rv-pipe-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let out = tmp.join("out.mp4");

    run_remote_agent_job(
        &pool,
        "pipe",
        &mov,
        &out,
        coordinator_port,
        secret,
        &tmp,
    )
    .await;

    // Quick validation: non-empty, valid container.
    let probe_out = tokio::process::Command::new(ffprobe())
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=codec_name",
            "-of", "default=noprint_wrappers=1:nokey=1",
            out.to_str().unwrap(),
        ])
        .output()
        .await
        .expect("ffprobe failed");
    let probe_stdout = String::from_utf8_lossy(&probe_out.stdout);
    assert!(
        probe_stdout.trim() == "h264",
        "expected h264 stream from remote pipe conversion, got: {probe_stdout}"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn stability_timer_handles_real_file_growth() {
    // T5 — real-file counterpart to the unit test: write a file, then
    // modify its size after a short delay. The monitor must NOT enqueue
    // the file until stable_time after the LAST modification (regression
    // guard for 03 §H2 — first_seen must reset on size change).
    let mov = match test_mov() {
        Some(p) => p,
        None => {
            eprintln!("test.mov not found — skipping stability_timer_handles_real_file_growth");
            return;
        }
    };
    if !ffmpeg_available() {
        eprintln!("ffmpeg unavailable — skipping stability_timer_handles_real_file_growth");
        return;
    }

    let tmp = std::env::temp_dir().join(format!("cw-e2e-stab-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let watch_dir = tmp.join("watch");
    let _ = std::fs::create_dir_all(&watch_dir);
    let out_dir = tmp.join("outputs");
    let _ = std::fs::create_dir_all(&out_dir);

    // Set up a minimal monitor with a short stable_time.
    let (job_tx, mut job_rx) = tokio::sync::mpsc::channel::<ConversionJob>(10);
    let short_stable = Duration::from_millis(300);
    let check_interval = Duration::from_millis(50);

    let watch_path = watch_dir.to_string_lossy().to_string();
    let out_path = out_dir.to_string_lossy().to_string();
    let cfg = convwatcher::config::watch::WatchConfig {
        name: "stability-test".to_string(),
        watch_folder: watch_path.clone(),
        output_folder: out_path.clone(),
        subfolders: vec![],
        watch_type: convwatcher::config::watch::WatchType::Video {
            rules: vec![VideoRule {
                preset: "libx264".to_string(),
                subfolder: None,
                input_extensions: vec![".mov".into()],
                output_ext: Some(".mp4".to_string()),
                codec: Some("libx264".to_string()),
                quality: Some("crf 28".to_string()),
                audio_codec: Some("aac".to_string()),
                audio_bitrate: Some("128k".to_string()),
                output_name: None,
                check_duration: Some(false),
                min_duration_ratio: None,
            }],
        },
    };

    let health = Arc::new(HealthServer::new(0, "127.0.0.1".to_string(), 100));
    let gc = convwatcher::config::global::GlobalConfig::default();
    let pf = Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new()));

    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    // Write a small initial chunk.
    let staged = watch_dir.join("test.mov");
    std::fs::write(&staged, b"initial partial content").unwrap();

    // Spawn the monitor.
    let monitor_handle = tokio::spawn({
        let watch_path = watch_path.clone();
        let cfg = cfg.clone();
        let gc = gc.clone();
        let pf = pf.clone();
        let health = health.clone();
        async move {
            convwatcher::watcher::monitor::run_file_monitor(
                &watch_path,
                job_tx,
                check_interval,
                short_stable,
                "stability_test",
                health,
                shutdown_rx,
                cfg,
                gc,
                pf,
            )
            .await;
        }
    });

    // Wait less than stable_time, then modify the file (simulating a
    // download in progress). Without the 03 §H2 fix, first_seen from
    // the initial write would not be reset, and once elapsed > stable_time
    // the file would be enqueued prematurely.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Replace the file with the real full content (size change).
    let full_content = std::fs::read(&mov).unwrap();
    std::fs::write(&staged, &full_content).unwrap();

    // Wait for stable_time after the modification.
    tokio::time::sleep(short_stable + Duration::from_millis(200)).await;

    // Exactly one job should have been produced (after the full write
    // stabilized). If 0: the file wasn't enqueued at all. If >1: the
    // 03 §H2 fix isn't working (first_seen wasn't reset, file was
    // enqueued based on the old first_seen).
    let mut jobs_received = 0;
    loop {
        match job_rx.try_recv() {
            Ok(_) => jobs_received += 1,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }
    }

    assert_eq!(
        jobs_received, 1,
        "expected exactly one job after full write, got {jobs_received}. \
         If 0: the full-file write was never enqueued. \
         If >1: first_seen wasn't reset on size change (03 §H2 regression)"
    );

    let _ = shutdown_tx.send(());
    let _ = monitor_handle.await;
    let _ = std::fs::remove_dir_all(&tmp);
}
