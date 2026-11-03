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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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

impl Default for CodecRegistry {
    fn default() -> Self {
        Self::new()
    }
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
/// Generates one `resolve_*_rule` function. All six resolvers share the same
/// shape: look the preset up by name, then merge every rule field with the
/// precedence **rule override → preset value → hard-coded default**.
///
/// The generated function signature is fixed:
/// `preset_name, input_extensions, subfolder, output_name, <params...>, registry`
/// (matching the historical positional API), returning `Result<#rule>`.
///
/// Each param entry is `name: Type => rule_field, mode [(default)]`; the param
/// name is also the `CodecPreset` field it reads from (as in the original
/// hand-written resolvers). Per-field modes:
/// - `fallback(d)`       → `Some(p.unwrap_or(d))` — no preset involvement.
/// - `string(d)`         → override, preset clone, literal default.
/// - `opt`               → override or preset clone (stays `Option`).
/// - `copy_opt`          → override or preset value (Copy field types).
/// - `parsed_u32(d)`     → preset stores `Option<String>` parsed into `u32`.
/// - `bool_preset(d)`    → override, preset bool, literal default.
/// - `enum_default`      → override, preset clone, `Default::default()`.
/// - `required`          → override or preset clone; errors when both are
///   missing (`"<Label> preset '<name>' has no <field> field"`).
macro_rules! define_rule_resolver {
    // Field-merge helpers. `$preset` / `$pname` are passed in as idents from
    // the generated body so they keep that scope's hygiene context; `$p` is
    // both the parameter binding and the identically named `CodecPreset` field.
    (@field $label:expr, $preset:ident, $pname:ident, $p:ident, fallback($d:tt)) => {
        Some($p.unwrap_or($d))
    };
    (@field $label:expr, $preset:ident, $pname:ident, $p:ident, string($d:tt)) => {
        Some($p.or_else(|| $preset.$p.clone()).unwrap_or_else(|| $d.to_string()))
    };
    (@field $label:expr, $preset:ident, $pname:ident, $p:ident, opt) => {
        $p.or_else(|| $preset.$p.clone())
    };
    (@field $label:expr, $preset:ident, $pname:ident, $p:ident, copy_opt) => {
        $p.or($preset.$p)
    };
    (@field $label:expr, $preset:ident, $pname:ident, $p:ident, parsed_u32($d:tt)) => {
        Some($p.or($preset.$p.as_ref().and_then(|s| s.parse().ok())).unwrap_or($d))
    };
    (@field $label:expr, $preset:ident, $pname:ident, $p:ident, bool_preset($d:tt)) => {
        Some($p.or($preset.$p).unwrap_or($d))
    };
    (@field $label:expr, $preset:ident, $pname:ident, $p:ident, enum_default) => {
        Some($p.or($preset.$p.clone()).unwrap_or_default())
    };
    (@field $label:expr, $preset:ident, $pname:ident, $p:ident, required) => {
        Some(
            $p.or_else(|| $preset.$p.clone())
                .ok_or_else(|| anyhow::anyhow!("{} preset '{}' has no {} field", $label, $pname, stringify!($p)))?,
        )
    };

    (
        fn $fn_name:ident,
        map: $map:ident,
        label: $label:expr,
        preset_file: $preset_file:expr,
        rule: $rule:ident,
        output_name_default: $name_default:expr,
        params: [ $( $param:ident : $param_ty:ty => $field:ident, $mode:ident $( ( $d:tt ) )? , )* ],
    ) => {
        #[allow(clippy::too_many_arguments)]
        pub fn $fn_name(
            preset_name: &str,
            input_extensions: Vec<String>,
            subfolder: Option<String>,
            output_name: Option<String>,
            $( $param : $param_ty, )*
            registry: &CodecRegistry,
        ) -> Result<$rule> {
            let preset = registry.$map.get(preset_name).ok_or_else(|| {
                anyhow::anyhow!("{} preset '{}' not found in {}", $label, preset_name, $preset_file)
            })?;

            Ok($rule {
                preset: preset_name.to_string(),
                subfolder,
                input_extensions,
                output_name: Some(output_name.unwrap_or_else(|| $name_default.to_string())),
                $(
                    $field: define_rule_resolver!(
                        @field $label, preset, preset_name, $param, $mode $( ( $d ) )?
                    ),
                )*
            })
        }
    };
}

define_rule_resolver! {
    fn resolve_video_rule,
    map: video,
    label: "Video",
    preset_file: "video_codecs.yaml",
    rule: VideoRule,
    output_name_default: "{base}_{codec}_{num}.{ext}",
    params: [
        check_duration: Option<bool> => check_duration, fallback(true),
        min_duration_ratio: Option<f64> => min_duration_ratio, fallback(0.9),
        codec: Option<String> => codec, string("libx264"),
        quality: Option<String> => quality, string("crf 23"),
        audio_codec: Option<String> => audio_codec, string("aac"),
        audio_bitrate: Option<String> => audio_bitrate, string("128k"),
        output_ext: Option<String> => output_ext, string(".mp4"),
    ],
}

define_rule_resolver! {
    fn resolve_image_rule,
    map: image,
    label: "Image",
    preset_file: "image_codecs.yaml",
    rule: ImageRule,
    output_name_default: "{base}_conv.{ext}",
    params: [
        quality: Option<u32> => quality, parsed_u32(90),
        transparent: Option<bool> => transparent, bool_preset(false),
        output_ext: Option<String> => output_ext, string(".png"),
    ],
}

define_rule_resolver! {
    fn resolve_audio_rule,
    map: audio,
    label: "Audio",
    preset_file: "audio_codecs.yaml",
    rule: AudioRule,
    output_name_default: "{base}_{codec}_{num}.{ext}",
    params: [
        audio_codec: Option<String> => audio_codec, string("libmp3lame"),
        audio_bitrate: Option<String> => audio_bitrate, string("192k"),
        sample_rate: Option<u32> => sample_rate, copy_opt,
        channels: Option<u8> => channels, copy_opt,
        output_ext: Option<String> => output_ext, string(".mp3"),
    ],
}

define_rule_resolver! {
    fn resolve_pdf_rule,
    map: pdf,
    label: "PDF",
    preset_file: "pdf_presets.yaml",
    rule: PdfRule,
    output_name_default: "{base}_converted.{ext}",
    params: [
        mode: Option<PdfMode> => mode, enum_default,
        pdf_quality: Option<PdfQuality> => quality, opt,
        pdfa_version: Option<String> => pdfa_version, opt,
        resolution: Option<u32> => resolution, copy_opt,
        password: Option<String> => password, opt,
        output_ext: Option<String> => output_ext, string(".pdf"),
    ],
}

define_rule_resolver! {
    fn resolve_document_rule,
    map: document,
    label: "Document",
    preset_file: "document_presets.yaml",
    rule: DocumentRule,
    output_name_default: "{base}_converted.{ext}",
    params: [
        output_ext: Option<String> => output_ext, string(".pdf"),
        toc: Option<bool> => toc, bool_preset(false),
        toc_depth: Option<u8> => toc_depth, copy_opt,
        css: Option<String> => css, opt,
        template: Option<String> => template, opt,
        standalone: Option<bool> => standalone, bool_preset(false),
        metadata: Option<Vec<String>> => metadata, opt,
        pdf_engine: Option<String> => pdf_engine, opt,
        options: Option<Vec<String>> => options, opt,
    ],
}

define_rule_resolver! {
    fn resolve_custom_rule,
    map: custom,
    label: "Custom",
    preset_file: "custom_presets.yaml",
    rule: CustomRule,
    output_name_default: "{base}_custom.{ext}",
    params: [
        description: Option<String> => description, opt,
        command: Option<String> => command, required,
        output_ext: Option<String> => output_ext, string(".mp4"),
    ],
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
    fn test_resolve_custom_rule_from_preset() {
        let mut registry = test_registry();
        registry.custom.insert(
            "mkv_mix".to_string(),
            CodecPreset {
                command: Some("ffmpeg -i {input} {output}".to_string()),
                description: Some("remux to mkv".to_string()),
                ..Default::default()
            },
        );
        let rule = resolve_custom_rule(
            "mkv_mix",
            vec![".mp4".into()],
            None,
            None,
            None,
            None,
            None,
            &registry,
        )
        .unwrap();

        assert_eq!(rule.command.as_deref(), Some("ffmpeg -i {input} {output}"));
        assert_eq!(rule.description.as_deref(), Some("remux to mkv"));
        assert_eq!(rule.output_ext.as_deref(), Some(".mp4")); // hard default
        assert_eq!(rule.output_name.as_deref(), Some("{base}_custom.{ext}"));
    }

    #[test]
    fn test_resolve_custom_rule_missing_command() {
        let mut registry = test_registry();
        registry.custom.insert(
            "no_cmd".to_string(),
            CodecPreset {
                output_ext: Some(".mkv".to_string()),
                ..Default::default()
            },
        );
        let err = resolve_custom_rule(
            "no_cmd",
            vec![".mp4".into()],
            None,
            None,
            None,
            None,
            None,
            &registry,
        )
        .unwrap_err();
        // Exact message preserved from the hand-written resolver.
        assert_eq!(
            err.to_string(),
            "Custom preset 'no_cmd' has no command field"
        );
    }

    #[test]
    fn test_resolve_custom_rule_missing_preset_message() {
        let registry = test_registry();
        let err = resolve_custom_rule(
            "ghost",
            vec![".mp4".into()],
            None,
            None,
            None,
            None,
            None,
            &registry,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Custom preset 'ghost' not found in custom_presets.yaml"
        );
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
