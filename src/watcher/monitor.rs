use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{error, info, warn};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{broadcast, mpsc};

use crate::config::embedded::EmbeddedConfig;
use crate::config::global::GlobalConfig;
use crate::config::watch::{WatchConfig, WatchType};
use crate::health::server::HealthServer;
use crate::processor::job::ConversionJob;

type FileState = (Instant, u64, u64);

pub async fn run_file_monitor(
    watch_folder: &str,
    tx: mpsc::Sender<ConversionJob>,
    check_interval: Duration,
    stable_time: Duration,
    watcher_name: &str,
    health_server: Arc<HealthServer>,
    mut shutdown_rx: broadcast::Receiver<()>,
    current_config: WatchConfig,
    global_config: GlobalConfig,
) {
    info!("Starting monitor for: {}", watch_folder);

    let watch_path = PathBuf::from(watch_folder);

    if let Err(e) = create_folders(&current_config) {
        error!("Failed to create folders for {}: {}", watch_folder, e);
        return;
    }

    let mut file_states: HashMap<PathBuf, FileState> = HashMap::new();

    let (event_tx, mut event_rx) = mpsc::channel::<PathBuf>(100);

    let watch_dir = watch_path.clone();
    let event_tx_clone = event_tx.clone();
    let mut watcher: RecommendedWatcher = match notify::recommended_watcher(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        for path in event.paths {
                            if path.is_file() {
                                let _ = event_tx_clone.blocking_send(path);
                            }
                        }
                    }
                    _ => {}
                }
            }
        },
    )
    .map_err(|e| error!("Failed to create watcher: {}", e))
    .ok()
    {
        Some(w) => w,
        None => return,
    };

    if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::Recursive) {
        error!("Failed to watch {}: {}", watch_folder, e);
        return;
    }

    let mut scan_ticker = tokio::time::interval(check_interval);

    let health_server_clone = health_server.clone();
    let watcher_name_owned = watcher_name.to_string();

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutting down monitor for: {}", watch_folder);
                break;
            }

            Some(file_path) = event_rx.recv() => {
                if file_path.is_file() {
                    if let Ok(metadata) = std::fs::metadata(&file_path) {
                        let size = metadata.len();
                        file_states.insert(file_path, (Instant::now(), size, size));
                    }
                }
            }

            _ = scan_ticker.tick() => {
                let _ = scan_directory(
                    &watch_path,
                    &mut file_states,
                    stable_time,
                    &current_config,
                    &watcher_name_owned,
                    &tx,
                    &health_server_clone,
                    &global_config,
                ).await;

                cleanup_stale_entries(&mut file_states, stable_time);
            }
        }
    }
}

async fn scan_directory(
    watch_dir: &Path,
    file_states: &mut HashMap<PathBuf, FileState>,
    stable_time: Duration,
    config: &WatchConfig,
    watcher_name: &str,
    tx: &mpsc::Sender<ConversionJob>,
    health_server: &Arc<HealthServer>,
    global_config: &GlobalConfig,
) {
    let entries = match std::fs::read_dir(watch_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let config_yaml_name = format!("{}.yaml", config.name);

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if file_name == config_yaml_name {
            let invalid_path = watch_dir.join(format!("{}.invalid", config.name));
            if invalid_path.exists() {
                continue;
            }

            let metadata = match path.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let current_size = metadata.len();

            match file_states.get(&path) {
                Some(&(first_seen, last_size, _)) => {
                    if current_size != last_size {
                        file_states.insert(path.clone(), (first_seen, current_size, current_size));
                    } else if first_seen.elapsed() >= stable_time {
                        validate_and_promote_config(
                            &path,
                            config,
                            global_config,
                        );
                        file_states.remove(&path);
                    }
                }
                None => {
                    file_states
                        .insert(path.clone(), (Instant::now(), current_size, current_size));
                }
            }
            continue;
        }

        if file_name.ends_with(".invalid") {
            continue;
        }

        let metadata = match path.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let current_size = metadata.len();

        match file_states.get(&path) {
            Some(&(first_seen, last_size, _)) => {
                if current_size != last_size {
                    file_states.insert(path.clone(), (first_seen, current_size, current_size));
                } else if first_seen.elapsed() >= stable_time {
                    if let Some(job) = create_job(&path, &file_name, config, watcher_name) {
                        let _ = health_server.enqueue(&file_name);
                        if tx.send(job).await.is_err() {
                            error!("Failed to send job to worker pool");
                        }
                    }
                    file_states.remove(&path);
                }
            }
            None => {
                file_states.insert(path.clone(), (Instant::now(), current_size, current_size));
            }
        }
    }
}

fn validate_and_promote_config(
    config_path: &Path,
    manifest: &WatchConfig,
    global: &GlobalConfig,
) {
    let content = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to read config file {:?}: {}", config_path, e);
            return;
        }
    };

    let embedded: EmbeddedConfig = match serde_yaml::from_str(&content) {
        Ok(e) => e,
        Err(e) => {
            warn!("Failed to parse config {:?}: {}", config_path, e);
            create_invalid_marker(config_path, manifest);
            return;
        }
    };

    if !global.embedded_secret.is_empty() && embedded.secret != global.embedded_secret {
        warn!("Secret mismatch in {:?}", config_path);
        create_invalid_marker(config_path, manifest);
        return;
    }

    let expected_type = manifest.watch_type.type_name();
    let got_type = embedded.watch_type.type_name();
    if expected_type != got_type {
        warn!(
            "Type mismatch in {:?}: expected '{}' but got '{}'",
            config_path, expected_type, got_type
        );
        create_invalid_marker(config_path, manifest);
        return;
    }

    let watchs_dir = PathBuf::from(&global.watchs_dir);
    if let Err(e) = std::fs::create_dir_all(&watchs_dir) {
        warn!("Failed to create watchs dir: {}", e);
        return;
    }

    let dest = watchs_dir.join(format!("{}.yaml", manifest.name));
    if let Err(e) = std::fs::copy(config_path, &dest) {
        warn!("Failed to promote config to {:?}: {}", dest, e);
        return;
    }

    let old_path = config_path.with_extension("yaml.old");
    let _ = std::fs::rename(config_path, &old_path);

    let invalid_path = config_path.with_extension("yaml.invalid");
    let _ = std::fs::remove_file(&invalid_path);

    info!(
        "Config promoted: {:?} -> {:?} (backup: {:?})",
        config_path, dest, old_path
    );
}

fn create_invalid_marker(config_path: &Path, manifest: &WatchConfig) {
    let watch_dir = config_path.parent().unwrap_or(Path::new("."));
    let invalid_path = watch_dir.join(format!("{}.invalid", manifest.name));

    if let Err(e) = std::fs::write(&invalid_path, "") {
        warn!("Failed to create invalid marker {:?}: {}", invalid_path, e);
    } else {
        warn!(
            "Config {:?} is invalid. Marker created at {:?}",
            config_path, invalid_path
        );
    }
}

fn create_job(
    file_path: &PathBuf,
    file_name: &str,
    watch_config: &WatchConfig,
    watcher_name: &str,
) -> Option<ConversionJob> {
    let file_ext = file_path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();

    let subfolder_format = file_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .filter(|s| s.starts_with("->"))
        .map(|s| s[2..].to_lowercase());

    if let Some(ref fmt) = subfolder_format {
        let matched = match &watch_config.watch_type {
            WatchType::Video { video } => video.iter().find(|r| r.format.as_deref() == Some(fmt)).is_some(),
            WatchType::Image { image } => image.iter().find(|r| r.format.as_deref() == Some(fmt)).is_some(),
            WatchType::Audio { audio } => audio.iter().find(|r| r.format.as_deref() == Some(fmt)).is_some(),
            WatchType::Pdf { pdf } => pdf.iter().find(|r| r.format.as_deref() == Some(fmt)).is_some(),
            WatchType::Document { document } => document.iter().find(|r| r.format.as_deref() == Some(fmt)).is_some(),
            WatchType::Custom { custom } => custom.iter().find(|r| r.format.as_deref() == Some(fmt)).is_some(),
        };

        if matched {
            return Some(ConversionJob {
                watcher_name: watcher_name.to_string(),
                file_name: file_name.to_string(),
                file_path: file_path.clone(),
                watch_type: watch_config.watch_type.clone(),
                output_folder: watch_config.output_folder.clone(),
                watch_folder: watch_config.watch_folder.clone(),
            });
        }
    }

    let ext_matched = match &watch_config.watch_type {
        WatchType::Video { video } => {
            video.iter().any(|r| r.format.is_none() && r.input_extensions.contains(&file_ext))
        }
        WatchType::Image { image } => {
            image.iter().any(|r| r.format.is_none() && r.input_extensions.contains(&file_ext))
        }
        WatchType::Audio { audio } => {
            audio.iter().any(|r| r.format.is_none() && r.input_extensions.contains(&file_ext))
        }
        WatchType::Pdf { pdf } => {
            pdf.iter().any(|r| r.format.is_none() && r.input_extensions.contains(&file_ext))
        }
        WatchType::Document { document } => {
            document.iter().any(|r| r.format.is_none() && r.input_extensions.contains(&file_ext))
        }
        WatchType::Custom { custom } => {
            custom.iter().any(|r| r.format.is_none() && r.input_extensions.contains(&file_ext))
        }
    };

    if ext_matched {
        Some(ConversionJob {
            watcher_name: watcher_name.to_string(),
            file_name: file_name.to_string(),
            file_path: file_path.clone(),
            watch_type: watch_config.watch_type.clone(),
            output_folder: watch_config.output_folder.clone(),
            watch_folder: watch_config.watch_folder.clone(),
        })
    } else {
        None
    }
}

pub fn create_folders(watch_config: &WatchConfig) -> anyhow::Result<()> {
    std::fs::create_dir_all(&watch_config.watch_folder)?;
    std::fs::create_dir_all(&watch_config.output_folder)?;

    match &watch_config.watch_type {
        WatchType::Video { video } => {
            for rule in video {
                if let Some(ref fmt) = rule.format {
                    let sub =
                        PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                    std::fs::create_dir_all(&sub)?;
                }
            }
        }
        WatchType::Image { image } => {
            for rule in image {
                if let Some(ref fmt) = rule.format {
                    let sub =
                        PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                    std::fs::create_dir_all(&sub)?;
                }
            }
        }
        WatchType::Audio { audio } => {
            for rule in audio {
                if let Some(ref fmt) = rule.format {
                    let sub =
                        PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                    std::fs::create_dir_all(&sub)?;
                }
            }
        }
        WatchType::Pdf { pdf } => {
            for rule in pdf {
                if let Some(ref fmt) = rule.format {
                    let sub =
                        PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                    std::fs::create_dir_all(&sub)?;
                }
            }
        }
        WatchType::Document { document } => {
            for rule in document {
                if let Some(ref fmt) = rule.format {
                    let sub =
                        PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                    std::fs::create_dir_all(&sub)?;
                }
            }
        }
        WatchType::Custom { custom } => {
            for rule in custom {
                if let Some(ref fmt) = rule.format {
                    let sub =
                        PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                    std::fs::create_dir_all(&sub)?;
                }
            }
        }
    }

    Ok(())
}

fn cleanup_stale_entries(file_states: &mut HashMap<PathBuf, FileState>, stable_time: Duration) {
    let stale_threshold = stable_time * 10;
    file_states.retain(|path, (first_seen, _, _)| {
        if !path.exists() {
            return false;
        }
        first_seen.elapsed() < stale_threshold
    });
}
