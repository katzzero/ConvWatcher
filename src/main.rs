mod cli;
mod config;
mod health;
mod logs;
mod processor;
mod utils;
mod watcher;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use log::{error, info, warn};
use tokio::sync::{broadcast, mpsc, Semaphore};

use cli::Cli;
use config::watch::WatchConfig;
use logs::error_logger::ErrorLogger;
use processor::job::ConversionJob;
use utils::hardware::check_hardware_accel;

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
    setup_logging(&cli)?;

    info!("ConvWatcher v{} starting", env!("CARGO_PKG_VERSION"));

    let global_config = config::load_global_config(cli.config.as_deref())?;
    info!("Global config loaded");

    let mut watch_configs =
        config::load_watch_configs(cli.config.as_deref(), &global_config)?;
    info!("Loaded {} watcher config(s)", watch_configs.len());

    if let Some(watch_folder) = &cli.watch {
        let quick_config = WatchConfig {
            name: "quick".to_string(),
            watch_folder: watch_folder.clone(),
            output_folder: format!("{}-output", watch_folder.trim_end_matches('/')),
            watch_type: config::watch::WatchType::Video {
                video: Vec::new(),
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
        "Hardware acceleration: VAAPI={}, NVENC={}, QSV={}",
        hw_info.vaapi_available, hw_info.nvenc_available, hw_info.qsv_available
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
        ),
    );

    for cfg in &watch_configs {
        health_server.add_watcher_with_config(cfg).await;
    }

    let health_handle = {
        let hs = health_server.clone();
        tokio::spawn(async move {
            if let Err(e) = hs.run().await {
                error!("Health server error: {}", e);
            }
        })
    };

    let (job_tx, job_rx) = mpsc::channel::<ConversionJob>(100);

    let semaphore = Arc::new(Semaphore::new(
        global_config.max_concurrent_conversions as usize,
    ));

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
            )
            .await;
        });

        monitor_handles.push(handle);
    }

    info!("Spawned {} file monitor(s)", monitor_handles.len());

    let global_for_reloader = global_config.clone();
    let disk_config_clone = global_config.disk_space.clone();
    let output_folders: Vec<String> =
        watch_configs.iter().map(|c| c.output_folder.clone()).collect();
    let watch_folders: Vec<String> =
        watch_configs.iter().map(|c| c.watch_folder.clone()).collect();

    let worker_handle = {
        let health = health_server.clone();
        let err_logger = error_logger.clone();
        let sem = semaphore.clone();
        let ffmpeg = global_config.ffmpeg_path.clone();
        let ffprobe = global_config.ffprobe_path.clone();

        tokio::spawn(async move {
            process_jobs(job_rx, health, err_logger, disk_config_clone, sem, ffmpeg, ffprobe).await;
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

    let cfg_path_for_reloader = cli.config.clone();
    let reload_tx_clone = reload_tx.clone();
    let global_for_hotreload = global_config.clone();
    let config_reload_handle = {
        tokio::spawn(async move {
            let interval =
                Duration::from_secs(global_for_reloader.config_refresh_interval_s);

            loop {
                tokio::time::sleep(interval).await;
                info!("Checking for config changes...");

                if let Ok(new_configs) =
                    config::load_watch_configs(cfg_path_for_reloader.as_deref(), &global_for_hotreload)
                {
                    let _ = reload_tx_clone.send(new_configs).await;
                }
            }
        })
    };

    let embedded_scanner_handle = {
        let reload_tx = reload_tx.clone();
        let watchs_dir = global_config.watchs_dir.clone();
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
            )
            .await;
        })
    };

    info!("ConvWatcher running. Press Ctrl+C to stop.");

    let mgmt_handle = {
        let health = health_server.clone();
        let tx = job_tx.clone();
        let gc = global_config.clone();
        tokio::spawn(async move {
            monitor_manager(reload_rx, tx, health, &gc).await;
        })
    };

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Shutdown signal received");
        }
        _ = mgmt_handle => {
            info!("Monitor manager exited");
        }
    }

    let _ = shutdown_tx.send(());
    info!("Waiting for subsystems to shut down...");

    for handle in monitor_handles {
        let _ = handle.await;
    }

    health_handle.abort();
    worker_handle.abort();
    disk_handle.abort();
    config_reload_handle.abort();
    embedded_scanner_handle.abort();

    info!("ConvWatcher stopped");
    Ok(())
}

fn setup_logging(cli: &Cli) -> Result<()> {
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

    if cli.daemon {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("./logs/convwatcher.log")
        {
            Ok(log_file) => dispatch = dispatch.chain(log_file),
            Err(e) => eprintln!("Warning: cannot open log file: {}", e),
        }
    } else {
        dispatch = dispatch.chain(
            fern::Dispatch::new()
                .level(level)
                .filter(|meta| meta.level() <= log::Level::Warn)
                .chain(std::io::stderr()),
        );
        dispatch = dispatch.chain(
            fern::Dispatch::new()
                .level(level)
                .chain(std::io::stdout()),
        );
        if let Ok(log_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("./logs/convwatcher.log")
        {
            dispatch = dispatch.chain(fern::Dispatch::new().level(level).chain(log_file));
        }
    }

    dispatch.apply()?;
    Ok(())
}

async fn process_jobs(
    mut job_rx: mpsc::Receiver<ConversionJob>,
    health_server: Arc<health::server::HealthServer>,
    error_logger: Arc<ErrorLogger>,
    disk_config: crate::config::global::DiskSpaceConfig,
    semaphore: Arc<Semaphore>,
    ffmpeg_path: String,
    ffprobe_path: String,
) {
    while let Some(job) = job_rx.recv().await {
        let hs = health_server.clone();
        let el = error_logger.clone();
        let dc = disk_config.clone();
        let sem = semaphore.clone();
        let ffmpeg = ffmpeg_path.clone();
        let ffprobe = ffprobe_path.clone();

        tokio::spawn(async move {
            let _permit = sem.acquire().await;
            match job.watch_type {
                config::watch::WatchType::Video { video } => {
                    for rule in &video {
                        processor::video::process_video(
                            job.watcher_name.clone(),
                            job.file_name.clone(),
                            job.file_path.clone(),
                            rule,
                            &job.output_folder,
                            &job.watch_folder,
                            el.clone(),
                            hs.clone(),
                            &dc,
                            &ffmpeg,
                            &ffprobe,
                        )
                        .await;
                    }
                }
                config::watch::WatchType::Image { image } => {
                    for rule in &image {
                        processor::image::process_image(
                            job.watcher_name.clone(),
                            job.file_name.clone(),
                            job.file_path.clone(),
                            rule,
                            &job.output_folder,
                            &job.watch_folder,
                            el.clone(),
                            hs.clone(),
                            &dc,
                        )
                        .await;
                    }
                }
                config::watch::WatchType::Audio { audio } => {
                    for rule in &audio {
                        processor::audio::process_audio(
                            job.watcher_name.clone(),
                            job.file_name.clone(),
                            job.file_path.clone(),
                            rule,
                            &job.output_folder,
                            &job.watch_folder,
                            el.clone(),
                            hs.clone(),
                            &dc,
                            &ffmpeg,
                        )
                        .await;
                    }
                }
                config::watch::WatchType::Pdf { pdf } => {
                    for rule in &pdf {
                        processor::pdf::process_pdf(
                            job.watcher_name.clone(),
                            job.file_name.clone(),
                            job.file_path.clone(),
                            rule,
                            &job.output_folder,
                            &job.watch_folder,
                            el.clone(),
                            hs.clone(),
                            &dc,
                        )
                        .await;
                    }
                }
                config::watch::WatchType::Document { document } => {
                    for rule in &document {
                        processor::document::process_document(
                            job.watcher_name.clone(),
                            job.file_name.clone(),
                            job.file_path.clone(),
                            rule,
                            &job.output_folder,
                            &job.watch_folder,
                            el.clone(),
                            hs.clone(),
                            &dc,
                        )
                        .await;
                    }
                }
                config::watch::WatchType::Custom { custom } => {
                    for rule in &custom {
                        processor::external::process_external(
                            job.watcher_name.clone(),
                            job.file_name.clone(),
                            job.file_path.clone(),
                            rule,
                            &job.output_folder,
                            &job.watch_folder,
                            el.clone(),
                            hs.clone(),
                            &dc,
                        )
                        .await;
                    }
                }
            }
        });
    }

    info!("Job processor shutting down");
}

async fn monitor_manager(
    mut reload_rx: mpsc::Receiver<Vec<WatchConfig>>,
    job_tx: mpsc::Sender<ConversionJob>,
    health_server: Arc<health::server::HealthServer>,
    global_config: &crate::config::global::GlobalConfig,
) {
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let mut current_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    while let Some(new_configs) = reload_rx.recv().await {
        let _ = shutdown_tx.send(());

        for handle in current_handles.drain(..) {
            let _ = handle.await;
        }

        for cfg in &new_configs {
            if let Err(e) = watcher::monitor::create_folders(cfg) {
                warn!("Failed to create folders: {}", e);
            }
            health_server.add_watcher_with_config(cfg).await;
        }

        let (new_shutdown_tx, _) = broadcast::channel::<()>(1);
        let check_interval = Duration::from_millis(global_config.file_check_interval_ms);
        let stable_time = Duration::from_millis(global_config.stable_time_ms);

        for cfg in new_configs {
            let tx = job_tx.clone();
            let name = format!("watcher_{}", cfg.name);
            let hs = health_server.clone();
            let shutdown_rx = new_shutdown_tx.subscribe();
            let cfg_clone = cfg.clone();
            let watch_folder = cfg_clone.watch_folder.clone();
            let gc = global_config.clone();

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
                )
                .await;
            });

            current_handles.push(handle);
        }

        info!("Monitors refreshed: {} watcher(s)", current_handles.len());
    }
}
