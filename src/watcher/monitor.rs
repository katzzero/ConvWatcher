use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use log::{error, info, warn};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use subtle::ConstantTimeEq;
use tokio::sync::{broadcast, mpsc};

use crate::config::embedded::EmbeddedConfig;
use crate::config::global::GlobalConfig;
use crate::config::watch::{
    AudioRule, CustomRule, DocumentRule, ImageRule, PdfRule, VideoRule, WatchConfig, WatchType,
};
use crate::health::server::HealthServer;
use crate::processor::job::{ConversionJob, MatchedRule};

/// Trait for types that have rule-based matching with subfolder and extension support.
/// Implemented by all rule types to enable generic matching logic.
trait RuleMatcher {
    /// Returns the subfolder name if this rule is for a specific subfolder, or None for root folder rules.
    fn subfolder(&self) -> Option<&str>;
    /// Returns the list of input extensions this rule accepts.
    fn input_extensions(&self) -> &[String];
    /// Converts this rule into a MatchedRule variant.
    fn to_matched_rule(&self) -> MatchedRule;
}

impl RuleMatcher for VideoRule {
    fn subfolder(&self) -> Option<&str> {
        self.subfolder.as_deref()
    }
    fn input_extensions(&self) -> &[String] {
        &self.input_extensions
    }
    fn to_matched_rule(&self) -> MatchedRule {
        MatchedRule::Video(self.clone())
    }
}

impl RuleMatcher for ImageRule {
    fn subfolder(&self) -> Option<&str> {
        self.subfolder.as_deref()
    }
    fn input_extensions(&self) -> &[String] {
        &self.input_extensions
    }
    fn to_matched_rule(&self) -> MatchedRule {
        MatchedRule::Image(self.clone())
    }
}

impl RuleMatcher for AudioRule {
    fn subfolder(&self) -> Option<&str> {
        self.subfolder.as_deref()
    }
    fn input_extensions(&self) -> &[String] {
        &self.input_extensions
    }
    fn to_matched_rule(&self) -> MatchedRule {
        MatchedRule::Audio(self.clone())
    }
}

impl RuleMatcher for PdfRule {
    fn subfolder(&self) -> Option<&str> {
        self.subfolder.as_deref()
    }
    fn input_extensions(&self) -> &[String] {
        &self.input_extensions
    }
    fn to_matched_rule(&self) -> MatchedRule {
        MatchedRule::Pdf(self.clone())
    }
}

impl RuleMatcher for DocumentRule {
    fn subfolder(&self) -> Option<&str> {
        self.subfolder.as_deref()
    }
    fn input_extensions(&self) -> &[String] {
        &self.input_extensions
    }
    fn to_matched_rule(&self) -> MatchedRule {
        MatchedRule::Document(self.clone())
    }
}

impl RuleMatcher for CustomRule {
    fn subfolder(&self) -> Option<&str> {
        self.subfolder.as_deref()
    }
    fn input_extensions(&self) -> &[String] {
        &self.input_extensions
    }
    fn to_matched_rule(&self) -> MatchedRule {
        MatchedRule::Custom(self.clone())
    }
}

/// Type alias for the file stability state machine.
/// `(first_seen: Instant, last_size: u64)` maps to a (file_path → FileState) in
/// `file_states`. The semantics are:
///
/// - When we first see or re-scan a file, we set `first_seen` to the current instant
///   and record `last_size` as the file size at that moment.
/// - If the file's size changes mid-write, we reset `first_seen` to now with the
///   new `last_size`, restarting the stability timer.
/// - When the elapsed time since `first_seen` exceeds `stable_time` (default 5s)
///   and no size change has occurred, the file is "stable" — safe to begin
///   conversion because the writer appears to have finished.
type FileState = (Instant, u64);
#[allow(clippy::too_many_arguments)]
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
    processing_files: Arc<std::sync::Mutex<HashSet<PathBuf>>>,
    config_dir: &Path,
) {
    info!("Starting monitor for: {}", watch_folder);

    let watch_path = PathBuf::from(watch_folder);

    if let Err(e) = create_folders(&current_config) {
        error!("Failed to create folders for {}: {}", watch_folder, e);
        return;
    }

    let mut file_states: HashMap<PathBuf, FileState> = HashMap::new();
    // Signatures of files already enqueued with `input_file_action: none`.
    // Without this, the file stays on disk and would be converted forever.
    let mut processed_files: HashMap<PathBuf, (u64, Option<SystemTime>)> = HashMap::new();

    let (event_tx, mut event_rx) = mpsc::channel::<PathBuf>(100);

    let watch_dir = watch_path.clone();
    let event_tx_clone = event_tx.clone();
    let mut watcher: Option<RecommendedWatcher> =
        notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        for path in event.paths {
                            if path.is_file() && event_tx_clone.try_send(path).is_err() {
                                warn!("Notify event channel full; dropping event");
                            }
                        }
                    }
                    _ => {}
                }
            }
        })
        .map_err(|e| error!("Failed to create watcher: {}", e))
        .ok();

    if let Some(ref mut w) = watcher {
        // Non-recursive: subfolder files are picked up by the periodic
        // `collect_scan_files` pass, which also avoids symlink loops.
        if let Err(e) = w.watch(&watch_dir, RecursiveMode::NonRecursive) {
            error!(
                "Failed to watch {} with notify ({}); relying on periodic scan",
                watch_folder, e
            );
        }
    } else {
        error!(
            "Notify unavailable for {}; relying on periodic scan",
            watch_folder
        );
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
                        file_states.insert(file_path, (Instant::now(), size));
                    }
                }
            }

            _ = scan_ticker.tick() => {
                let _ = scan_directory(
                    &watch_path,
                    &mut file_states,
                    &mut processed_files,
                    stable_time,
                    &current_config,
                    &watcher_name_owned,
                    &tx,
                    &health_server_clone,
                    &global_config,
                    &processing_files,
                    config_dir,
                ).await;

                cleanup_stale_entries(&mut file_states, stable_time);
            }
        }
    }
}
#[allow(clippy::too_many_arguments)]
async fn scan_directory(
    watch_dir: &Path,
    file_states: &mut HashMap<PathBuf, FileState>,
    processed_files: &mut HashMap<PathBuf, (u64, Option<SystemTime>)>,
    stable_time: Duration,
    config: &WatchConfig,
    watcher_name: &str,
    tx: &mpsc::Sender<ConversionJob>,
    health_server: &Arc<HealthServer>,
    global_config: &GlobalConfig,
    processing_files: &Arc<std::sync::Mutex<HashSet<PathBuf>>>,
    config_dir: &Path,
) {
    let config_yaml_name = format!("{}.yaml", config.name);
    let config_yaml_path = watch_dir.join(&config_yaml_name);

    for path in collect_scan_files(watch_dir) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        if path == config_yaml_path {
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
                Some(&(first_seen, last_size)) => {
                    if current_size != last_size {
                        file_states.insert(path.clone(), (Instant::now(), current_size));
                    } else if first_seen.elapsed() >= stable_time {
                        validate_and_promote_config(&path, config, global_config, config_dir);
                        file_states.remove(&path);
                    }
                }
                None => {
                    file_states.insert(path.clone(), (Instant::now(), current_size));
                }
            }
            continue;
        }

        if file_name.ends_with(".invalid")
            || file_name.ends_with(".done")
            || file_name.ends_with(".error")
        {
            continue;
        }

        let metadata = match path.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let current_size = metadata.len();
        let current_mtime = metadata.modified().ok();

        // `input_file_action: none` leaves the source on disk. Remember its
        // signature so the next scan does not enqueue it again; if the file
        // changes (size or mtime), allow it to be processed once more.
        if matches!(
            global_config.input_file_action,
            crate::config::global::InputFileAction::None
        ) {
            if is_unchanged_processed(processed_files, &path, current_size, current_mtime) {
                file_states.remove(&path);
                continue;
            }
            processed_files.remove(&path);
        }

        match file_states.get(&path) {
            Some(&(first_seen, last_size)) => {
                if current_size != last_size {
                    // Reset timer on size change (AGENTS.md contract)
                    file_states.insert(path.clone(), (Instant::now(), current_size));
                } else if first_seen.elapsed() >= stable_time {
                    let already_processing = {
                        let mut pf = processing_files.lock().unwrap_or_else(|p| p.into_inner());
                        if pf.contains(&path) {
                            true
                        } else {
                            pf.insert(path.clone());
                            false
                        }
                    };
                    if already_processing {
                        file_states.remove(&path);
                        continue;
                    }
                    if !path.exists() {
                        processing_files
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .remove(&path);
                        file_states.remove(&path);
                        continue;
                    }
                    if let Some(job) = create_job(
                        &path,
                        &file_name,
                        config,
                        watcher_name,
                        global_config.input_file_action.clone(),
                    ) {
                        // BACKPRESSURE-01: use timeout when sending job
                        match tokio::time::timeout(Duration::from_secs(5), tx.send(job)).await {
                            Ok(Ok(_)) => {
                                if matches!(
                                    global_config.input_file_action,
                                    crate::config::global::InputFileAction::None
                                ) {
                                    processed_files
                                        .insert(path.clone(), (current_size, current_mtime));
                                }
                                let _ = health_server.enqueue(watcher_name, &file_name);
                            }
                            Ok(Err(_)) => {
                                error!("Failed to send job to job queue");
                                processing_files
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .remove(&path);
                                file_states.remove(&path);
                                continue;
                            }
                            Err(_) => {
                                // Timeout sending job - backpressure issue
                                warn!("Timeout sending job for {} - dropping", file_name);
                                processing_files
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .remove(&path);
                                file_states.remove(&path);
                                continue;
                            }
                        }
                    } else {
                        processing_files
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .remove(&path);
                    }
                    file_states.remove(&path);
                }
            }
            None => {
                file_states.insert(path.clone(), (Instant::now(), current_size));
            }
        }
    }

    processed_files.retain(|path, _| path.exists());
}

fn is_unchanged_processed(
    processed: &HashMap<PathBuf, (u64, Option<SystemTime>)>,
    path: &Path,
    size: u64,
    mtime: Option<SystemTime>,
) -> bool {
    processed
        .get(path)
        .is_some_and(|&(last_size, last_mtime)| last_size == size && last_mtime == mtime)
}

/// Files at the watch-folder root plus files inside `->name/` subfolders.
/// Only immediate `->*` directories are scanned; arbitrary nested directories
/// are ignored to keep the documented subfolder semantics predictable.
fn collect_scan_files(watch_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(watch_dir) {
        Ok(entries) => entries,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if file_type.is_file() {
            files.push(path);
        } else if file_type.is_dir() && entry.file_name().to_string_lossy().starts_with("->") {
            if let Ok(sub_entries) = std::fs::read_dir(&path) {
                for sub_entry in sub_entries.flatten() {
                    if sub_entry.file_type().is_ok_and(|ft| ft.is_file()) {
                        files.push(sub_entry.path());
                    }
                }
            }
        }
    }

    files
}

fn validate_and_promote_config(
    config_path: &Path,
    manifest: &WatchConfig,
    global: &GlobalConfig,
    config_dir: &Path,
) {
    // Reject names with path separators or directory traversal.
    let name = &manifest.name;
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        warn!("Invalid config name '{name}' — rejected (path traversal)");
        create_invalid_marker(config_path, manifest);
        return;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
    {
        warn!("Invalid config name '{name}' — rejected (invalid characters)");
        create_invalid_marker(config_path, manifest);
        return;
    }

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

    if !embedded.output_folder.is_empty() {
        if let Err(e) =
            crate::config::validate_absolute_path(&embedded.output_folder, "override output_folder")
        {
            warn!("Invalid override {:?}: {}", config_path, e);
            create_invalid_marker(config_path, manifest);
            return;
        }
    }

    if global.embedded_secret.is_empty() {
        warn!(
            "SECURITY: rejecting override {:?} from watch folder — \
            embedded_secret is empty (secret validation required). \
            Use config/watchs/ for manual admin overrides.",
            config_path
        );
        create_invalid_marker(config_path, manifest);
        return;
    }

    let secrets_match = bool::from(
        global
            .embedded_secret
            .as_bytes()
            .ct_eq(embedded.secret.as_bytes()),
    );
    if !secrets_match {
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

    // WATCHS-01: use config_dir parameter for watchs directory
    let watchs_dir = config_dir.join("watchs");
    if let Err(e) = std::fs::create_dir_all(&watchs_dir) {
        warn!("Failed to create watchs dir: {}", e);
        return;
    }

    let dest = watchs_dir.join(format!("{}.yaml", manifest.name));

    // COPY-01: preserve existing destination with backup before copy
    if dest.exists() {
        let backup_path = dest.with_extension("yaml.prev");
        if let Err(e) = std::fs::rename(&dest, &backup_path) {
            warn!("Failed to backup existing config {:?}: {}", dest, e);
        }
    }

    if let Err(e) = std::fs::copy(config_path, &dest) {
        warn!("Failed to promote config to {:?}: {}", dest, e);
        return;
    }

    let old_path = config_path.with_extension("yaml.old");
    let _ = std::fs::rename(config_path, &old_path);

    let watch_dir = config_path.parent().unwrap_or(Path::new("."));
    let invalid_path = watch_dir.join(format!("{}.invalid", manifest.name));
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

fn find_matching_rule(file_path: &Path, watch_type: &WatchType) -> Option<MatchedRule> {
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

    // Helper function to find matching rule using the RuleMatcher trait
    fn find_rule<R: RuleMatcher>(
        rules: &[R],
        subfolder_format: &Option<String>,
        file_ext: &str,
    ) -> Option<MatchedRule> {
        if let Some(ref fmt) = subfolder_format {
            // Subfolder rules must also match the file extension (03 §H7).
            let matched = rules
                .iter()
                .find(|r| {
                    r.subfolder().is_some_and(|s| s.eq_ignore_ascii_case(fmt))
                        && r.input_extensions().contains(&file_ext.to_string())
                })
                .map(|r| r.to_matched_rule());

            if matched.is_some() {
                return matched;
            }
        }

        rules
            .iter()
            .find(|r| {
                r.subfolder().is_none() && r.input_extensions().contains(&file_ext.to_string())
            })
            .map(|r| r.to_matched_rule())
    }

    match watch_type {
        WatchType::Video { rules } => find_rule(rules, &subfolder_format, &file_ext),
        WatchType::Image { rules } => find_rule(rules, &subfolder_format, &file_ext),
        WatchType::Audio { rules } => find_rule(rules, &subfolder_format, &file_ext),
        WatchType::Pdf { rules } => find_rule(rules, &subfolder_format, &file_ext),
        WatchType::Document { rules } => find_rule(rules, &subfolder_format, &file_ext),
        WatchType::Custom { rules } => find_rule(rules, &subfolder_format, &file_ext),
    }
}

fn create_job(
    file_path: &Path,
    file_name: &str,
    watch_config: &WatchConfig,
    watcher_name: &str,
    input_file_action: crate::config::global::InputFileAction,
) -> Option<ConversionJob> {
    find_matching_rule(file_path, &watch_config.watch_type).map(|rule| ConversionJob {
        watcher_name: watcher_name.to_string(),
        file_name: file_name.to_string(),
        file_path: file_path.to_path_buf(),
        matched_rule: rule,
        output_folder: watch_config.output_folder.clone(),
        watch_folder: watch_config.watch_folder.clone(),
        input_file_action,
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
    let declared_names: std::collections::HashSet<&str> = watch_config
        .subfolders
        .iter()
        .map(|s| s.name.as_str())
        .collect();

    let rule_subfolders: Vec<&str> = rule_subfolder_names(&watch_config.watch_type);

    for sf_name in rule_subfolders {
        if !declared_names.contains(sf_name) {
            let sub = PathBuf::from(&watch_config.watch_folder).join(format!("->{}", sf_name));
            std::fs::create_dir_all(&sub)?;
        }
    }

    Ok(())
}

fn rule_subfolder_names(watch_type: &WatchType) -> Vec<&str> {
    match watch_type {
        WatchType::Video { rules } => rules.iter().filter_map(|r| r.subfolder()).collect(),
        WatchType::Image { rules } => rules.iter().filter_map(|r| r.subfolder()).collect(),
        WatchType::Audio { rules } => rules.iter().filter_map(|r| r.subfolder()).collect(),
        WatchType::Pdf { rules } => rules.iter().filter_map(|r| r.subfolder()).collect(),
        WatchType::Document { rules } => rules.iter().filter_map(|r| r.subfolder()).collect(),
        WatchType::Custom { rules } => rules.iter().filter_map(|r| r.subfolder()).collect(),
    }
}

fn cleanup_stale_entries(file_states: &mut HashMap<PathBuf, FileState>, stable_time: Duration) {
    let stale_threshold = stable_time.saturating_mul(10);
    file_states.retain(|path, (first_seen, _)| {
        if !path.exists() {
            return false;
        }
        first_seen.elapsed() < stale_threshold
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::watch::{
        AudioRule, CustomRule, DocumentRule, ImageRule, PdfRule, VideoRule, WatchConfig, WatchType,
    };

    pub(super) fn test_video_config() -> WatchConfig {
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
                        output_ext: None,
                        codec: None,
                        quality: None,
                        audio_codec: None,
                        audio_bitrate: None,
                        output_name: None,
                        check_duration: None,
                        min_duration_ratio: None,
                    },
                    VideoRule {
                        preset: "h264_nvenc".to_string(),
                        subfolder: Some("gpu".to_string()),
                        input_extensions: vec![".mxf".into()],
                        output_ext: None,
                        codec: None,
                        quality: None,
                        audio_codec: None,
                        audio_bitrate: None,
                        output_name: None,
                        check_duration: None,
                        min_duration_ratio: None,
                    },
                ],
            },
        }
    }

    pub(super) fn test_image_config() -> WatchConfig {
        WatchConfig {
            name: "test_image".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test_image/".to_string(),
            output_folder: "/app/outputs/test_image/".to_string(),
            watch_type: WatchType::Image {
                rules: vec![
                    ImageRule {
                        preset: "imagemagick".to_string(),
                        subfolder: None,
                        input_extensions: vec![".png".into(), ".jpg".into()],
                        output_ext: None,
                        quality: None,
                        transparent: None,
                        output_name: None,
                    },
                    ImageRule {
                        preset: "imagemagick_thumb".to_string(),
                        subfolder: Some("thumb".to_string()),
                        input_extensions: vec![".png".into()],
                        output_ext: None,
                        quality: Some(150),
                        transparent: None,
                        output_name: None,
                    },
                ],
            },
        }
    }

    pub(super) fn test_audio_config() -> WatchConfig {
        WatchConfig {
            name: "test_audio".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test_audio/".to_string(),
            output_folder: "/app/outputs/test_audio/".to_string(),
            watch_type: WatchType::Audio {
                rules: vec![AudioRule {
                    preset: "ffmpeg".to_string(),
                    subfolder: None,
                    input_extensions: vec![".wav".into(), ".flac".into()],
                    output_ext: Some(".mp3".to_string()),
                    audio_codec: None,
                    audio_bitrate: None,
                    sample_rate: None,
                    channels: None,
                    output_name: None,
                }],
            },
        }
    }

    pub(super) fn test_pdf_config() -> WatchConfig {
        WatchConfig {
            name: "test_pdf".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test_pdf/".to_string(),
            output_folder: "/app/outputs/test_pdf/".to_string(),
            watch_type: WatchType::Pdf {
                rules: vec![PdfRule {
                    preset: "gs".to_string(),
                    subfolder: None,
                    input_extensions: vec![".pdf".into()],
                    output_ext: None,
                    mode: None,
                    quality: None,
                    pdfa_version: None,
                    resolution: None,
                    password: None,
                    output_name: None,
                }],
            },
        }
    }

    pub(super) fn test_document_config() -> WatchConfig {
        WatchConfig {
            name: "test_document".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test_document/".to_string(),
            output_folder: "/app/outputs/test_document/".to_string(),
            watch_type: WatchType::Document {
                rules: vec![DocumentRule {
                    preset: "libreoffice".to_string(),
                    subfolder: None,
                    input_extensions: vec![".docx".into(), ".pptx".into()],
                    output_ext: None,
                    toc: None,
                    toc_depth: None,
                    css: None,
                    template: None,
                    standalone: None,
                    metadata: None,
                    pdf_engine: None,
                    options: None,
                    output_name: None,
                }],
            },
        }
    }

    pub(super) fn test_custom_config() -> WatchConfig {
        WatchConfig {
            name: "test_custom".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test_custom/".to_string(),
            output_folder: "/app/outputs/test_custom/".to_string(),
            watch_type: WatchType::Custom {
                rules: vec![CustomRule {
                    preset: "custom_script".to_string(),
                    subfolder: None,
                    input_extensions: vec![".xyz".into()],
                    output_ext: None,
                    command: Some("custom_converter".to_string()),
                    output_name: None,
                    description: None,
                }],
            },
        }
    }

    #[test]
    fn test_create_job_matches_by_extension() {
        let config = test_video_config();
        let file_path = PathBuf::from("/app/inputs/test/clip.mp4");
        let job = create_job(
            &file_path,
            "clip.mp4",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_some());
        let job = job.unwrap();
        assert_eq!(job.file_name, "clip.mp4");
    }

    #[test]
    fn test_create_job_matches_by_subfolder_format() {
        let config = test_video_config();
        let file_path = PathBuf::from("/app/inputs/test/->gpu/broadcast.mxf");
        let job = create_job(
            &file_path,
            "broadcast.mxf",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_some());
    }

    #[test]
    fn test_create_job_no_match_unknown_extension() {
        let config = test_video_config();
        let file_path = PathBuf::from("/app/inputs/test/clip.xyz");
        let job = create_job(
            &file_path,
            "clip.xyz",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_none());
    }

    #[test]
    fn test_create_job_no_match_wrong_subfolder() {
        // File in ->unknown/ subfolder but extension .xyz doesn't match any rule
        // Subfolder format doesn't match, extension doesn't match → no job
        let config = test_video_config();
        let file_path = PathBuf::from("/app/inputs/test/->unknown/clip.xyz");
        let job = create_job(
            &file_path,
            "clip.xyz",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_none());
    }

    #[test]
    fn test_create_job_no_match_known_subfolder_wrong_extension() {
        // File in ->gpu/ (known subfolder with a rule) but extension doesn't
        // match that rule's input_extensions → should NOT match (regression
        // test for bug 03 §H7 where subfolder-only matching would match any
        // extension in a known subfolder).
        let config = test_video_config();
        // ->gpu rule only accepts .mxf, but this is .txt
        let file_path = PathBuf::from("/app/inputs/test/->gpu/notes.txt");
        let job = create_job(
            &file_path,
            "notes.txt",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_none(), "should NOT match .txt in ->gpu subfolder");
    }

    // ========== Image type tests ==========
    #[test]
    fn test_image_matches_by_extension() {
        let config = test_image_config();
        let file_path = PathBuf::from("/app/inputs/test_image/photo.png");
        let job = create_job(
            &file_path,
            "photo.png",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_some(), "Image rule should match .png extension");
    }

    #[test]
    fn test_image_matches_by_subfolder() {
        let config = test_image_config();
        let file_path = PathBuf::from("/app/inputs/test_image/->thumb/screenshot.png");
        let job = create_job(
            &file_path,
            "screenshot.png",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_some(), "Image rule should match ->thumb subfolder");
    }

    #[test]
    fn test_image_case_insensitive_extension() {
        let config = test_image_config();
        let file_path = PathBuf::from("/app/inputs/test_image/photo.PNG");
        let job = create_job(
            &file_path,
            "photo.PNG",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(
            job.is_some(),
            "Image rule should match .PNG extension case-insensitively"
        );
    }

    // ========== Audio type tests ==========
    #[test]
    fn test_audio_matches_by_extension() {
        let config = test_audio_config();
        let file_path = PathBuf::from("/app/inputs/test_audio/recording.wav");
        let job = create_job(
            &file_path,
            "recording.wav",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_some(), "Audio rule should match .wav extension");
    }

    #[test]
    fn test_audio_matches_by_subfolder() {
        let config = WatchConfig {
            name: "test_audio_sub".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test_audio_sub/".to_string(),
            output_folder: "/app/outputs/test_audio_sub/".to_string(),
            watch_type: WatchType::Audio {
                rules: vec![AudioRule {
                    preset: "ffmpeg".to_string(),
                    subfolder: Some("compressed".to_string()),
                    input_extensions: vec![".flac".into()],
                    output_ext: Some(".mp3".to_string()),
                    audio_codec: None,
                    audio_bitrate: None,
                    sample_rate: None,
                    channels: None,
                    output_name: None,
                }],
            },
        };
        let file_path = PathBuf::from("/app/inputs/test_audio_sub/->compressed/track.flac");
        let job = create_job(
            &file_path,
            "track.flac",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(
            job.is_some(),
            "Audio rule should match ->compressed subfolder"
        );
    }

    // ========== Pdf type tests ==========
    #[test]
    fn test_pdf_matches_by_extension() {
        let config = test_pdf_config();
        let file_path = PathBuf::from("/app/inputs/test_pdf/document.pdf");
        let job = create_job(
            &file_path,
            "document.pdf",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_some(), "Pdf rule should match .pdf extension");
    }

    #[test]
    fn test_pdf_matches_by_subfolder() {
        let config = WatchConfig {
            name: "test_pdf_sub".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test_pdf_sub/".to_string(),
            output_folder: "/app/outputs/test_pdf_sub/".to_string(),
            watch_type: WatchType::Pdf {
                rules: vec![PdfRule {
                    preset: "gs".to_string(),
                    subfolder: Some("optimized".to_string()),
                    input_extensions: vec![".pdf".into()],
                    output_ext: None,
                    mode: Some(crate::config::watch::PdfMode::Compress),
                    quality: None,
                    pdfa_version: None,
                    resolution: None,
                    password: None,
                    output_name: None,
                }],
            },
        };
        let file_path = PathBuf::from("/app/inputs/test_pdf_sub/->optimized/report.pdf");
        let job = create_job(
            &file_path,
            "report.pdf",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_some(), "Pdf rule should match ->optimized subfolder");
    }

    // ========== Document type tests ==========
    #[test]
    fn test_document_matches_by_extension() {
        let config = test_document_config();
        let file_path = PathBuf::from("/app/inputs/test_document/report.docx");
        let job = create_job(
            &file_path,
            "report.docx",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_some(), "Document rule should match .docx extension");
    }

    #[test]
    fn test_document_matches_by_subfolder() {
        let config = WatchConfig {
            name: "test_document_sub".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test_document_sub/".to_string(),
            output_folder: "/app/outputs/test_document_sub/".to_string(),
            watch_type: WatchType::Document {
                rules: vec![DocumentRule {
                    preset: "libreoffice".to_string(),
                    subfolder: Some("export".to_string()),
                    input_extensions: vec![".pptx".into()],
                    output_ext: None,
                    toc: None,
                    toc_depth: None,
                    css: None,
                    template: None,
                    standalone: None,
                    metadata: None,
                    pdf_engine: None,
                    options: None,
                    output_name: None,
                }],
            },
        };
        let file_path = PathBuf::from("/app/inputs/test_document_sub/->export/slides.pptx");
        let job = create_job(
            &file_path,
            "slides.pptx",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(
            job.is_some(),
            "Document rule should match ->export subfolder"
        );
    }

    // ========== Custom type tests ==========
    #[test]
    fn test_custom_matches_by_extension() {
        let config = test_custom_config();
        let file_path = PathBuf::from("/app/inputs/test_custom/data.xyz");
        let job = create_job(
            &file_path,
            "data.xyz",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_some(), "Custom rule should match .xyz extension");
    }

    #[test]
    fn test_custom_matches_by_subfolder() {
        let config = WatchConfig {
            name: "test_custom_sub".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test_custom_sub/".to_string(),
            output_folder: "/app/outputs/test_custom_sub/".to_string(),
            watch_type: WatchType::Custom {
                rules: vec![CustomRule {
                    preset: "custom_script".to_string(),
                    subfolder: Some("special".to_string()),
                    input_extensions: vec![".xyz".into()],
                    output_ext: None,
                    command: Some("special_converter".to_string()),
                    output_name: None,
                    description: None,
                }],
            },
        };
        let file_path = PathBuf::from("/app/inputs/test_custom_sub/->special/data.xyz");
        let job = create_job(
            &file_path,
            "data.xyz",
            &config,
            "watcher_test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(
            job.is_some(),
            "Custom rule should match ->special subfolder"
        );
    }

    #[test]
    fn test_find_matching_rule_returns_correct_variant() {
        // Test that find_matching_rule returns the correct MatchedRule variant
        let video_config = test_video_config();
        let video_path = PathBuf::from("/app/inputs/test/clip.mp4");
        let video_rule = find_matching_rule(&video_path, &video_config.watch_type);
        assert!(matches!(video_rule, Some(MatchedRule::Video(_))));

        let image_config = test_image_config();
        let image_path = PathBuf::from("/app/inputs/test_image/photo.png");
        let image_rule = find_matching_rule(&image_path, &image_config.watch_type);
        assert!(matches!(image_rule, Some(MatchedRule::Image(_))));

        let audio_config = test_audio_config();
        let audio_path = PathBuf::from("/app/inputs/test_audio/recording.wav");
        let audio_rule = find_matching_rule(&audio_path, &audio_config.watch_type);
        assert!(matches!(audio_rule, Some(MatchedRule::Audio(_))));

        let pdf_config = test_pdf_config();
        let pdf_path = PathBuf::from("/app/inputs/test_pdf/document.pdf");
        let pdf_rule = find_matching_rule(&pdf_path, &pdf_config.watch_type);
        assert!(matches!(pdf_rule, Some(MatchedRule::Pdf(_))));

        let document_config = test_document_config();
        let document_path = PathBuf::from("/app/inputs/test_document/report.docx");
        let document_rule = find_matching_rule(&document_path, &document_config.watch_type);
        assert!(matches!(document_rule, Some(MatchedRule::Document(_))));

        let custom_config = test_custom_config();
        let custom_path = PathBuf::from("/app/inputs/test_custom/data.xyz");
        let custom_rule = find_matching_rule(&custom_path, &custom_config.watch_type);
        assert!(matches!(custom_rule, Some(MatchedRule::Custom(_))));
    }

    #[test]
    fn test_rule_subfolder_names_extracts_correctly() {
        // Test that rule_subfolder_names extracts subfolder names correctly
        let video_config = test_video_config();
        let video_subfolders = rule_subfolder_names(&video_config.watch_type);
        assert!(video_subfolders.contains(&"gpu"));

        let image_config = test_image_config();
        let image_subfolders = rule_subfolder_names(&image_config.watch_type);
        assert!(image_subfolders.contains(&"thumb"));

        let audio_config = test_audio_config();
        let audio_subfolders = rule_subfolder_names(&audio_config.watch_type);
        assert!(audio_subfolders.is_empty());

        let pdf_config = test_pdf_config();
        let pdf_subfolders = rule_subfolder_names(&pdf_config.watch_type);
        assert!(pdf_subfolders.is_empty());

        let document_config = test_document_config();
        let document_subfolders = rule_subfolder_names(&document_config.watch_type);
        assert!(document_subfolders.is_empty());

        let custom_config = test_custom_config();
        let custom_subfolders = rule_subfolder_names(&custom_config.watch_type);
        assert!(custom_subfolders.is_empty());
    }

    #[test]
    fn test_stability_timer_resets_on_size_change() {
        use std::time::Instant;

        let mut file_states: HashMap<PathBuf, FileState> = HashMap::new();
        let path = PathBuf::from("/tmp/test_stability.mp4");

        // First scan: file appears with 100 bytes
        let t0 = Instant::now();
        file_states.insert(path.clone(), (t0, 100));

        // Simulate a size change: file grows to 200 bytes
        let (first_seen, _) = file_states.get(&path).unwrap();
        let old_first_seen = *first_seen;
        file_states.insert(path.clone(), (Instant::now(), 200));

        // first_seen MUST have advanced (reset on size change)
        let (new_first_seen, new_size) = file_states.get(&path).unwrap();
        assert_eq!(*new_size, 200, "size should be updated");
        assert!(
            *new_first_seen > old_first_seen,
            "first_seen must reset on size change (old={:?}, new={:?})",
            old_first_seen,
            new_first_seen
        );

        // Now simulate a scan with no size change and a stable_time that
        // hasn't elapsed since the NEW first_seen → file should NOT be ready.
        let stable_time = Duration::from_secs(60);
        let elapsed = new_first_seen.elapsed();
        assert!(
            elapsed < stable_time,
            "immediately after reset, elapsed ({:?}) must be < stable_time ({:?})",
            elapsed,
            stable_time
        );
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
                        output_ext: None,
                        codec: None,
                        quality: None,
                        audio_codec: None,
                        audio_bitrate: None,
                        output_name: None,
                        check_duration: None,
                        min_duration_ratio: None,
                    },
                    VideoRule {
                        preset: "libx265".to_string(),
                        subfolder: Some("h265".to_string()),
                        input_extensions: vec![".mp4".into()],
                        output_ext: None,
                        codec: None,
                        quality: None,
                        audio_codec: None,
                        audio_bitrate: None,
                        output_name: None,
                        check_duration: None,
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

#[cfg(test)]
mod scan_tests {
    use super::*;

    #[test]
    fn collect_scan_files_includes_root_and_direct_subfolders() {
        let root = std::env::temp_dir().join(format!("cw_scan_{}", std::process::id()));
        let gpu = root.join("->GPU");
        let other = root.join("other");
        std::fs::create_dir_all(&gpu).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(root.join("root.mp4"), b"x").unwrap();
        std::fs::write(gpu.join("sub.mxf"), b"x").unwrap();
        std::fs::write(other.join("ignored.mp4"), b"x").unwrap();

        let files = collect_scan_files(&root);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();

        assert!(names.contains(&"root.mp4".to_string()));
        assert!(names.contains(&"sub.mxf".to_string()));
        assert!(!names.contains(&"ignored.mp4".to_string()));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn processed_signature_skips_only_unchanged_files() {
        let path = PathBuf::from("/tmp/action_none.mp4");
        let mut processed = HashMap::new();
        let mtime = Some(SystemTime::UNIX_EPOCH);
        processed.insert(path.clone(), (10, mtime));

        assert!(is_unchanged_processed(&processed, &path, 10, mtime));
        assert!(!is_unchanged_processed(&processed, &path, 11, mtime));
        assert!(!is_unchanged_processed(&processed, &path, 10, None));
    }

    #[cfg(unix)]
    #[test]
    fn collect_scan_files_skips_symlink_loops() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("cw_symlink_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("file.mp4"), b"x").unwrap();
        symlink(&root, root.join("loop")).unwrap();

        let files = collect_scan_files(&root);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["file.mp4".to_string()]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn subfolder_matching_is_case_insensitive() {
        let config = super::tests::test_video_config();
        let path = PathBuf::from("/app/inputs/test/->GPU/broadcast.mxf");
        let job = create_job(
            &path,
            "broadcast.mxf",
            &config,
            "test",
            crate::config::global::InputFileAction::Mark,
        );
        assert!(job.is_some(), "->GPU should match subfolder rule 'gpu'");
    }

    #[test]
    fn test_processing_files_tracking() {
        use std::sync::Arc;

        // Test that processing_files can be tracked and removed
        let processing_files: Arc<std::sync::Mutex<HashSet<PathBuf>>> =
            Arc::new(std::sync::Mutex::new(HashSet::new()));

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let path = PathBuf::from("/tmp/test.mp4");

            // Insert should succeed
            {
                let mut pf = processing_files.lock().unwrap_or_else(|p| p.into_inner());
                pf.insert(path.clone());
                assert!(pf.contains(&path));
            }

            // Remove should succeed
            {
                let mut pf = processing_files.lock().unwrap_or_else(|p| p.into_inner());
                pf.remove(&path);
                assert!(!pf.contains(&path));
            }
        });
    }
}
