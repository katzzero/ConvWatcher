use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{error, info, warn};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{broadcast, mpsc, Mutex as TokioMutex};

use crate::config::embedded::EmbeddedConfig;
use crate::config::global::GlobalConfig;
use crate::config::watch::{WatchConfig, WatchType};
use crate::health::server::HealthServer;
use crate::processor::job::{ConversionJob, MatchedRule};

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
    processing_files: Arc<TokioMutex<HashSet<PathBuf>>>,
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
                    &processing_files,
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
    processing_files: &Arc<TokioMutex<HashSet<PathBuf>>>,
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

        if file_name.ends_with(".invalid") || file_name.ends_with(".done") || file_name.ends_with(".error") {
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
                    if processing_files.lock().await.contains(&path) {
                        continue;
                    }
                    if let Some(job) = create_job(&path, &file_name, config, watcher_name) {
                        let _ = health_server.enqueue(&file_name);
                        processing_files.lock().await.insert(path.clone());
                        if tx.send(job).await.is_err() {
                            error!("Failed to send job to worker pool");
                            processing_files.lock().await.remove(&path);
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

    let watchs_dir = PathBuf::from("config/watchs");
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

fn find_matching_rule(
    file_path: &PathBuf,
    _file_name: &str,
    watch_type: &WatchType,
) -> Option<MatchedRule> {
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
        let matched = match watch_type {
            WatchType::Video { rules } => rules.iter().find(|r| r.subfolder.as_deref() == Some(fmt)).map(|r| MatchedRule::Video(r.clone())),
            WatchType::Image { rules } => rules.iter().find(|r| r.subfolder.as_deref() == Some(fmt)).map(|r| MatchedRule::Image(r.clone())),
            WatchType::Audio { rules } => rules.iter().find(|r| r.subfolder.as_deref() == Some(fmt)).map(|r| MatchedRule::Audio(r.clone())),
            WatchType::Pdf { rules } => rules.iter().find(|r| r.subfolder.as_deref() == Some(fmt)).map(|r| MatchedRule::Pdf(r.clone())),
            WatchType::Document { rules } => rules.iter().find(|r| r.subfolder.as_deref() == Some(fmt)).map(|r| MatchedRule::Document(r.clone())),
            WatchType::Custom { rules } => rules.iter().find(|r| r.subfolder.as_deref() == Some(fmt)).map(|r| MatchedRule::Custom(r.clone())),
        };

        if matched.is_some() {
            return matched;
        }
    }

    match watch_type {
        WatchType::Video { rules } => rules.iter().find(|r| r.subfolder.is_none() && r.input_extensions.contains(&file_ext)).map(|r| MatchedRule::Video(r.clone())),
        WatchType::Image { rules } => rules.iter().find(|r| r.subfolder.is_none() && r.input_extensions.contains(&file_ext)).map(|r| MatchedRule::Image(r.clone())),
        WatchType::Audio { rules } => rules.iter().find(|r| r.subfolder.is_none() && r.input_extensions.contains(&file_ext)).map(|r| MatchedRule::Audio(r.clone())),
        WatchType::Pdf { rules } => rules.iter().find(|r| r.subfolder.is_none() && r.input_extensions.contains(&file_ext)).map(|r| MatchedRule::Pdf(r.clone())),
        WatchType::Document { rules } => rules.iter().find(|r| r.subfolder.is_none() && r.input_extensions.contains(&file_ext)).map(|r| MatchedRule::Document(r.clone())),
        WatchType::Custom { rules } => rules.iter().find(|r| r.subfolder.is_none() && r.input_extensions.contains(&file_ext)).map(|r| MatchedRule::Custom(r.clone())),
    }
}

fn create_job(
    file_path: &PathBuf,
    file_name: &str,
    watch_config: &WatchConfig,
    watcher_name: &str,
) -> Option<ConversionJob> {
    find_matching_rule(file_path, file_name, &watch_config.watch_type).map(|rule| {
        ConversionJob {
            watcher_name: watcher_name.to_string(),
            file_name: file_name.to_string(),
            file_path: file_path.clone(),
            matched_rule: rule,
            output_folder: watch_config.output_folder.clone(),
            watch_folder: watch_config.watch_folder.clone(),
        }
    })
}

pub fn create_folders(watch_config: &WatchConfig) -> anyhow::Result<()> {
    std::fs::create_dir_all(&watch_config.watch_folder)?;
    std::fs::create_dir_all(&watch_config.output_folder)?;

    // Create declared subfolder directories
    for sf in &watch_config.subfolders {
        let sub = PathBuf::from(&watch_config.watch_folder).join(format!("->{}", sf.name));
        std::fs::create_dir_all(&sub)?;
    }

    // Also create any subfolders referenced by rules but not declared
    let declared_names: std::collections::HashSet<&str> =
        watch_config.subfolders.iter().map(|s| s.name.as_str()).collect();

    let rule_subfolders: Vec<&str> = match &watch_config.watch_type {
        WatchType::Video { rules } => rules.iter().filter_map(|r| r.subfolder.as_deref()).collect(),
        WatchType::Image { rules } => rules.iter().filter_map(|r| r.subfolder.as_deref()).collect(),
        WatchType::Audio { rules } => rules.iter().filter_map(|r| r.subfolder.as_deref()).collect(),
        WatchType::Pdf { rules } => rules.iter().filter_map(|r| r.subfolder.as_deref()).collect(),
        WatchType::Document { rules } => rules.iter().filter_map(|r| r.subfolder.as_deref()).collect(),
        WatchType::Custom { rules } => rules.iter().filter_map(|r| r.subfolder.as_deref()).collect(),
    };

    for sf_name in rule_subfolders {
        if !declared_names.contains(sf_name) {
            let sub = PathBuf::from(&watch_config.watch_folder).join(format!("->{}", sf_name));
            std::fs::create_dir_all(&sub)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::watch::{VideoRule, WatchConfig, WatchType};

    fn test_video_config() -> WatchConfig {
        WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test/".to_string(),
            output_folder: "/app/outputs/test/".to_string(),
            watch_type: WatchType::Video {
                rules: vec![
                    VideoRule {
                        preset: "libx264".to_string(),
                        subfolder: None,
                        input_extensions: vec![".mp4".into(), ".mxf".into()],
                        output_ext: None, codec: None, quality: None,
                        audio_codec: None, audio_bitrate: None,
                        output_name: None, check_duration: None,
                        min_duration_ratio: None,
                    },
                    VideoRule {
                        preset: "h264_nvenc".to_string(),
                        subfolder: Some("gpu".to_string()),
                        input_extensions: vec![".mxf".into()],
                        output_ext: None, codec: None, quality: None,
                        audio_codec: None, audio_bitrate: None,
                        output_name: None, check_duration: None,
                        min_duration_ratio: None,
                    },
                ],
            },
        }
    }

    #[test]
    fn test_create_job_matches_by_extension() {
        let config = test_video_config();
        let file_path = PathBuf::from("/app/inputs/test/clip.mp4");
        let job = create_job(&file_path, "clip.mp4", &config, "watcher_test");
        assert!(job.is_some());
        let job = job.unwrap();
        assert_eq!(job.file_name, "clip.mp4");
    }

    #[test]
    fn test_create_job_matches_by_subfolder_format() {
        let config = test_video_config();
        let file_path = PathBuf::from("/app/inputs/test/->gpu/broadcast.mxf");
        let job = create_job(&file_path, "broadcast.mxf", &config, "watcher_test");
        assert!(job.is_some());
    }

    #[test]
    fn test_create_job_no_match_unknown_extension() {
        let config = test_video_config();
        let file_path = PathBuf::from("/app/inputs/test/clip.xyz");
        let job = create_job(&file_path, "clip.xyz", &config, "watcher_test");
        assert!(job.is_none());
    }

    #[test]
    fn test_create_job_no_match_wrong_subfolder() {
        // File in ->unknown/ subfolder but extension .xyz doesn't match any rule
        // Subfolder format doesn't match, extension doesn't match → no job
        let config = test_video_config();
        let file_path = PathBuf::from("/app/inputs/test/->unknown/clip.xyz");
        let job = create_job(&file_path, "clip.xyz", &config, "watcher_test");
        assert!(job.is_none());
    }

    #[test]
    fn test_create_folders_creates_subfolder_formats() {
        let temp_dir = std::env::temp_dir().join(format!("cw_test_{}", std::process::id()));
        let config = WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: temp_dir.join("inputs").to_string_lossy().to_string() + "/",
            output_folder: temp_dir.join("outputs").to_string_lossy().to_string() + "/",
            watch_type: WatchType::Video {
                rules: vec![
                    VideoRule {
                        preset: "libx264".to_string(),
                        subfolder: Some("h264".to_string()),
                        input_extensions: vec![".mp4".into()],
                        output_ext: None, codec: None, quality: None,
                        audio_codec: None, audio_bitrate: None,
                        output_name: None, check_duration: None,
                        min_duration_ratio: None,
                    },
                    VideoRule {
                        preset: "libx265".to_string(),
                        subfolder: Some("h265".to_string()),
                        input_extensions: vec![".mp4".into()],
                        output_ext: None, codec: None, quality: None,
                        audio_codec: None, audio_bitrate: None,
                        output_name: None, check_duration: None,
                        min_duration_ratio: None,
                    },
                ],
            },
        };

        let result = create_folders(&config);
        assert!(result.is_ok());
        assert!(PathBuf::from(&config.watch_folder).exists());
        assert!(PathBuf::from(&config.output_folder).exists());
        assert!(PathBuf::from(&config.watch_folder).join("->h264").exists());
        assert!(PathBuf::from(&config.watch_folder).join("->h265").exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
