//! Structural validation of the loaded configuration: absolute-path rules,
//! watcher names, subfolder names, intervals and rule sanity checks.

use std::path::Path;

use anyhow::Result;
use log::warn;

use super::global::GlobalConfig;
use super::watch::{WatchConfig, WatchType};

pub fn validate_config(global: &GlobalConfig, watchers: &[WatchConfig]) -> Result<()> {
    // Reject zero max_concurrent_conversions (would deadlock)
    if global.max_concurrent_conversions == 0 {
        anyhow::bail!("max_concurrent must be >= 1");
    }

    // Reject zero intervals that would panic in tokio::time::interval
    if global.file_check_interval_ms == 0 {
        anyhow::bail!(
            "file_check_interval must be > 0 (set to 0 would panic tokio::time::interval)"
        );
    }
    if global.config_refresh_interval_s == 0 {
        anyhow::bail!("refresh_interval must be > 0 (set to 0 would panic tokio::time::interval)");
    }
    if global.disk_space.check_interval_s == 0 {
        anyhow::bail!(
            "disk_space.check_interval must be > 0 (set to 0 would panic tokio::time::interval)"
        );
    }

    // Warn about risky small stable_time
    if global.stable_time_ms == 0 {
        warn!("stable_time is 0ms — files will be queued immediately on first scan, risking half-written conversions");
    }

    // Validate global paths
    validate_absolute_path(&global.log.errors_file, "log.errors_file")?;
    validate_absolute_path(&global.log.app_log_file, "log.app_log_file")?;
    validate_absolute_path(&global.history.file, "history.file")?;
    validate_absolute_path(&global.ffmpeg_path, "global.ffmpeg_path")?;
    if let Some(ref fp) = global.ffprobe_path {
        validate_absolute_path(fp, "global.ffprobe_path")?;
    }

    // Track watcher names/watch folders for duplicate detection
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_folders: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Validate watcher paths and names
    for watcher in watchers {
        // Validate watcher.name is not empty
        if watcher.name.is_empty() {
            anyhow::bail!("Watcher name must not be empty");
        }

        // Validate watcher.name contains only valid characters (alphanumeric, _, -, .)
        if !watcher
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
        {
            anyhow::bail!(
                "Watcher name '{}' contains invalid characters (only alphanumeric, '_', '-', '.' allowed)",
                watcher.name
            );
        }

        // Check for duplicate names
        if seen_names.contains(&watcher.name) {
            anyhow::bail!("Duplicate watcher name: '{}'", watcher.name);
        }
        seen_names.insert(watcher.name.clone());

        if !seen_folders.insert(watcher.watch_folder.clone()) {
            anyhow::bail!(
                "Duplicate watch_folder '{}' (watchers must not share a folder)",
                watcher.watch_folder
            );
        }

        if watcher.watch_folder.is_empty() {
            anyhow::bail!(
                "Watcher '{}': watch_folder must be an absolute path, got empty string",
                watcher.name
            );
        }
        validate_absolute_path(
            &watcher.watch_folder,
            &format!("watcher '{}'.watch_folder", watcher.name),
        )?;

        if watcher.output_folder.is_empty() {
            anyhow::bail!(
                "Watcher '{}': output_folder must be an absolute path, got empty string",
                watcher.name
            );
        }
        validate_absolute_path(
            &watcher.output_folder,
            &format!("watcher '{}'.output_folder", watcher.name),
        )?;

        // Validate subfolder names don't contain path separators
        for sf in &watcher.subfolders {
            if sf.name.contains('/') || sf.name.contains('\\') || sf.name.contains("..") {
                anyhow::bail!(
                    "Watcher '{}': subfolder name '{}' contains path separators or '..'",
                    watcher.name,
                    sf.name
                );
            }
        }

        // Validate rules have non-empty input_extensions
        match &watcher.watch_type {
            WatchType::Video { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', video rule {}: input_extensions must not be empty",
                            watcher.name,
                            i
                        );
                    }
                }
            }
            WatchType::Image { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', image rule {}: input_extensions must not be empty",
                            watcher.name,
                            i
                        );
                    }
                }
            }
            WatchType::Audio { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', audio rule {}: input_extensions must not be empty",
                            watcher.name,
                            i
                        );
                    }
                }
            }
            WatchType::Pdf { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', pdf rule {}: input_extensions must not be empty",
                            watcher.name,
                            i
                        );
                    }
                }
            }
            WatchType::Document { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', document rule {}: input_extensions must not be empty",
                            watcher.name,
                            i
                        );
                    }
                }
            }
            WatchType::Custom { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', custom rule {}: input_extensions must not be empty",
                            watcher.name,
                            i
                        );
                    }
                }
            }
        }
    }

    Ok(())
}
pub(crate) fn validate_absolute_path(path: &str, label: &str) -> Result<()> {
    let p = Path::new(path);
    if !p.is_absolute() {
        anyhow::bail!(
            "{}: must be an absolute path, got '{}'. Example: /app/inputs/default/",
            label,
            path
        );
    }
    for component in p.components() {
        if matches!(component, std::path::Component::ParentDir) {
            anyhow::bail!("{}: path must not contain '..', got '{}'", label, path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{global, watch};

    #[test]
    fn test_validate_absolute_path_rejects_relative() {
        assert!(validate_absolute_path("/app/inputs/", "test").is_ok());
        assert!(validate_absolute_path("/usr/bin/ffmpeg", "test").is_ok());
        assert!(validate_absolute_path("./inputs/", "test").is_err());
        assert!(validate_absolute_path("../config/", "test").is_err());
        assert!(validate_absolute_path("config.yaml", "test").is_err());
    }
    fn test_global() -> GlobalConfig {
        GlobalConfig {
            log: global::LogConfig {
                errors_file: "/app/logs/errors.log".to_string(),
                app_log_file: "/app/logs/app.log".to_string(),
                ..Default::default()
            },
            history: global::HistoryConfig {
                file: "/app/logs/history.json".to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
    #[test]
    fn test_validate_empty_watch_folder() {
        let global = test_global();
        let watchers = vec![WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: String::new(),
            output_folder: "/app/outputs/test/".to_string(),
            watch_type: WatchType::Video { rules: vec![] },
        }];
        let result = validate_config(&global, &watchers);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("watch_folder must be an absolute path"));
    }
    #[test]
    fn test_validate_empty_output_folder() {
        let global = test_global();
        let watchers = vec![WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test/".to_string(),
            output_folder: String::new(),
            watch_type: WatchType::Video { rules: vec![] },
        }];
        let result = validate_config(&global, &watchers);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("output_folder must be an absolute path"));
    }
    #[test]
    fn test_validate_empty_input_extensions() {
        let global = test_global();
        let watchers = vec![WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test/".to_string(),
            output_folder: "/app/outputs/test/".to_string(),
            watch_type: WatchType::Video {
                rules: vec![watch::VideoRule {
                    preset: "libx264".to_string(),
                    subfolder: None,
                    input_extensions: vec![],
                    output_ext: None,
                    codec: None,
                    quality: None,
                    audio_codec: None,
                    audio_bitrate: None,
                    output_name: None,
                    check_duration: None,
                    min_duration_ratio: None,
                }],
            },
        }];
        let result = validate_config(&global, &watchers);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("input_extensions must not be empty"));
    }
    #[test]
    fn test_validate_config_passes_with_valid_data() {
        let global = test_global();
        let watchers = vec![WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test/".to_string(),
            output_folder: "/app/outputs/test/".to_string(),
            watch_type: WatchType::Video {
                rules: vec![watch::VideoRule {
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
                }],
            },
        }];
        let result = validate_config(&global, &watchers);
        assert!(result.is_ok());
    }
    #[test]
    fn test_validate_all_watcher_types() {
        let global = test_global();
        let watchers = vec![
            WatchConfig {
                name: "video".to_string(),
                watch_folder: "/app/inputs/video/".to_string(),
                output_folder: "/app/outputs/video/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Video {
                    rules: vec![watch::VideoRule {
                        preset: "libx264".to_string(),
                        subfolder: None,
                        input_extensions: vec![".mp4".into()],
                        output_ext: None,
                        codec: None,
                        quality: None,
                        audio_codec: None,
                        audio_bitrate: None,
                        output_name: None,
                        check_duration: None,
                        min_duration_ratio: None,
                    }],
                },
            },
            WatchConfig {
                name: "image".to_string(),
                watch_folder: "/app/inputs/image/".to_string(),
                output_folder: "/app/outputs/image/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Image {
                    rules: vec![watch::ImageRule {
                        preset: "jpeg_80".to_string(),
                        subfolder: None,
                        input_extensions: vec![".tiff".into()],
                        output_ext: None,
                        quality: None,
                        transparent: None,
                        output_name: None,
                    }],
                },
            },
            WatchConfig {
                name: "audio".to_string(),
                watch_folder: "/app/inputs/audio/".to_string(),
                output_folder: "/app/outputs/audio/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Audio {
                    rules: vec![watch::AudioRule {
                        preset: "mp3_192".to_string(),
                        subfolder: None,
                        input_extensions: vec![".wav".into()],
                        output_ext: None,
                        audio_codec: None,
                        audio_bitrate: None,
                        sample_rate: None,
                        channels: None,
                        output_name: None,
                    }],
                },
            },
            WatchConfig {
                name: "pdf".to_string(),
                watch_folder: "/app/inputs/pdf/".to_string(),
                output_folder: "/app/outputs/pdf/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Pdf {
                    rules: vec![watch::PdfRule {
                        preset: "pdf_ebook".to_string(),
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
            },
            WatchConfig {
                name: "document".to_string(),
                watch_folder: "/app/inputs/doc/".to_string(),
                output_folder: "/app/outputs/doc/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Document {
                    rules: vec![watch::DocumentRule {
                        preset: "docx_to_pdf".to_string(),
                        subfolder: None,
                        input_extensions: vec![".docx".into()],
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
            },
            WatchConfig {
                name: "custom".to_string(),
                watch_folder: "/app/inputs/custom/".to_string(),
                output_folder: "/app/outputs/custom/".to_string(),
                subfolders: Vec::new(),
                watch_type: WatchType::Custom {
                    rules: vec![watch::CustomRule {
                        preset: "handbrake".to_string(),
                        subfolder: None,
                        input_extensions: vec![".mkv".into()],
                        output_ext: None,
                        command: None,
                        output_name: None,
                        description: None,
                    }],
                },
            },
        ];
        let result = validate_config(&global, &watchers);
        assert!(result.is_ok());
    }
    #[test]
    fn test_validate_config_rejects_empty_name() {
        let global = test_global();
        let watchers = vec![WatchConfig {
            name: String::new(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test/".to_string(),
            output_folder: "/app/outputs/test/".to_string(),
            watch_type: WatchType::Video { rules: vec![] },
        }];
        let result = validate_config(&global, &watchers);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("name must not be empty"));
    }
    #[test]
    fn test_validate_config_rejects_invalid_name_chars() {
        let global = test_global();
        let watchers = vec![WatchConfig {
            name: "invalid@name!".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test/".to_string(),
            output_folder: "/app/outputs/test/".to_string(),
            watch_type: WatchType::Video { rules: vec![] },
        }];
        let result = validate_config(&global, &watchers);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid characters"));
    }
    #[test]
    fn test_validate_config_rejects_duplicate_names() {
        let global = test_global();
        let watchers = vec![
            WatchConfig {
                name: "duplicate".to_string(),
                subfolders: Vec::new(),
                watch_folder: "/app/inputs/test1/".to_string(),
                output_folder: "/app/outputs/test1/".to_string(),
                watch_type: WatchType::Video { rules: vec![] },
            },
            WatchConfig {
                name: "duplicate".to_string(),
                subfolders: Vec::new(),
                watch_folder: "/app/inputs/test2/".to_string(),
                output_folder: "/app/outputs/test2/".to_string(),
                watch_type: WatchType::Video { rules: vec![] },
            },
        ];
        let result = validate_config(&global, &watchers);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Duplicate watcher name"));
    }
}
