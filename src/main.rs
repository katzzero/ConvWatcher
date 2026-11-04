use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use log::{error, info, warn};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{broadcast, mpsc, Semaphore};

use convwatcher::cli::Cli;
use convwatcher::config;
use convwatcher::config::watch::WatchConfig;
use convwatcher::daemon;
use convwatcher::daemon::monitors;
use convwatcher::health;
use convwatcher::logs::error_logger::ErrorLogger;
use convwatcher::processor;
use convwatcher::processor::context::ProcessorDeps;
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

    // Load config first to get app_log_file path for logging setup
    let (global_config, mut watch_configs, _registry) = config::load_config(cli.config.as_deref())?;
    info!("Global config loaded");
    info!("Loaded {} watcher config(s)", watch_configs.len());

    // Shared mutable view of GlobalConfig for hot-reload; monitor_manager reads
    // this when a reload arrives so global.* changes take effect.
    let shared_global = Arc::new(std::sync::RwLock::new(global_config.clone()));
    let config_dir = config::config_dir_for(cli.config.as_deref());

    // Timeout propagated per-job via ProcessorDeps (Arc<AtomicU64>). Applied
    // at startup with global_config.timeout_s (0 -> default 3600s).
    let timeout_secs: Arc<AtomicU64> = Arc::new(AtomicU64::new(if global_config.timeout_s == 0 {
        processor::context::DEFAULT_TIMEOUT_S
    } else {
        global_config.timeout_s
    }));

    // Setup logging after loading config (uses app_log_file from LogConfig)
    convwatcher::logs::setup_logging(
        &cli.level.to_string(),
        Some(&global_config.log.app_log_file),
        cli.daemon,
        cli.no_daemon,
    )?;

    info!("ConvWatcher v{} starting", env!("CARGO_PKG_VERSION"));

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
        // Validate the quick config
        config::validate_config(&global_config, std::slice::from_ref(&quick_config))
            .with_context(|| "Invalid quick watch config")?;
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
        .with_app_log(global_config.log.app_log_file.clone())
        .with_auth_token(global_config.healthcheck.auth_token.clone()),
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

    let processing_files: Arc<std::sync::Mutex<HashSet<PathBuf>>> =
        Arc::new(std::sync::Mutex::new(HashSet::new()));

    let (monitor_shutdown_tx, _) = broadcast::channel::<()>(8);

    let global_for_reloader = global_config.clone();
    let output_folders: Vec<String> = watch_configs
        .iter()
        .map(|c| c.output_folder.clone())
        .collect();
    let watch_folders: Vec<String> = watch_configs
        .iter()
        .map(|c| c.watch_folder.clone())
        .collect();

    // Shared dependencies for all conversion jobs.
    let deps = ProcessorDeps {
        health_server: health_server.clone(),
        error_logger: error_logger.clone(),
        disk_config: global_config.disk_space.clone(),
        ffmpeg_path: global_config.ffmpeg_path.clone(),
        ffprobe_path: global_config.ffprobe_path.clone().unwrap_or_else(|| {
            std::path::Path::new(&global_config.ffmpeg_path)
                .parent()
                .map(|p| p.join("ffprobe").to_string_lossy().to_string())
                .unwrap_or_else(|| "/usr/bin/ffprobe".to_string())
        }),
        timeout_secs: timeout_secs.clone(),
    };

    let job_handle = {
        let pf = processing_files.clone();

        tokio::spawn(async move {
            daemon::jobs::process_jobs(job_rx, deps, semaphore, pf).await;
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
    let shared_global_for_reloader = shared_global.clone();
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
                            *shared_global_for_reloader
                                .write()
                                .unwrap_or_else(|p| p.into_inner()) = global.clone();
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
        let watchs_dir = config_dir.join("watchs");
        let secret = global_config.embedded_secret.clone();
        let scan_interval = global_config.embedded_scan_interval_s;
        let main_configs = watch_configs.clone();

        tokio::spawn(async move {
            watcher::embedded::run_embedded_scanner(
                watchs_dir.to_string_lossy().to_string(),
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
        let shared_global = shared_global.clone();
        let config_dir = config_dir.clone();
        let timeout_secs = timeout_secs.clone();
        tokio::spawn(async move {
            monitors::monitor_manager(
                initial_configs,
                reload_rx,
                tx,
                health,
                gc,
                pf,
                monitor_shutdown_rx,
                shared_global,
                config_dir,
                timeout_secs,
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
    let _ = config_reload_handle.await;
    embedded_scanner_handle.abort();
    let _ = embedded_scanner_handle.await;
    drop(reload_tx);

    drop(job_tx);
    let _ = job_handle.await;

    health_server.stop();
    health_handle.abort();
    let _ = health_handle.await;
    disk_handle.abort();
    let _ = disk_handle.await;

    info!("ConvWatcher stopped");
    Ok(())
}
