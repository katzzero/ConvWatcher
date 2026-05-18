# ConvWatcher — Watcher System

## Module: `src/watcher/monitor.rs`

Core engine: filesystem monitoring, stability detection, and job creation.

### run_file_monitor()

```rust
/// Main per-watcher async task.
///
/// 1. Creates a `notify::RecommendedWatcher` on the watch folder
/// 2. Runs a periodic scan_directory() loop every check_interval
/// 3. Tracks file stability (size unchanged for stable_time)
/// 4. Creates ConversionJob for ready files and sends via mpsc
/// 5. Cleans up stale entries (failed downloads, temporary files)
/// 6. Watches for shutdown signal via broadcast receiver
pub async fn run_file_monitor(
    watch_folder: &str,
    tx: mpsc::Sender<ConversionJob>,
    check_interval: Duration,
    stable_time: Duration,
    watcher_name: &str,
    health_server: Arc<HealthServer>,
    shutdown_rx: &mut broadcast::Receiver<()>,
    current_config: WatchConfig,
)
```

### File Stability Tracking

```rust
// HashMap keyed by file path
// Value: (first_seen: Instant, last_checked_size: u64, initial_size: u64)
let mut file_states: HashMap<PathBuf, (Instant, u64, u64)> = HashMap::new();

// On notify Create/Modify event:
//   - Insert or update file_states entry
//   - Set last_checked_size = current file size

// On periodic scan:
//   - For files where current_size != last_checked_size:
//     → still growing, update last_checked_size
//   - For files where current_size == last_checked_size AND
//     age >= stable_time:
//     → READY for processing

// Cleanup:
//   - Remove entries if file no longer exists
//   - Remove entries older than stable_time * 10 (stale temp files)
```

### create_job() — Universal Matching

```rust
fn create_job(
    file_path: &PathBuf,
    file_name: &str,
    watch_config: &WatchConfig,
    watcher_name: &str,
) -> Option<ConversionJob> {
    // Get file extension (lowercase, with dot)
    let file_ext = file_path.extension()
        .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
        .unwrap_or_default();

    // Get subfolder name if file is in ->format/ subfolder
    let subfolder_format = file_path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .filter(|s| s.starts_with("->"))
        .map(|s| s[2..].to_lowercase());

    // ── STEP 1: Subfolder match ─────────────────────────
    if let Some(ref fmt) = subfolder_format {
        // Video (if video_watch enabled)
        if watch_config.video_watch {
            if let Some(rule) = watch_config.video_rules.iter().find(|r| r.format.as_deref() == Some(fmt)) {
                return Some(ConversionJob::Video { ... });
            }
        }
        // Audio (if audio_watch enabled)
        if watch_config.audio_watch {
            if let Some(rule) = watch_config.audio_rules.iter().find(|r| r.format.as_deref() == Some(fmt)) {
                return Some(ConversionJob::Audio { ... });
            }
        }
        // Document (if doc_watch enabled)
        if watch_config.doc_watch {
            if let Some(rule) = watch_config.document_rules.iter().find(|r| r.format.as_deref() == Some(fmt)) {
                return Some(ConversionJob::Document { ... });
            }
        }
        // PDF
        if let Some(rule) = watch_config.pdf_rules.iter().find(|r| r.format.as_deref() == Some(fmt)) {
            return Some(ConversionJob::Pdf { ... });
        }
        // Image (if image_watch enabled)
        if watch_config.image_watch {
            if let Some(rule) = watch_config.image_rules.iter().find(|r| r.format.as_deref() == Some(fmt)) {
                return Some(ConversionJob::Image { ... });
            }
        }
        // Custom (if custom_watch enabled)
        if watch_config.custom_watch {
            if let Some(rule) = watch_config.custom_rules.iter().find(|r| r.format.as_deref() == Some(fmt)) {
                return Some(ConversionJob::External { ... });
            }
        }
    }

    // ── STEP 2: Extension match ─────────────────────────
    // Custom rules (highest priority)
    for rule in &watch_config.custom_rules {
        if rule.format.is_none() && rule.input_extensions.contains(&file_ext) {
            return Some(ConversionJob::External { ... });
        }
    }

    // Video rules
    for rule in &watch_config.video_rules {
        if rule.format.is_none() && rule.input_extensions.contains(&file_ext) {
            return Some(ConversionJob::Video { ... });
        }
    }

    // Audio rules
    for rule in &watch_config.audio_rules {
        if rule.format.is_none() && rule.input_extensions.contains(&file_ext) {
            return Some(ConversionJob::Audio { ... });
        }
    }

    // PDF rules
    for rule in &watch_config.pdf_rules {
        if rule.format.is_none() && rule.input_extensions.contains(&file_ext) {
            return Some(ConversionJob::Pdf { ... });
        }
    }

    // Document rules
    for rule in &watch_config.document_rules {
        if rule.format.is_none() && rule.input_extensions.contains(&file_ext) {
            return Some(ConversionJob::Document { ... });
        }
    }

    // Image rules
    for rule in &watch_config.image_rules {
        if rule.format.is_none() && rule.input_extensions.contains(&file_ext) {
            return Some(ConversionJob::Image { ... });
        }
    }

    None
}
```

### create_folders()

```rust
pub fn create_folders(watch_config: &WatchConfig) -> Result<()> {
    // 1. Create watch_folder if not exists
    // 2. Create output_folder if not exists

    // 3. Create subfolders for each active watch type
    //    -> Subfolder name = rule.format (e.g., "->h264", "->mp3", "->epub")

    if watch_config.video_watch {
        for rule in &watch_config.video_rules {
            if let Some(ref fmt) = rule.format {
                let sub = PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                fs::create_dir_all(&sub)?;
            }
        }
    }

    if watch_config.audio_watch {
        for rule in &watch_config.audio_rules {
            if let Some(ref fmt) = rule.format {
                let sub = PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                fs::create_dir_all(&sub)?;
            }
        }
    }

    if watch_config.image_watch {
        for rule in &watch_config.image_rules {
            if let Some(ref fmt) = rule.format {
                let sub = PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                fs::create_dir_all(&sub)?;
            }
        }
    }

    if watch_config.doc_watch {
        for rule in &watch_config.document_rules {
            if let Some(ref fmt) = rule.format {
                let sub = PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                fs::create_dir_all(&sub)?;
            }
        }
    }

    if watch_config.custom_watch {
        for rule in &watch_config.custom_rules {
            if let Some(ref fmt) = rule.format {
                let sub = PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
                fs::create_dir_all(&sub)?;
            }
        }
    }

    // PDF also supports subfolders (no flag needed, always on if format is set)
    for rule in &watch_config.pdf_rules {
        if let Some(ref fmt) = rule.format {
            let sub = PathBuf::from(&watch_config.watch_folder).join(format!("->{}", fmt));
            fs::create_dir_all(&sub)?;
        }
    }

    Ok(())
}
```

---

## Module: `src/watcher/embedded.rs` — NEW

Background scanner that watches for `mainconfig.yaml` files in configurable paths and dynamically registers/unregisters watchers.

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│                 embedded_scanner task                    │
│                                                         │
│  Loop every embedded_scan_interval_s:                    │
│    1. Walk embedded_scan_paths looking for config files  │
│    2. Compare found paths vs previously known set        │
│    3. For NEW paths:                                     │
│       a. Parse mainconfig.yaml → EmbeddedConfig             │
│       b. Convert to WatchConfig                          │
│       c. Send via reload_tx to monitor_manager           │
│    4. For REMOVED paths:                                 │
│       a. Remove from known set                           │
│       b. (monitor_manager handles cleanup)               │
│    5. For CHANGED paths:                                 │
│       a. Re-parse and send updated config                │
│                                                         │
│  Tracks: HashMap<PathBuf, (WatchConfig, FileModified)>   │
└─────────────────────────────────────────────────────────┘
```

### EmbeddedScanner

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;
use tokio::sync::mpsc;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use crate::config::embedded::EmbeddedConfig;
use crate::config::watch::WatchConfig;

pub struct EmbeddedScanner {
    config_name: String,
    scan_paths: Vec<PathBuf>,
    watch_config_name: String,
    known_configs: HashMap<PathBuf, (WatchConfig, SystemTime)>,
    reload_tx: mpsc::Sender<Vec<WatchConfig>>,
}

impl EmbeddedScanner {
    pub fn new(
        config_name: String,
        scan_paths: Vec<String>,
        watcher_name_prefix: String,
        reload_tx: mpsc::Sender<Vec<WatchConfig>>,
    ) -> Self { ... }

    /// Main scan: walk paths, find config files, compare with known set
    pub async fn scan(&mut self) -> Result<()> {
        let found = self.find_config_files().await?;
        let mut updates = Vec::new();

        for (path, modified) in &found {
            match self.known_configs.get(path) {
                Some((existing, prev_modified)) if *prev_modified == *modified => {
                    // Unchanged
                    updates.push(existing.clone());
                }
                _ => {
                    // New or changed — parse and add
                    match self.parse_config(path).await {
                        Ok(wc) => {
                            info!("Embedded config found: {:?}", path);
                            self.known_configs.insert(path.clone(), (wc.clone(), *modified));
                            updates.push(wc);
                        }
                        Err(e) => {
                            warn!("Failed to parse embedded config {:?}: {}", path, e);
                        }
                    }
                }
            }
        }

        // Detect removed configs
        let removed: Vec<PathBuf> = self.known_configs.keys()
            .filter(|p| !found.contains_key(p))
            .cloned()
            .collect();
        for path in &removed {
            info!("Embedded config removed: {:?}", path);
            self.known_configs.remove(path);
        }

        if !updates.is_empty() || !removed.is_empty() {
            // Send all current configs (including main configs) via reload_tx
            // The monitor_manager will reconcile
            let all_configs = self.known_configs.values().map(|(c, _)| c.clone()).collect();
            let _ = self.reload_tx.send(all_configs).await;
        }

        Ok(())
    }

    /// Walk scan_paths recursively to find config files
    async fn find_config_files(&self) -> Result<HashMap<PathBuf, SystemTime>> {
        let mut found = HashMap::new();
        for base in &self.scan_paths {
            self.walk_dir(base, &mut found)?;
        }
        Ok(found)
    }

    fn walk_dir(&self, dir: &Path, found: &mut HashMap<PathBuf, SystemTime>) -> Result<()> {
        if dir.is_dir() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    self.walk_dir(&path, found)?;
                } else if path.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n == self.config_name)
                    .unwrap_or(false)
                {
                    let modified = entry.metadata()?.modified()?;
                    found.insert(path, modified);
                }
            }
        }
        Ok(())
    }

    /// Parse a mainconfig.yaml file into a WatchConfig
    async fn parse_config(&self, config_path: &Path) -> Result<WatchConfig> {
        let content = fs::read_to_string(config_path)?;
        let embedded: EmbeddedConfig = serde_yaml::from_str(&content)?;

        let watch_folder = config_path.parent()
            .ok_or_else(|| anyhow::anyhow!("Cannot get parent directory"))?
            .to_string_lossy()
            .to_string();

        let mut wc = embedded.to_watch_config(&watch_folder);

        // Prefix the watcher name to avoid collision with main configs
        // The monitor_manager will use this name for the watcher
        Ok(wc)
    }
}
```

### Background Task

```rust
/// Background task spawned from main.rs
/// Scans for embedded configs and sends updates via reload_tx
pub async fn run_embedded_scanner(
    global_config: GlobalConfig,
    reload_tx: mpsc::Sender<Vec<WatchConfig>>,
    main_configs: Vec<WatchConfig>,  // current main configs to keep alive
) {
    if !global_config.scan_embedded_configs {
        info!("Embedded config scanning is disabled");
        return;
    }

    let interval = if global_config.embedded_scan_interval_s > 0 {
        Duration::from_secs(global_config.embedded_scan_interval_s)
    } else {
        Duration::from_secs(60) // default
    };

    let mut scanner = EmbeddedScanner::new(
        global_config.embedded_config_name,
        global_config.embedded_scan_paths,
        "embedded_".to_string(),
        reload_tx.clone(),
    );

    let mut ticker = tokio::time::interval(interval);

    loop {
        ticker.tick().await;
        if let Err(e) = scanner.scan().await {
            warn!("Embedded config scan error: {}", e);
        }
    }
}
```

---

## Config Loading (`src/config/mod.rs`)

```rust
pub fn load_watch_configs(custom_path: Option<&Path>) -> Result<Vec<WatchConfig>> {
    if let Some(path) = custom_path {
        // Load single file
        let content = fs::read_to_string(path)?;
        let collection: WatchConfigCollection = serde_yaml::from_str(&content)?;
        Ok(collection.watchers)
    } else {
        // Try config/watchers.yaml first
        let path = PathBuf::from("config/watchers.yaml");
        if path.exists() {
            let content = fs::read_to_string(&path)?;
            let collection: WatchConfigCollection = serde_yaml::from_str(&content)?;
            return Ok(collection.watchers);
        }

        // Fallback: load individual config/watch*.yaml files
        let mut configs = Vec::new();
        let glob_pattern = "config/watch*.yaml";
        // ... iterate matching files, parse each as WatchConfig ...

        if configs.is_empty() {
            anyhow::bail!("No watch configs found");
        }

        Ok(configs)
    }
}
```

---

## monitor_manager (in `main.rs`)

Manages lifecycle of all watcher tasks. Receives config updates from both the hot-reloader and the embedded scanner.

```rust
// Spawned once at startup
let monitor_manager = tokio::spawn(async move {
    let mut current_folders: Vec<String> = active_watch_folders;

    while let Some(new_configs) = reload_rx.recv().await {
        // 1. Broadcast shutdown to all current monitors
        let _ = shutdown_for_manager.send(());

        // 2. Wait for old monitors to finish
        let old_handles = std::mem::take(&mut *monitor_handles_clone.lock().await);
        for handle in old_handles {
            let _ = handle.await;
        }

        // 3. Re-create folders for all configs
        for cfg in &new_configs {
            if let Err(e) = watcher::monitor::create_folders(cfg) {
                warn!("Failed to create folders for {}: {}", cfg.watch_folder, e);
            }
        }

        // 4. Update health server
        for cfg in &new_configs {
            health_server.add_watcher_with_config(cfg).await;
        }

        // 5. Spawn new monitors
        let fresh_global = global_for_monitors.lock().await.clone();
        let new_handles = spawn_monitors(&new_configs, &job_tx,
            &shutdown_for_manager, &health_server, &fresh_global);
        *monitor_handles_clone.lock().await = new_handles;

        current_folders = new_configs.iter().map(|c| c.watch_folder.clone()).collect();
        info!("Monitors restarted with {} watcher(s)", current_folders.len());
    }
});
```
