pub mod global;
pub mod watch;
pub mod embedded;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use log::warn;

use global::GlobalConfig;
use watch::{WatchConfig, WatchConfigCollection, WatchType};

pub fn load_global_config(custom_path: Option<&Path>) -> Result<GlobalConfig> {
    let path = custom_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("config/global.yaml"));

    if !path.exists() {
        return Ok(GlobalConfig::default());
    }

    let content = fs::read_to_string(&path)?;
    let config: GlobalConfig = serde_yaml::from_str(&content)?;
    Ok(config)
}

pub fn load_watch_configs(
    custom_path: Option<&Path>,
    global: &GlobalConfig,
) -> Result<Vec<WatchConfig>> {
    if let Some(path) = custom_path {
        let content = fs::read_to_string(path)?;
        let collection: WatchConfigCollection = serde_yaml::from_str(&content)?;
        return Ok(resolve_all_configs(collection.watchers, global));
    }

    let main_path = PathBuf::from("config/watchers.yaml");
    let mut configs = Vec::new();

    if main_path.exists() {
        let content = fs::read_to_string(&main_path)?;
        let collection: WatchConfigCollection = serde_yaml::from_str(&content)?;
        configs = collection.watchers;
    }

    if configs.is_empty() {
        anyhow::bail!("No watchers found. Create config/watchers.yaml");
    }

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
