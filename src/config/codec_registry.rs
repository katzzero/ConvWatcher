use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use log::{info, warn};
use serde::{Deserialize, Serialize};

use super::watch::{
    AudioRule, CustomRule, DocumentRule, ImageRule, PdfMode, PdfQuality, PdfRule, VideoRule,
    WatchConfig, WatchType,
};

/// Paths to codec preset files, relative to the config directory.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CodecPresetPaths {
    #[serde(default = "default_video_presets")]
    pub video: String,
    #[serde(default = "default_audio_presets")]
    pub audio: String,
    #[serde(default = "default_image_presets")]
    pub image: String,
    #[serde(default = "default_pdf_presets")]
    pub pdf: String,
    #[serde(default = "default_document_presets")]
    pub document: String,
    #[serde(default = "default_custom_presets")]
    pub custom: String,
}

fn default_video_presets() -> String {
    "video_codecs.yaml".to_string()
}
fn default_audio_presets() -> String {
    "audio_codecs.yaml".to_string()
}
fn default_image_presets() -> String {
    "image_codecs.yaml".to_string()
}
fn default_pdf_presets() -> String {
    "pdf_presets.yaml".to_string()
}
fn default_document_presets() -> String {
    "document_presets.yaml".to_string()
}
fn default_custom_presets() -> String {
    "custom_presets.yaml".to_string()
}

impl Default for CodecPresetPaths {
    fn default() -> Self {
        Self {
            video: default_video_presets(),
            audio: default_audio_presets(),
            image: default_image_presets(),
            pdf: default_pdf_presets(),
            document: default_document_presets(),
            custom: default_custom_presets(),
        }
    }
}

/// Generic preset loaded from a YAML file.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct CodecPreset {
    pub codec: Option<String>,
    pub quality: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_bitrate: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
    pub transparent: Option<bool>,
    pub mode: Option<PdfMode>,
    pub pdf_quality: Option<PdfQuality>,
    pub pdfa_version: Option<String>,
    pub resolution: Option<u32>,
    pub password: Option<String>,
    pub pdf_engine: Option<String>,
    pub toc: Option<bool>,
    pub toc_depth: Option<u8>,
    pub css: Option<String>,
    pub template: Option<String>,
    pub standalone: Option<bool>,
    pub metadata: Option<Vec<String>>,
    pub options: Option<Vec<String>>,
    pub command: Option<String>,
    pub output_ext: Option<String>,
    pub description: Option<String>,
}

/// All loaded presets, organized by type.
#[derive(Debug, Clone)]
pub struct CodecRegistry {
    pub video: HashMap<String, CodecPreset>,
    pub audio: HashMap<String, CodecPreset>,
    pub image: HashMap<String, CodecPreset>,
    pub pdf: HashMap<String, CodecPreset>,
    pub document: HashMap<String, CodecPreset>,
    pub custom: HashMap<String, CodecPreset>,
}

impl CodecRegistry {
    pub fn new() -> Self {
        Self {
            video: HashMap::new(),
            audio: HashMap::new(),
            image: HashMap::new(),
            pdf: HashMap::new(),
            document: HashMap::new(),
            custom: HashMap::new(),
        }
    }

    pub fn load(config_dir: &Path, paths: &CodecPresetPaths) -> Result<Self> {
        let mut registry = Self::new();

        validate_preset_path(&paths.video, "video")?;
        validate_preset_path(&paths.audio, "audio")?;
        validate_preset_path(&paths.image, "image")?;
        validate_preset_path(&paths.pdf, "pdf")?;
        validate_preset_path(&paths.document, "document")?;
        validate_preset_path(&paths.custom, "custom")?;

        registry.video = load_preset_file(&config_dir.join(&paths.video), "video")?;
        registry.audio = load_preset_file(&config_dir.join(&paths.audio), "audio")?;
        registry.image = load_preset_file(&config_dir.join(&paths.image), "image")?;
        registry.pdf = load_preset_file(&config_dir.join(&paths.pdf), "pdf")?;
        registry.document = load_preset_file(&config_dir.join(&paths.document), "document")?;
        registry.custom = load_preset_file(&config_dir.join(&paths.custom), "custom")?;

        info!(
            "Codec presets loaded: video={}, audio={}, image={}, pdf={}, document={}, custom={}",
            registry.video.len(),
            registry.audio.len(),
            registry.image.len(),
            registry.pdf.len(),
            registry.document.len(),
            registry.custom.len(),
        );

        Ok(registry)
    }
}

fn validate_preset_path(path: &str, label: &str) -> Result<()> {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        anyhow::bail!(
            "codec_presets.{}: absolute path not allowed, got '{}'",
            label,
            path
        );
    }
    for component in p.components() {
        if matches!(component, std::path::Component::ParentDir) {
            anyhow::bail!(
                "codec_presets.{}: path must not contain '..', got '{}'",
                label,
                path
            );
        }
    }
    Ok(())
}

fn load_preset_file(path: &Path, label: &str) -> Result<HashMap<String, CodecPreset>> {
    if !path.exists() {
        warn!(
            "Preset file not found: {} (path: {})",
            label,
            path.display()
        );
        return Ok(HashMap::new());
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read preset file {}", path.display()))?;

    #[derive(Deserialize)]
    struct PresetFile {
        presets: HashMap<String, CodecPreset>,
    }

    let file: PresetFile = serde_yaml::from_str(&content)
        .with_context(|| format!("Failed to parse preset file {}", path.display()))?;

    info!(
        "Loaded {} {} preset(s) from {}",
        file.presets.len(),
        label,
        path.display()
    );
    Ok(file.presets)
}

pub fn resolve_video_rule(
    preset_name: &str,
    input_extensions: Vec<String>,
    subfolder: Option<String>,
    output_name: Option<String>,
    check_duration: Option<bool>,
    min_duration_ratio: Option<f64>,
    codec: Option<String>,
    quality: Option<String>,
    audio_codec: Option<String>,
    audio_bitrate: Option<String>,
    output_ext: Option<String>,
    registry: &CodecRegistry,
) -> Result<VideoRule> {
    let preset = registry.video.get(preset_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Video preset '{}' not found in video_codecs.yaml",
            preset_name
        )
    })?;

    Ok(VideoRule {
        preset: preset_name.to_string(),
        subfolder,
        input_extensions,
        output_ext: Some(
            output_ext
                .or_else(|| preset.output_ext.clone())
                .unwrap_or_else(|| ".mp4".to_string()),
        ),
        codec: Some(
            codec
                .or_else(|| preset.codec.clone())
                .unwrap_or_else(|| "libx264".to_string()),
        ),
        quality: Some(
            quality
                .or_else(|| preset.quality.clone())
                .unwrap_or_else(|| "crf 23".to_string()),
        ),
        audio_codec: Some(
            audio_codec
                .or_else(|| preset.audio_codec.clone())
                .unwrap_or_else(|| "aac".to_string()),
        ),
        audio_bitrate: Some(
            audio_bitrate
                .or_else(|| preset.audio_bitrate.clone())
                .unwrap_or_else(|| "128k".to_string()),
        ),
        output_name: Some(output_name.unwrap_or_else(|| "{base}_{codec}_{num}.{ext}".to_string())),
        check_duration: Some(check_duration.unwrap_or(true)),
        min_duration_ratio: Some(min_duration_ratio.unwrap_or(0.9)),
    })
}

pub fn resolve_image_rule(
    preset_name: &str,
    input_extensions: Vec<String>,
    subfolder: Option<String>,
    output_name: Option<String>,
    quality: Option<u32>,
    transparent: Option<bool>,
    output_ext: Option<String>,
    registry: &CodecRegistry,
) -> Result<ImageRule> {
    let preset = registry.image.get(preset_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Image preset '{}' not found in image_codecs.yaml",
            preset_name
        )
    })?;

    Ok(ImageRule {
        preset: preset_name.to_string(),
        subfolder,
        input_extensions,
        output_ext: Some(
            output_ext
                .or_else(|| preset.output_ext.clone())
                .unwrap_or_else(|| ".png".to_string()),
        ),
        quality: Some(
            quality
                .or(preset.quality.as_ref().and_then(|s| s.parse().ok()))
                .unwrap_or(90),
        ),
        transparent: Some(transparent.or(preset.transparent).unwrap_or(false)),
        output_name: Some(output_name.unwrap_or_else(|| "{base}_conv.{ext}".to_string())),
    })
}

pub fn resolve_audio_rule(
    preset_name: &str,
    input_extensions: Vec<String>,
    subfolder: Option<String>,
    output_name: Option<String>,
    audio_codec: Option<String>,
    audio_bitrate: Option<String>,
    sample_rate: Option<u32>,
    channels: Option<u8>,
    output_ext: Option<String>,
    registry: &CodecRegistry,
) -> Result<AudioRule> {
    let preset = registry.audio.get(preset_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Audio preset '{}' not found in audio_codecs.yaml",
            preset_name
        )
    })?;

    Ok(AudioRule {
        preset: preset_name.to_string(),
        subfolder,
        input_extensions,
        output_ext: Some(
            output_ext
                .or_else(|| preset.output_ext.clone())
                .unwrap_or_else(|| ".mp3".to_string()),
        ),
        audio_codec: Some(
            audio_codec
                .or_else(|| preset.audio_codec.clone())
                .unwrap_or_else(|| "libmp3lame".to_string()),
        ),
        audio_bitrate: Some(
            audio_bitrate
                .or_else(|| preset.audio_bitrate.clone())
                .unwrap_or_else(|| "192k".to_string()),
        ),
        sample_rate: sample_rate.or(preset.sample_rate),
        channels: channels.or(preset.channels),
        output_name: Some(output_name.unwrap_or_else(|| "{base}_{codec}_{num}.{ext}".to_string())),
    })
}

pub fn resolve_pdf_rule(
    preset_name: &str,
    input_extensions: Vec<String>,
    subfolder: Option<String>,
    output_name: Option<String>,
    mode: Option<PdfMode>,
    pdf_quality: Option<PdfQuality>,
    pdfa_version: Option<String>,
    resolution: Option<u32>,
    password: Option<String>,
    output_ext: Option<String>,
    registry: &CodecRegistry,
) -> Result<PdfRule> {
    let preset = registry.pdf.get(preset_name).ok_or_else(|| {
        anyhow::anyhow!("PDF preset '{}' not found in pdf_presets.yaml", preset_name)
    })?;

    Ok(PdfRule {
        preset: preset_name.to_string(),
        subfolder,
        input_extensions,
        output_ext: Some(
            output_ext
                .or_else(|| preset.output_ext.clone())
                .unwrap_or_else(|| ".pdf".to_string()),
        ),
        mode: Some(mode.or(preset.mode.clone()).unwrap_or_default()),
        quality: pdf_quality.or(preset.pdf_quality.clone()),
        pdfa_version: pdfa_version.or_else(|| preset.pdfa_version.clone()),
        resolution: resolution.or(preset.resolution),
        password: password.or_else(|| preset.password.clone()),
        output_name: Some(output_name.unwrap_or_else(|| "{base}_converted.{ext}".to_string())),
    })
}

pub fn resolve_document_rule(
    preset_name: &str,
    input_extensions: Vec<String>,
    subfolder: Option<String>,
    output_name: Option<String>,
    output_ext: Option<String>,
    toc: Option<bool>,
    toc_depth: Option<u8>,
    css: Option<String>,
    template: Option<String>,
    standalone: Option<bool>,
    metadata: Option<Vec<String>>,
    pdf_engine: Option<String>,
    options: Option<Vec<String>>,
    registry: &CodecRegistry,
) -> Result<DocumentRule> {
    let preset = registry.document.get(preset_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Document preset '{}' not found in document_presets.yaml",
            preset_name
        )
    })?;

    Ok(DocumentRule {
        preset: preset_name.to_string(),
        subfolder,
        input_extensions,
        output_ext: Some(
            output_ext
                .or_else(|| preset.output_ext.clone())
                .unwrap_or_else(|| ".pdf".to_string()),
        ),
        toc: Some(toc.or(preset.toc).unwrap_or(false)),
        toc_depth: toc_depth.or(preset.toc_depth),
        css: css.or_else(|| preset.css.clone()),
        template: template.or_else(|| preset.template.clone()),
        standalone: Some(standalone.or(preset.standalone).unwrap_or(false)),
        metadata: metadata.or_else(|| preset.metadata.clone()),
        pdf_engine: pdf_engine.or_else(|| preset.pdf_engine.clone()),
        options: options.or_else(|| preset.options.clone()),
        output_name: Some(output_name.unwrap_or_else(|| "{base}_converted.{ext}".to_string())),
    })
}

pub fn resolve_custom_rule(
    preset_name: &str,
    input_extensions: Vec<String>,
    subfolder: Option<String>,
    output_name: Option<String>,
    description: Option<String>,
    command: Option<String>,
    output_ext: Option<String>,
    registry: &CodecRegistry,
) -> Result<CustomRule> {
    let preset = registry.custom.get(preset_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Custom preset '{}' not found in custom_presets.yaml",
            preset_name
        )
    })?;

    let cmd = command
        .or_else(|| preset.command.clone())
        .ok_or_else(|| anyhow::anyhow!("Custom preset '{}' has no command field", preset_name))?;

    Ok(CustomRule {
        preset: preset_name.to_string(),
        subfolder,
        input_extensions,
        output_ext: Some(
            output_ext
                .or_else(|| preset.output_ext.clone())
                .unwrap_or_else(|| ".mp4".to_string()),
        ),
        command: Some(cmd),
        output_name: Some(output_name.unwrap_or_else(|| "{base}_custom.{ext}".to_string())),
        description: description.or_else(|| preset.description.clone()),
    })
}

pub fn resolve_watchers(
    watchers: Vec<WatchConfig>,
    registry: &CodecRegistry,
) -> Result<Vec<WatchConfig>> {
    let mut resolved = Vec::new();

    for watcher in watchers {
        let resolved_type = match watcher.watch_type {
            WatchType::Video { rules } => {
                let mut resolved_rules = Vec::new();
                for rule in rules {
                    resolved_rules.push(resolve_video_rule(
                        &rule.preset,
                        rule.input_extensions,
                        rule.subfolder,
                        rule.output_name,
                        rule.check_duration,
                        rule.min_duration_ratio,
                        rule.codec,
                        rule.quality,
                        rule.audio_codec,
                        rule.audio_bitrate,
                        rule.output_ext,
                        registry,
                    )?);
                }
                WatchType::Video {
                    rules: resolved_rules,
                }
            }
            WatchType::Image { rules } => {
                let mut resolved_rules = Vec::new();
                for rule in rules {
                    resolved_rules.push(resolve_image_rule(
                        &rule.preset,
                        rule.input_extensions,
                        rule.subfolder,
                        rule.output_name,
                        rule.quality,
                        rule.transparent,
                        rule.output_ext,
                        registry,
                    )?);
                }
                WatchType::Image {
                    rules: resolved_rules,
                }
            }
            WatchType::Audio { rules } => {
                let mut resolved_rules = Vec::new();
                for rule in rules {
                    resolved_rules.push(resolve_audio_rule(
                        &rule.preset,
                        rule.input_extensions,
                        rule.subfolder,
                        rule.output_name,
                        rule.audio_codec,
                        rule.audio_bitrate,
                        rule.sample_rate,
                        rule.channels,
                        rule.output_ext,
                        registry,
                    )?);
                }
                WatchType::Audio {
                    rules: resolved_rules,
                }
            }
            WatchType::Pdf { rules } => {
                let mut resolved_rules = Vec::new();
                for rule in rules {
                    resolved_rules.push(resolve_pdf_rule(
                        &rule.preset,
                        rule.input_extensions,
                        rule.subfolder,
                        rule.output_name,
                        rule.mode,
                        rule.quality,
                        rule.pdfa_version,
                        rule.resolution,
                        rule.password,
                        rule.output_ext,
                        registry,
                    )?);
                }
                WatchType::Pdf {
                    rules: resolved_rules,
                }
            }
            WatchType::Document { rules } => {
                let mut resolved_rules = Vec::new();
                for rule in rules {
                    resolved_rules.push(resolve_document_rule(
                        &rule.preset,
                        rule.input_extensions,
                        rule.subfolder,
                        rule.output_name,
                        rule.output_ext,
                        rule.toc,
                        rule.toc_depth,
                        rule.css,
                        rule.template,
                        rule.standalone,
                        rule.metadata,
                        rule.pdf_engine,
                        rule.options,
                        registry,
                    )?);
                }
                WatchType::Document {
                    rules: resolved_rules,
                }
            }
            WatchType::Custom { rules } => {
                let mut resolved_rules = Vec::new();
                for rule in rules {
                    resolved_rules.push(resolve_custom_rule(
                        &rule.preset,
                        rule.input_extensions,
                        rule.subfolder,
                        rule.output_name,
                        rule.description,
                        rule.command,
                        rule.output_ext,
                        registry,
                    )?);
                }
                WatchType::Custom {
                    rules: resolved_rules,
                }
            }
        };

        resolved.push(WatchConfig {
            name: watcher.name,
            watch_folder: watcher.watch_folder,
            output_folder: watcher.output_folder,
            subfolders: watcher.subfolders,
            watch_type: resolved_type,
        });
    }

    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::watch;

    fn test_registry() -> CodecRegistry {
        let mut registry = CodecRegistry::new();
        registry.video.insert(
            "libx264".to_string(),
            CodecPreset {
                codec: Some("libx264".to_string()),
                quality: Some("crf 23".to_string()),
                audio_codec: Some("aac".to_string()),
                audio_bitrate: Some("128k".to_string()),
                output_ext: Some(".mp4".to_string()),
                ..Default::default()
            },
        );
        registry.video.insert(
            "h264_nvenc".to_string(),
            CodecPreset {
                codec: Some("h264_nvenc".to_string()),
                quality: Some("cq 23".to_string()),
                audio_codec: Some("copy".to_string()),
                audio_bitrate: None,
                output_ext: Some(".mp4".to_string()),
                ..Default::default()
            },
        );
        registry.image.insert(
            "jpeg_80".to_string(),
            CodecPreset {
                quality: Some("80".to_string()),
                output_ext: Some(".jpg".to_string()),
                ..Default::default()
            },
        );
        registry.audio.insert(
            "mp3_192".to_string(),
            CodecPreset {
                audio_codec: Some("libmp3lame".to_string()),
                audio_bitrate: Some("192k".to_string()),
                output_ext: Some(".mp3".to_string()),
                ..Default::default()
            },
        );
        registry.pdf.insert(
            "pdf_ebook".to_string(),
            CodecPreset {
                mode: Some(PdfMode::Compress),
                pdf_quality: Some(PdfQuality::Ebook),
                output_ext: Some(".pdf".to_string()),
                ..Default::default()
            },
        );
        registry.document.insert(
            "docx_to_pdf".to_string(),
            CodecPreset {
                output_ext: Some(".pdf".to_string()),
                pdf_engine: Some("weasyprint".to_string()),
                ..Default::default()
            },
        );
        registry
    }

    #[test]
    fn test_resolve_video_rule_from_preset() {
        let registry = test_registry();
        let rule = resolve_video_rule(
            "libx264",
            vec![".mp4".into()],
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &registry,
        )
        .unwrap();

        assert_eq!(rule.codec.as_deref(), Some("libx264"));
        assert_eq!(rule.quality.as_deref(), Some("crf 23"));
        assert_eq!(rule.audio_codec.as_deref(), Some("aac"));
        assert_eq!(rule.audio_bitrate.as_deref(), Some("128k"));
        assert_eq!(rule.output_ext.as_deref(), Some(".mp4"));
    }

    #[test]
    fn test_resolve_video_rule_override_preset() {
        let registry = test_registry();
        let rule = resolve_video_rule(
            "libx264",
            vec![".mp4".into()],
            None,
            None,
            None,
            None,
            None,
            Some("crf 18".to_string()),
            None,
            None,
            None,
            &registry,
        )
        .unwrap();

        assert_eq!(rule.quality.as_deref(), Some("crf 18")); // override wins
        assert_eq!(rule.codec.as_deref(), Some("libx264")); // from preset
    }

    #[test]
    fn test_resolve_video_rule_missing_preset() {
        let registry = test_registry();
        let result = resolve_video_rule(
            "nonexistent",
            vec![".mp4".into()],
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &registry,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_resolve_image_rule() {
        let registry = test_registry();
        let rule = resolve_image_rule(
            "jpeg_80",
            vec![".tiff".into()],
            None,
            None,
            None,
            None,
            None,
            &registry,
        )
        .unwrap();

        assert_eq!(rule.quality, Some(80));
        assert_eq!(rule.output_ext.as_deref(), Some(".jpg"));
    }

    #[test]
    fn test_resolve_audio_rule() {
        let registry = test_registry();
        let rule = resolve_audio_rule(
            "mp3_192",
            vec![".wav".into()],
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &registry,
        )
        .unwrap();

        assert_eq!(rule.audio_codec.as_deref(), Some("libmp3lame"));
        assert_eq!(rule.audio_bitrate.as_deref(), Some("192k"));
    }

    #[test]
    fn test_resolve_pdf_rule() {
        let registry = test_registry();
        let rule = resolve_pdf_rule(
            "pdf_ebook",
            vec![".pdf".into()],
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &registry,
        )
        .unwrap();

        assert_eq!(rule.mode, Some(PdfMode::Compress));
    }

    #[test]
    fn test_resolve_document_rule() {
        let registry = test_registry();
        let rule = resolve_document_rule(
            "docx_to_pdf",
            vec![".docx".into()],
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &registry,
        )
        .unwrap();

        assert_eq!(rule.output_ext.as_deref(), Some(".pdf"));
        assert_eq!(rule.pdf_engine.as_deref(), Some("weasyprint"));
    }

    #[test]
    fn test_resolve_watchers_multiple_rules() {
        let registry = test_registry();
        let watchers = vec![WatchConfig {
            name: "test".to_string(),
            subfolders: Vec::new(),
            watch_folder: "/app/inputs/test/".to_string(),
            output_folder: "/app/outputs/test/".to_string(),
            watch_type: WatchType::Video {
                rules: vec![
                    watch::VideoRule {
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
                    },
                    watch::VideoRule {
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
        }];

        let resolved = resolve_watchers(watchers, &registry).unwrap();
        assert_eq!(resolved.len(), 1);

        if let WatchType::Video { rules } = &resolved[0].watch_type {
            assert_eq!(rules.len(), 2);
            assert_eq!(rules[0].codec.as_deref(), Some("libx264"));
            assert_eq!(rules[1].codec.as_deref(), Some("h264_nvenc"));
            assert_eq!(rules[1].audio_codec.as_deref(), Some("copy"));
        } else {
            panic!("Expected Video type");
        }
    }

    #[test]
    fn test_codec_preset_paths_defaults() {
        let paths = CodecPresetPaths::default();
        assert_eq!(paths.video, "video_codecs.yaml");
        assert_eq!(paths.audio, "audio_codecs.yaml");
        assert_eq!(paths.image, "image_codecs.yaml");
        assert_eq!(paths.pdf, "pdf_presets.yaml");
        assert_eq!(paths.document, "document_presets.yaml");
    }
}
