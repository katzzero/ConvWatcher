use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use log::{info, warn};
use tokio::sync::mpsc;

use crate::config::embedded::EmbeddedConfig;
use crate::config::watch::{WatchConfig, WatchType};

pub struct EmbeddedScanner {
    watchs_dir: PathBuf,
    secret: String,
    known_configs: HashMap<PathBuf, (WatchConfig, SystemTime)>,
    reload_tx: mpsc::Sender<Vec<WatchConfig>>,
    main_configs: Vec<WatchConfig>,
}

impl EmbeddedScanner {
    pub fn new(
        watchs_dir: PathBuf,
        secret: String,
        reload_tx: mpsc::Sender<Vec<WatchConfig>>,
        main_configs: Vec<WatchConfig>,
    ) -> Self {
        Self {
            watchs_dir,
            secret,
            known_configs: HashMap::new(),
            reload_tx,
            main_configs,
        }
    }

    pub async fn scan(&mut self) -> anyhow::Result<()> {
        if !self.watchs_dir.exists() {
            return Ok(());
        }

        let found = self.find_config_files()?;
        let mut has_changes = false;

        for (path, modified) in &found {
            match self.known_configs.get(path) {
                Some((_existing, prev_modified)) if *prev_modified == *modified => {}
                _ => match self.parse_and_merge(path) {
                    Ok(wc) => {
                        info!(
                            "Watch config loaded: {:?} -> {}({})",
                            path,
                            wc.name,
                            wc.watch_type.type_name()
                        );
                        self.known_configs
                            .insert(path.clone(), (wc, *modified));
                        has_changes = true;
                    }
                    Err(e) => {
                        warn!("Failed to load watch config {:?}: {}", path, e);
                    }
                },
            }
        }

        let removed: Vec<PathBuf> = self
            .known_configs
            .keys()
            .filter(|p| !found.contains_key(*p))
            .cloned()
            .collect();
        for path in &removed {
            info!("Watch config removed: {:?}", path);
            self.known_configs.remove(path);
            has_changes = true;
        }

        if has_changes {
            let merged = self.merge_all();
            let _ = self.reload_tx.send(merged).await;
        }

        Ok(())
    }

    fn find_config_files(&self) -> anyhow::Result<HashMap<PathBuf, SystemTime>> {
        let mut found = HashMap::new();
        let entries = match std::fs::read_dir(&self.watchs_dir) {
            Ok(e) => e,
            Err(_) => return Ok(found),
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if !name.ends_with(".yaml") && !name.ends_with(".yml") {
                continue;
            }
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    found.insert(path, modified);
                }
            }
        }
        Ok(found)
    }

    fn parse_and_merge(&self, config_path: &Path) -> anyhow::Result<WatchConfig> {
        let content = std::fs::read_to_string(config_path)?;
        let embedded: EmbeddedConfig = serde_yaml::from_str(&content)?;

        if !self.secret.is_empty() && embedded.secret != self.secret {
            anyhow::bail!("Secret mismatch in {:?}", config_path);
        }

        let config_name = config_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid filename: {:?}", config_path))?;

        let base = self
            .main_configs
            .iter()
            .find(|c| c.name == config_name);

        match base {
            Some(manifesto) => {
                if !types_match(&manifesto.watch_type, &embedded.watch_type) {
                    anyhow::bail!(
                        "Type mismatch: manifesto is '{}', override is '{}'",
                        manifesto.watch_type.type_name(),
                        embedded.watch_type.type_name()
                    );
                }
                let mut merged = manifesto.clone();
                merged.watch_type = embedded.watch_type;
                if !embedded.output_folder.is_empty() {
                    merged.output_folder = embedded.output_folder;
                }
                Ok(merged)
            }
            None => {
                anyhow::bail!("No manifesto entry for watcher '{}'", config_name);
            }
        }
    }

    fn merge_all(&self) -> Vec<WatchConfig> {
        let mut result: Vec<WatchConfig> = self.main_configs.clone();
        for (_, (wc, _)) in &self.known_configs {
            if let Some(existing) = result.iter_mut().find(|c| c.name == wc.name) {
                existing.watch_type = wc.watch_type.clone();
                if !wc.output_folder.is_empty() {
                    existing.output_folder = wc.output_folder.clone();
                }
            }
        }
        result
    }
}

fn types_match(manifesto: &WatchType, override_: &WatchType) -> bool {
    matches!(
        (manifesto, override_),
        (WatchType::Video { .. }, WatchType::Video { .. })
            | (WatchType::Image { .. }, WatchType::Image { .. })
            | (WatchType::Audio { .. }, WatchType::Audio { .. })
            | (WatchType::Pdf { .. }, WatchType::Pdf { .. })
            | (WatchType::Document { .. }, WatchType::Document { .. })
            | (WatchType::Custom { .. }, WatchType::Custom { .. })
    )
}

pub async fn run_embedded_scanner(
    watchs_dir: String,
    secret: String,
    scan_interval_secs: u64,
    reload_tx: mpsc::Sender<Vec<WatchConfig>>,
    main_configs: Vec<WatchConfig>,
) {
    if scan_interval_secs == 0 {
        info!("Watch config scanning is disabled");
        return;
    }

    let _ = std::fs::create_dir_all(&watchs_dir);

    let interval = tokio::time::Duration::from_secs(scan_interval_secs);
    let mut scanner = EmbeddedScanner::new(
        PathBuf::from(&watchs_dir),
        secret,
        reload_tx.clone(),
        main_configs,
    );
    let mut ticker = tokio::time::interval(interval);

    loop {
        ticker.tick().await;
        if let Err(e) = scanner.scan().await {
            warn!("Watch config scan error: {}", e);
        }
    }
}
