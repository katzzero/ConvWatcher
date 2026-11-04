//! Config discovery, loading and directory bootstrap.
//!
//! Owns the default config path / size limit and the `load_config` entry
//! point used by the binary and the hot-reloader.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::warn;

use super::codec_registry::{resolve_watchers, CodecRegistry};
use super::generate::{generate_default_config, set_secure_dir_mode};
use super::global::GlobalConfig;
use super::presets::generate_preset_files;
use super::validate::validate_config;
use super::watch::WatchConfig;

const CONFIG_PATH: &str = "config/config.yaml";

/// Get the config directory from an optional config path.
/// If a custom path is provided, returns its parent directory.
/// Otherwise, returns the default "config" directory.
pub fn config_dir_for(custom_path: Option<&Path>) -> PathBuf {
    match custom_path {
        Some(path) => path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf(),
        None => PathBuf::from(CONFIG_PATH)
            .parent()
            .unwrap_or_else(|| Path::new("config"))
            .to_path_buf(),
    }
}

/// Maximum config file size (2 MiB) to prevent DoS attacks.
const MAX_CONFIG_BYTES: u64 = 2 * 1024 * 1024;
/// Load all configuration from a single config.yaml file.
/// Returns (GlobalConfig, Vec<WatchConfig>, CodecRegistry).
pub fn load_config(
    custom_path: Option<&Path>,
) -> Result<(GlobalConfig, Vec<WatchConfig>, CodecRegistry)> {
    let path = custom_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(CONFIG_PATH));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create directory {}", parent.display()))?;
    }

    // Generate default config if it doesn't exist
    if !path.exists() {
        let default = generate_default_config(&path)?;
        let global = default.0.clone();
        let registry = default.2.clone();
        let watchers = default.1.clone();
        return Ok((global, watchers, registry));
    }

    // FIX A2: Reject oversized config files (DoS guard)
    let meta =
        std::fs::metadata(&path).with_context(|| format!("Cannot stat {}", path.display()))?;
    if meta.len() > MAX_CONFIG_BYTES {
        anyhow::bail!(
            "Config file {} is {} bytes — exceeds maximum of {} bytes (2 MiB)",
            path.display(),
            meta.len(),
            MAX_CONFIG_BYTES
        );
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("Cannot read {}", path.display()))?;

    #[derive(serde::Deserialize)]
    struct ConfigFile {
        global: Option<GlobalConfig>,
        watchers: Vec<WatchConfig>,
    }

    let config_file: ConfigFile = match serde_yaml::from_str(&content) {
        Ok(cf) => cf,
        Err(e) => {
            anyhow::bail!(
                "Failed to parse {}: {}. The file was NOT overwritten — fix it manually.",
                path.display(),
                e
            );
        }
    };

    let global = config_file.global.unwrap_or_default();

    // Determine config directory for loading presets
    let config_dir = path.parent().unwrap_or(Path::new("config")).to_path_buf();

    // Check if any preset file is missing — if so, regenerate all presets
    let presets_ok = [
        &config_dir.join(&global.codec_presets.video),
        &config_dir.join(&global.codec_presets.audio),
        &config_dir.join(&global.codec_presets.image),
        &config_dir.join(&global.codec_presets.pdf),
        &config_dir.join(&global.codec_presets.document),
        &config_dir.join(&global.codec_presets.custom),
    ]
    .iter()
    .all(|p| p.exists());

    if !presets_ok {
        warn!("One or more preset files missing — regenerating defaults");
        generate_preset_files(&config_dir)?;
    }

    // Load codec presets
    let registry = CodecRegistry::load(&config_dir, &global.codec_presets)
        .with_context(|| "Failed to load codec presets")?;

    // Resolve watchers (merge presets into rules)
    let watchers = resolve_watchers(config_file.watchers, &registry)
        .with_context(|| "Failed to resolve watcher presets")?;

    // Validate paths
    validate_config(&global, &watchers)?;

    // Create missing directories
    create_directories(&global, &watchers, &config_dir)?;

    Ok((global, watchers, registry))
}
pub(crate) fn create_directories(
    global: &GlobalConfig,
    watchers: &[WatchConfig],
    config_dir: &Path,
) -> Result<()> {
    // Create log directory (covers both errors and app logs)
    if let Some(parent) = Path::new(&global.log.errors_file).parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create log directory {}", parent.display()))?;
        let _ = set_secure_dir_mode(parent);
    }

    // Also ensure app_log_file's parent exists (might be same dir as errors_file)
    if let Some(parent) = Path::new(&global.log.app_log_file).parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create log directory {}", parent.display()))?;
        let _ = set_secure_dir_mode(parent);
    }

    // Create history file directory
    if let Some(parent) = Path::new(&global.history.file).parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create history directory {}", parent.display()))?;
    }

    // Create watcher directories
    for watcher in watchers {
        fs::create_dir_all(&watcher.watch_folder)
            .with_context(|| format!("Cannot create watch folder {}", watcher.watch_folder))?;
        fs::create_dir_all(&watcher.output_folder)
            .with_context(|| format!("Cannot create output folder {}", watcher.output_folder))?;
    }

    // Create embedded configs directory relative to config path
    let watchs_dir = config_dir.join("watchs");
    fs::create_dir_all(&watchs_dir).with_context(|| {
        format!(
            "Cannot create config/watchs directory at {}",
            watchs_dir.display()
        )
    })?;
    let _ = set_secure_dir_mode(&watchs_dir);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_are_absolute_paths() {
        let global = GlobalConfig::default();
        // Defaults should be absolute paths (based on CWD)
        assert!(
            global.log.errors_file.starts_with('/'),
            "log.errors_file must be absolute, got {}",
            global.log.errors_file
        );
        assert!(
            global.log.app_log_file.starts_with('/'),
            "log.app_log_file must be absolute, got {}",
            global.log.app_log_file
        );
        assert!(
            global.history.file.starts_with('/'),
            "history.file must be absolute, got {}",
            global.history.file
        );
    }
    #[test]
    fn test_config_dir_for_default() {
        let config_dir = config_dir_for(None);
        assert!(config_dir.is_dir() || config_dir.ends_with("config"));
    }
    #[test]
    fn test_config_dir_for_relative_filename() {
        let dir = config_dir_for(Some(std::path::Path::new("config.yaml")));
        assert_eq!(dir, std::path::PathBuf::from("."));
    }
    #[test]
    fn test_config_dir_for_custom_path() {
        let custom = std::path::Path::new("/tmp/test/config/config.yaml");
        let config_dir = config_dir_for(Some(custom));
        assert_eq!(config_dir, std::path::PathBuf::from("/tmp/test/config"));
    }
    #[test]
    fn test_embedded_scan_interval_default() {
        let global = GlobalConfig::default();
        // Default should be 10 seconds (OVERRIDE-DEFAULT-01)
        assert_eq!(global.embedded_scan_interval_s, 10);
    }
}
