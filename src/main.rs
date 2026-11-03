use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use log::{error, info, warn};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{broadcast, mpsc, Mutex as TokioMutex, Semaphore};

use convwatcher::cli::Cli;
use convwatcher::config;
use convwatcher::config::watch::WatchConfig;
use convwatcher::health;
use convwatcher::logs::error_logger::ErrorLogger;
use convwatcher::processor;
use convwatcher::processor::job::ConversionJob;
use convwatcher::utils::hardware::check_hardware_accel;
use convwatcher::watcher;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("ConvWatcher fatal error:");
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

    // Setup logging after loading config (uses app_log_file from LogConfig)
    convwatcher::logs::setup_logging(
        &cli.level.to_string(),
        Some("./logs/convwatcher.log"),
        cli.daemon,
        cli.no_daemon,
    )?;

    info!("ConvWatcher v{} starting", env!("CARGO_PKG_VERSION"));

    let (global_config, mut watch_configs, _registry) = config::load_config(cli.config.as_deref())?;
    info!("Global config loaded");
    info!("Loaded {} watcher config(s)", watch_configs.len());

    if let Some(watch_folder) = &cli.watch {
        let quick_config = WatchConfig {
            name: "quick".to_string(),
            watch_folder: watch_folder.clone(),
            output_folder: format!("{}-output", watch_folder.trim_end_matches('/')),
            subfolders: Vec::new(),
            watch_type: config::watch::WatchType::Video {
                rules: vec![config::watch::VideoRule {
                    preset: "h264_cpu".to_string(),
                    subfolder: None,
                    input_extensions: vec![
                        ".mp4".into(),
                        ".avi".into(),
                        ".mkv".into(),
                        ".mov".into(),
                        ".mxf".into(),
                    ],
                    output_ext: None,
                    codec: None,
                    quality: None,
                    audio_codec: None,
                    audio_bitrate: None,
                    output_name: None,
                    check_duration: Some(true),
                    min_duration_ratio: Some(0.9),
                }],
            },
        };
        watch_configs.push(quick_config);
    }

    for cfg in &watch_configs {
        watcher::monitor::create_folders(cfg)
            .with_context(|| format!("Cannot create folders for watcher '{}'", cfg.name))?;
    }

    let error_logger = Arc::new(ErrorLogger::new(&global_config)?);
    info!("Error logger initialized");

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
        .with_app_log("logs/convwatcher.log".to_string()),
    );

    for cfg in &watch_configs {
        health_server.add_watcher_with_config(cfg).await;
    }

    let health_handle = {
        let hs = health_server.clone();
        tokio::spawn(async move {
            let hs = hs.clone();
            if let Err(e) = tokio::task::spawn_blocking(move || hs.run()).await {
                error!("Health server error: {}", e);
            }
        })
    };

    let (job_tx, job_rx) = mpsc::channel::<ConversionJob>(100);

    let semaphore = Arc::new(Semaphore::new(
        global_config.max_concurrent_conversions as usize,
    ));

    let processing_files: Arc<TokioMutex<HashSet<PathBuf>>> =
        Arc::new(TokioMutex::new(HashSet::new()));

    let (monitor_shutdown_tx, _) = broadcast::channel::<()>(1);

    let global_for_reloader = global_config.clone();
    let disk_config_clone = global_config.disk_space.clone();
    let output_folders: Vec<String> = watch_configs
        .iter()
        .map(|c| c.output_folder.clone())
        .collect();
    let watch_folders: Vec<String> = watch_configs
        .iter()
        .map(|c| c.watch_folder.clone())
        .collect();

    let job_handle = {
        let health = health_server.clone();
        let err_logger = error_logger.clone();
        let sem = semaphore.clone();
        let ffmpeg = global_config.ffmpeg_path.clone();
        let ffprobe = global_config.ffprobe_path.clone().unwrap_or_else(|| {
            std::path::Path::new(&global_config.ffmpeg_path)
                .parent()
                .map(|p| p.join("ffprobe").to_string_lossy().to_string())
                .unwrap_or_else(|| "/usr/bin/ffprobe".to_string())
        });
        let pf = processing_files.clone();

        tokio::spawn(async move {
            process_jobs(
                job_rx,
                health,
                err_logger,
                disk_config_clone,
                sem,
                ffmpeg,
                ffprobe,
                pf,
            )
            .await;
        })
    };

    let disk_cfg_for_monitor = global_config.disk_space.clone();
    let disk_handle = {
        tokio::spawn(async move {
            processor::disk::disk_space_monitor(
                disk_cfg_for_monitor,
                output_folders,
                watch_folders,
            )
            .await;
        })
    };

    let (reload_tx, reload_rx) = mpsc::channel::<Vec<WatchConfig>>(10);
    let (config_tx, config_rx) = mpsc::channel::<Vec<WatchConfig>>(10);

    let cfg_path_for_reloader = cli.config.clone();
    let reload_tx_clone = reload_tx.clone();
    let config_tx_clone = config_tx.clone();
    let initial_config_sig = serde_yaml::to_string(&(&global_for_reloader, &watch_configs)).ok();

    let config_reload_handle = {
        tokio::spawn(async move {
            let interval = Duration::from_secs(global_for_reloader.config_refresh_interval_s);

            let mut last_sig = initial_config_sig;

            loop {
                tokio::time::sleep(interval).await;
                info!("Checking for config changes...");

                match config::load_config(cfg_path_for_reloader.as_deref()) {
                    Ok((global, new_configs, _)) => {
                        let sig = match serde_yaml::to_string(&(&global, &new_configs)) {
                            Ok(s) => s,
                            Err(e) => {
                                warn!("Failed to serialize config for diff: {}", e);
                                continue;
                            }
                        };
                        if last_sig.as_deref() != Some(sig.as_str()) {
                            last_sig = Some(sig);
                            let _ = reload_tx_clone.send(new_configs.clone()).await;
                            let _ = config_tx_clone.send(new_configs).await;
                        } else {
                            info!("No config changes detected");
                        }
                    }
                    Err(e) => warn!("Failed to reload config: {}", e),
                }
            }
        })
    };

    let embedded_scanner_handle = {
        let reload_tx = reload_tx.clone();
        let watchs_dir = String::from("config/watchs");
        let secret = global_config.embedded_secret.clone();
        let scan_interval = global_config.embedded_scan_interval_s;
        let main_configs = watch_configs.clone();

        tokio::spawn(async move {
            watcher::embedded::run_embedded_scanner(
                watchs_dir,
                secret,
                scan_interval,
                reload_tx,
                main_configs,
                config_rx,
            )
            .await;
        })
    };

    info!("ConvWatcher running. Press Ctrl+C to stop.");

    let mut mgmt_handle = {
        let health = health_server.clone();
        let tx = job_tx.clone();
        let gc = global_config.clone();
        let pf = processing_files.clone();
        let initial_configs = watch_configs.clone();
        let monitor_shutdown_rx = monitor_shutdown_tx.subscribe();
        tokio::spawn(async move {
            monitor_manager(
                initial_configs,
                reload_rx,
                tx,
                health,
                gc,
                pf,
                monitor_shutdown_rx,
            )
            .await;
        })
    };

    let mut term_signal = signal(SignalKind::terminate())?;
    let mut int_signal = signal(SignalKind::interrupt())?;
    tokio::select! {
        _ = term_signal.recv() => {
            info!("SIGTERM received, shutting down");
        }
        _ = int_signal.recv() => {
            info!("SIGINT received, shutting down");
        }
        _ = &mut mgmt_handle => {
            info!("Monitor manager exited");
        }
    }

    let _ = monitor_shutdown_tx.send(());
    info!("Waiting for subsystems to shut down...");

    let _ = mgmt_handle.await;

    config_reload_handle.abort();
    embedded_scanner_handle.abort();
    drop(reload_tx);

    drop(job_tx);
    let _ = job_handle.await;

    health_server.stop();
    health_handle.abort();
    disk_handle.abort();

    info!("ConvWatcher stopped");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_jobs(
    mut job_rx: mpsc::Receiver<ConversionJob>,
    health_server: Arc<health::server::HealthServer>,
    error_logger: Arc<ErrorLogger>,
    disk_config: convwatcher::config::global::DiskSpaceConfig,
    semaphore: Arc<Semaphore>,
    ffmpeg_path: String,
    ffprobe_path: String,
    processing_files: Arc<TokioMutex<HashSet<PathBuf>>>,
) {
    let mut tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    while let Some(job) = job_rx.recv().await {
        let hs = health_server.clone();
        let el = error_logger.clone();
        let dc = disk_config.clone();
        let sem = semaphore.clone();
        let ffmpeg = ffmpeg_path.clone();
        let ffprobe = ffprobe_path.clone();
        let pf = processing_files.clone();
        let file_path = job.file_path.clone();

        tasks.spawn(async move {
            let _permit = sem.acquire().await;
            processor::process_one(job, hs, el, dc, ffmpeg, ffprobe).await;
            pf.lock().await.remove(&file_path);
        });

        // Reap completed jobs so the set does not grow unbounded on a daemon
        // that runs for weeks.
        while tasks.try_join_next().is_some() {}
    }

    info!(
        "Job processor shutting down, draining {} in-flight jobs",
        tasks.len()
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !tasks.is_empty() {
        match tokio::time::timeout_at(deadline, tasks.join_next()).await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                info!("Job drain timeout reached, aborting remaining jobs");
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
            }
        }
    }

    info!("Job processor shut down complete");
}

fn spawn_monitors(
    configs: Vec<WatchConfig>,
    job_tx: &mpsc::Sender<ConversionJob>,
    health_server: &Arc<health::server::HealthServer>,
    global_config: &convwatcher::config::global::GlobalConfig,
    processing_files: &Arc<TokioMutex<HashSet<PathBuf>>>,
    shutdown_tx: &broadcast::Sender<()>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let check_interval = Duration::from_millis(global_config.file_check_interval_ms);
    let stable_time = Duration::from_millis(global_config.stable_time_ms);
    let mut handles = Vec::with_capacity(configs.len());

    for cfg in configs {
        let tx = job_tx.clone();
        let name = format!("watcher_{}", cfg.name);
        let hs = health_server.clone();
        let shutdown_rx = shutdown_tx.subscribe();
        let cfg_clone = cfg.clone();
        let watch_folder = cfg_clone.watch_folder.clone();
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

        handles.push(handle);
    }

    handles
}

async fn shutdown_monitors(handles: Vec<tokio::task::JoinHandle<()>>) {
    for mut handle in handles {
        if tokio::time::timeout(Duration::from_secs(5), &mut handle)
            .await
            .is_err()
        {
            warn!("Monitor did not stop within 5s; aborting");
            handle.abort();
            let _ = handle.await;
        }
    }
}

async fn monitor_manager(
    initial_configs: Vec<WatchConfig>,
    mut reload_rx: mpsc::Receiver<Vec<WatchConfig>>,
    job_tx: mpsc::Sender<ConversionJob>,
    health_server: Arc<health::server::HealthServer>,
    global_config: convwatcher::config::global::GlobalConfig,
    processing_files: Arc<TokioMutex<HashSet<PathBuf>>>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let mut generation_tx = broadcast::channel::<()>(1).0;
    let mut current_handles = spawn_monitors(
        initial_configs,
        &job_tx,
        &health_server,
        &global_config,
        &processing_files,
        &generation_tx,
    );
    info!("Spawned {} file monitor(s)", current_handles.len());

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => break,
            maybe = reload_rx.recv() => {
                let Some(new_configs) = maybe else { break; };

                let _ = generation_tx.send(());
                shutdown_monitors(std::mem::take(&mut current_handles)).await;

                for cfg in &new_configs {
                    if let Err(e) = watcher::monitor::create_folders(cfg) {
                        warn!("Failed to create folders: {}", e);
                    }
                    health_server.add_watcher_with_config(cfg).await;
                }

                generation_tx = broadcast::channel::<()>(1).0;
                current_handles = spawn_monitors(
                    new_configs,
                    &job_tx,
                    &health_server,
                    &global_config,
                    &processing_files,
                    &generation_tx,
                );

                info!("Monitors refreshed: {} watcher(s)", current_handles.len());
            }
        }
    }

    let _ = generation_tx.send(());
    shutdown_monitors(current_handles).await;
    info!("Monitor manager exited");
}
