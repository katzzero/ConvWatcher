//! ConvWatcher coordinator (server) binary.
//!
//! Behaves like the standalone daemon (watches folders, runs image/pdf/document
//! /custom conversions locally) but additionally discovers remote worker agents
//! and offloads video/audio conversions to them. If no agent is connected — or
//! a remote job fails — it falls back to local video/audio conversion.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use log::{error, info};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{broadcast, mpsc, Mutex as TokioMutex, Semaphore};

use convwatcher::cli::Cli;
use convwatcher::config;
use convwatcher::health;
use convwatcher::logs::error_logger::ErrorLogger;
use convwatcher::processor::job::ConversionJob;
use convwatcher::utils::hardware::check_hardware_accel;
use convwatcher::watcher;
use convwatcher::worker::dispatch::route_job;
use convwatcher::worker::WorkerPool;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("ConvWatcher server fatal error:");
        for (i, cause) in e.chain().enumerate() {
            if i == 0 {
                eprintln!("  {}", cause);
            } else {
                eprintln!("  caused by: {}", cause);
            }
        }
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    setup_logging(&cli);

    info!("ConvWatcher SERVER v{} starting", env!("CARGO_PKG_VERSION"));

    let (global_config, watch_configs, _registry) = config::load_config(cli.config.as_deref())?;
    info!("Loaded {} watcher config(s)", watch_configs.len());

    for cfg in &watch_configs {
        watcher::monitor::create_folders(cfg)
            .with_context(|| format!("Cannot create folders for watcher '{}'", cfg.name))?;
    }

    let error_logger = Arc::new(ErrorLogger::new(&global_config)?);

    let hw_info = Arc::new(check_hardware_accel(&global_config.ffmpeg_path).await);
    info!(
        "Hardware acceleration: VAAPI={}, NVENC={}, QSV={}, RKMPP={}",
        hw_info.vaapi_available,
        hw_info.nvenc_available,
        hw_info.qsv_available,
        hw_info.rkmpp_available
    );

    let health_server = Arc::new(
        health::server::HealthServer::new(
            global_config.healthcheck.http_port,
            global_config.healthcheck.bind_address.clone(),
            global_config.history.max_records,
        )
        .with_error_logger(global_config.log.errors_file.clone())
        .with_hardware_info((*hw_info).clone())
        .with_history_persistence(
            &global_config.history.file,
            global_config.history.persistent,
        )
        .with_app_log("logs/convwatcher-server.log".to_string()),
    );

    for cfg in &watch_configs {
        health_server.add_watcher_with_config(cfg).await;
    }

    let health_handle = {
        let hs = health_server.clone();
        tokio::spawn(async move {
            if let Err(e) = tokio::task::spawn_blocking(move || hs.run()).await {
                error!("Health server error: {}", e);
            }
        })
    };

    // --- Worker pool (coordinator) ---
    let pool = WorkerPool::new(global_config.embedded_secret.clone());
    let advertise = global_config
        .worker
        .advertise_address
        .clone()
        .unwrap_or_else(local_ip);
    pool.spawn(
        global_config.worker.bind_address.clone(),
        advertise,
        Some(global_config.worker.discovery_port),
        Some(global_config.worker.coordinator_port),
    );
    info!(
        "Coordinator ready: discovery udp:{}, agents tcp:{}",
        global_config.worker.discovery_port, global_config.worker.coordinator_port
    );

    let (job_tx, mut job_rx) = mpsc::channel::<ConversionJob>(100);
    let semaphore = Arc::new(Semaphore::new(
        global_config.max_concurrent_conversions as usize,
    ));
    let processing_files: Arc<TokioMutex<HashSet<PathBuf>>> =
        Arc::new(TokioMutex::new(HashSet::new()));

    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let mut monitor_handles = Vec::new();

    for cfg in &watch_configs {
        let tx = job_tx.clone();
        let check_interval = Duration::from_millis(global_config.file_check_interval_ms);
        let stable_time = Duration::from_millis(global_config.stable_time_ms);
        let name = format!("watcher_{}", cfg.name);
        let hs = health_server.clone();
        let cfg_clone = cfg.clone();
        let watch_folder = cfg_clone.watch_folder.clone();
        let shutdown_rx = shutdown_tx.subscribe();
        let gc = global_config.clone();
        let pf = processing_files.clone();

        let handle = tokio::spawn(async move {
            watcher::monitor::run_file_monitor(
                &watch_folder,
                tx,
                check_interval,
                stable_time,
                &name,
                hs,
                shutdown_rx,
                cfg_clone,
                gc,
                pf,
            )
            .await;
        });
        monitor_handles.push(handle);
    }
    info!("Spawned {} file monitor(s)", monitor_handles.len());

    // Disk monitor.
    let disk_handle = {
        let disk_cfg = global_config.disk_space.clone();
        let output_folders: Vec<String> = watch_configs
            .iter()
            .map(|c| c.output_folder.clone())
            .collect();
        let watch_folders: Vec<String> = watch_configs
            .iter()
            .map(|c| c.watch_folder.clone())
            .collect();
        tokio::spawn(async move {
            convwatcher::processor::disk::disk_space_monitor(
                disk_cfg,
                output_folders,
                watch_folders,
            )
            .await;
        })
    };

    // Job loop: route each job remotely (video/audio + agent) or locally.
    let worker_handle = {
        let health = health_server.clone();
        let err_logger = error_logger.clone();
        let sem = semaphore.clone();
        let ffmpeg = global_config.ffmpeg_path.clone();
        let ffprobe = global_config
            .ffprobe_path
            .clone()
            .unwrap_or_else(|| {
                std::path::Path::new(&global_config.ffmpeg_path)
                    .parent()
                    .map(|p| p.join("ffprobe").to_string_lossy().to_string())
                    .unwrap_or_else(|| "/usr/bin/ffprobe".to_string())
            });
        let disk_cfg = global_config.disk_space.clone();
        let pool = pool.clone();
        let pf = processing_files.clone();

        tokio::spawn(async move {
            while let Some(job) = job_rx.recv().await {
                let health = health.clone();
                let err_logger = err_logger.clone();
                let sem = sem.clone();
                let ffmpeg = ffmpeg.clone();
                let ffprobe = ffprobe.clone();
                let disk_cfg = disk_cfg.clone();
                let pool = pool.clone();
                let pf = pf.clone();
                let file_path = job.file_path.clone();

                tokio::spawn(async move {
                    let _permit = sem.acquire().await;
                    route_job(job, pool, health, err_logger, disk_cfg, ffmpeg, ffprobe).await;
                    pf.lock().await.remove(&file_path);
                });
            }
        })
    };

    info!("ConvWatcher server running. Press Ctrl+C to stop.");
    let mut term_signal = signal(SignalKind::terminate())?;
    let mut int_signal = signal(SignalKind::interrupt())?;
    tokio::select! {
        _ = term_signal.recv() => {
            info!("SIGTERM received, shutting down");
        }
        _ = int_signal.recv() => {
            info!("SIGINT received, shutting down");
        }
    }

    let _ = shutdown_tx.send(());
    for handle in monitor_handles {
        let _ = handle.await;
    }
    health_server.stop();
    health_handle.abort();
    worker_handle.abort();
    disk_handle.abort();

    info!("ConvWatcher server stopped");
    Ok(())
}

/// Best-effort local LAN IP for discovery advertisement. Opens a UDP socket to
/// a public address (no packets sent) to learn the outbound interface IP.
fn local_ip() -> String {
    use std::net::UdpSocket;
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

fn setup_logging(cli: &Cli) {
    let _ = std::fs::create_dir_all("logs");
    let level = cli.level.as_log_level_filter();
    let mut dispatch = fern::Dispatch::new()
        .level(level)
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} [{}] {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                message
            ))
        });
    dispatch = dispatch.chain(std::io::stdout());
    if let Ok(log_file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("./logs/convwatcher-server.log")
    {
        dispatch = dispatch.chain(fern::Dispatch::new().level(level).chain(log_file));
    }
    let _ = dispatch.apply();
}
