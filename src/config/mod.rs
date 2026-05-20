pub mod global;
pub mod watch;
pub mod embedded;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::{info, warn};

use global::GlobalConfig;
use watch::{WatchConfig, WatchConfigCollection, WatchType};

fn invalidate_and_replace(path: &Path, default_yaml: &str, label: &str) {
    let invalid_path = path.with_extension("yaml.invalid");
    if let Err(e) = fs::rename(path, &invalid_path) {
        warn!("Failed to rename invalid {} {:?}: {}", label, path, e);
    } else {
        warn!(
            "{} {:?} is incompatible — renamed to {:?}",
            label, path, invalid_path
        );
    }
    if let Err(e) = fs::write(path, default_yaml) {
        warn!("Failed to write default {}: {}", label, e);
    } else {
        info!("Created fresh default {}: {}", label, path.display());
    }
}

pub fn load_global_config(custom_path: Option<&Path>) -> Result<GlobalConfig> {
    let path = custom_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("config/global.yaml"));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create directory {}", parent.display()))?;
    }

    let default = GlobalConfig::default();
    let default_yaml = serde_yaml::to_string(&default)?;

    if !path.exists() {
        fs::write(&path, &default_yaml)
            .with_context(|| format!("Cannot write {}", path.display()))?;
        info!("Created default config: {}", path.display());
        return Ok(default);
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    match serde_yaml::from_str::<GlobalConfig>(&content) {
        Ok(config) => Ok(config),
        Err(e) => {
            warn!("Failed to parse {}: {}", path.display(), e);
            invalidate_and_replace(&path, &default_yaml, "global.yaml");
            Ok(default)
        }
    }
}

pub fn load_watch_configs(
    custom_path: Option<&Path>,
    global: &GlobalConfig,
) -> Result<Vec<WatchConfig>> {
    if let Some(path) = custom_path {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        let collection: WatchConfigCollection = serde_yaml::from_str(&content)?;
        return Ok(resolve_all_configs(collection.watchers, global));
    }

    let main_path = PathBuf::from("config/watchers.yaml");

    let default_watchers = WatchConfigCollection {
        watchers: vec![WatchConfig {
            name: "default".to_string(),
            watch_folder: String::new(),
            output_folder: String::new(),
            watch_type: WatchType::Video {
                video: Vec::new(),
            },
        }],
    };
    let default_yaml = serde_yaml::to_string(&default_watchers)
        .with_context(|| "Failed to serialize default watcher config")?;

    let configs = if main_path.exists() {
        let content = fs::read_to_string(&main_path)
            .with_context(|| format!("Cannot read {}", main_path.display()))?;
        match serde_yaml::from_str::<WatchConfigCollection>(&content) {
            Ok(collection) => collection.watchers,
            Err(e) => {
                warn!("Failed to parse {}: {}", main_path.display(), e);
                invalidate_and_replace(&main_path, &default_yaml, "watchers.yaml");
                default_watchers.watchers
            }
        }
    } else {
        fs::create_dir_all(main_path.parent().unwrap_or(Path::new("config")))
            .with_context(|| "Cannot create config directory")?;
        fs::write(&main_path, &default_yaml)
            .with_context(|| format!("Cannot write {}", main_path.display()))?;
        info!("Created default watcher config: {}", main_path.display());
        default_watchers.watchers
    };

    Ok(resolve_all_configs(configs, global))
}

fn resolve_all_configs(configs: Vec<WatchConfig>, global: &GlobalConfig) -> Vec<WatchConfig> {
    configs
        .into_iter()
        .map(|mut cfg| {
            resolve_defaults(&mut cfg, global);
            apply_override(&mut cfg, global);
            cfg
        })
        .collect()
}

fn resolve_defaults(cfg: &mut WatchConfig, global: &GlobalConfig) {
    if cfg.watch_folder.is_empty() {
        cfg.watch_folder = format!("{}/{}/", global.inputs_dir.trim_end_matches('/'), cfg.name);
    }
    if cfg.output_folder.is_empty() {
        cfg.output_folder = format!(
            "{}/{}-output/",
            global.outputs_dir.trim_end_matches('/'),
            cfg.name
        );
    }
}

fn apply_override(cfg: &mut WatchConfig, global: &GlobalConfig) {
    if global.embedded_secret.is_empty() {
        return;
    }

    let watchs_dir = PathBuf::from(&global.watchs_dir);
    let override_path = watchs_dir.join(format!("{}.yaml", cfg.name));

    if !override_path.exists() {
        return;
    }

    let content = match fs::read_to_string(&override_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let embedded: embedded::EmbeddedConfig = match serde_yaml::from_str(&content) {
        Ok(e) => e,
        Err(e) => {
            warn!("Failed to parse {}: {}", override_path.display(), e);
            return;
        }
    };

    if embedded.secret != global.embedded_secret {
        warn!("Secret mismatch in {}", override_path.display());
        return;
    }

    if !types_match(&cfg.watch_type, &embedded.watch_type) {
        warn!(
            "Type mismatch in {}: manifesto is '{}', override is '{}'",
            override_path.display(),
            cfg.watch_type.type_name(),
            embedded.watch_type.type_name()
        );
        return;
    }

    cfg.watch_type = embedded.watch_type;

    if !embedded.output_folder.is_empty() {
        cfg.output_folder = embedded.output_folder;
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
