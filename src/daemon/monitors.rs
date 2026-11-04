use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{info, warn};
use tokio::sync::{broadcast, mpsc};

use crate::config::global::GlobalConfig;
use crate::config::watch::WatchConfig;
use crate::health::server::HealthServer;
use crate::processor::context::DEFAULT_TIMEOUT_S;
use crate::watcher::monitor;

#[allow(clippy::too_many_arguments)]
fn spawn_monitors(
    configs: Vec<WatchConfig>,
    job_tx: &mpsc::Sender<crate::processor::job::ConversionJob>,
    health_server: &Arc<HealthServer>,
    global_config: &GlobalConfig,
    processing_files: &Arc<std::sync::Mutex<HashSet<PathBuf>>>,
    shutdown_tx: &broadcast::Sender<()>,
    config_dir: &Path,
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
        let config_dir = config_dir.to_path_buf();

        let handle = tokio::spawn(async move {
            monitor::run_file_monitor(
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
                &config_dir,
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

#[allow(clippy::too_many_arguments)]
pub async fn monitor_manager(
    initial_configs: Vec<WatchConfig>,
    mut reload_rx: mpsc::Receiver<Vec<WatchConfig>>,
    job_tx: mpsc::Sender<crate::processor::job::ConversionJob>,
    health_server: Arc<HealthServer>,
    global_config: GlobalConfig,
    processing_files: Arc<std::sync::Mutex<HashSet<PathBuf>>>,
    mut shutdown_rx: broadcast::Receiver<()>,
    shared_global: Arc<std::sync::RwLock<GlobalConfig>>,
    config_dir: PathBuf,
    timeout_secs: Arc<AtomicU64>,
) {
    let mut generation_tx = broadcast::channel::<()>(8).0;
    let mut current_global = global_config.clone();
    let mut current_handles = spawn_monitors(
        initial_configs,
        &job_tx,
        &health_server,
        &current_global,
        &processing_files,
        &generation_tx,
        &config_dir,
    );
    info!("Spawned {} file monitor(s)", current_handles.len());

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => break,
            maybe = reload_rx.recv() => {
                let Some(new_configs) = maybe else { break };

                // Pick up global.* changes applied by the reload task.
                current_global = shared_global
                    .read()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                let val = if current_global.timeout_s == 0 {
                    DEFAULT_TIMEOUT_S
                } else {
                    current_global.timeout_s
                };
                timeout_secs.store(val, Ordering::Relaxed);

                let _ = generation_tx.send(());
                shutdown_monitors(std::mem::take(&mut current_handles)).await;

                for cfg in &new_configs {
                    if let Err(e) = monitor::create_folders(cfg) {
                        warn!("Failed to create folders: {}", e);
                    }
                    health_server.add_watcher_with_config(cfg).await;
                }

                generation_tx = broadcast::channel::<()>(8).0;
                current_handles = spawn_monitors(
                    new_configs,
                    &job_tx,
                    &health_server,
                    &current_global,
                    &processing_files,
                    &generation_tx,
                    &config_dir,
                );

                info!("Monitors refreshed: {} watcher(s)", current_handles.len());
            }
        }
    }

    let _ = generation_tx.send(());
    shutdown_monitors(current_handles).await;
    info!("Monitor manager exited");
}
