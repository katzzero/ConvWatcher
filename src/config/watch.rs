use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

/// Declares a subfolder that the watcher will create and use for rule matching.
/// Files placed in `->{name}/` are processed by rules with `subfolder: <name>`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Subfolder {
    /// Subfolder name — creates `->{name}/` in the watch folder.
    pub name: String,
    /// Human-readable description (shown in dashboard).
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchConfig {
    pub name: String,
    pub watch_folder: String,
    pub output_folder: String,
    /// Declared subfolders — creates `->{name}/` directories automatically.
    #[serde(default)]
    pub subfolders: Vec<Subfolder>,
    #[serde(flatten)]
    pub watch_type: WatchType,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchType {
    #[serde(rename = "video")]
    Video { rules: Vec<VideoRule> },
    #[serde(rename = "image")]
    Image { rules: Vec<ImageRule> },
    #[serde(rename = "audio")]
    Audio { rules: Vec<AudioRule> },
    #[serde(rename = "pdf")]
    Pdf { rules: Vec<PdfRule> },
    #[serde(rename = "document")]
    Document { rules: Vec<DocumentRule> },
    #[serde(rename = "custom")]
    Custom { rules: Vec<CustomRule> },
}

const VALID_WATCH_TYPES: &[&str] = &["video", "image", "audio", "pdf", "document", "custom"];

fn watch_type_from_kind(kind: &str, rules: serde_yaml::Value) -> Result<WatchType, String> {
    match kind {
        "video" => serde_yaml::from_value(rules)
            .map(|rules| WatchType::Video { rules })
            .map_err(|e| e.to_string()),
        "image" => serde_yaml::from_value(rules)
            .map(|rules| WatchType::Image { rules })
            .map_err(|e| e.to_string()),
        "audio" => serde_yaml::from_value(rules)
            .map(|rules| WatchType::Audio { rules })
            .map_err(|e| e.to_string()),
        "pdf" => serde_yaml::from_value(rules)
            .map(|rules| WatchType::Pdf { rules })
            .map_err(|e| e.to_string()),
        "document" => serde_yaml::from_value(rules)
            .map(|rules| WatchType::Document { rules })
            .map_err(|e| e.to_string()),
        "custom" => serde_yaml::from_value(rules)
            .map(|rules| WatchType::Custom { rules })
            .map_err(|e| e.to_string()),
        _ => Err(format!("unknown watcher type `{}`", kind)),
    }
}

impl<'de> Deserialize<'de> for WatchType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let map = value
            .as_mapping()
            .ok_or_else(|| D::Error::custom("WatchType must deserialize from a YAML mapping"))?;

        let type_key = serde_yaml::Value::String("type".to_string());
        let rules_key = serde_yaml::Value::String("rules".to_string());

        // Documented internal form: `type: <kind>` with a sibling `rules:` list.
        if map.contains_key(&type_key) {
            let unknown = map
                .keys()
                .filter(|k| **k != type_key && **k != rules_key)
                .map(|k| match k {
                    serde_yaml::Value::String(s) => s.as_str(),
                    _other => "non-string key",
                })
                .collect::<Vec<_>>()
                .join(", ");
            if !unknown.is_empty() {
                return Err(D::Error::custom(format!(
                    "unknown field(s) `{}` in watcher config (valid: `type`, `rules`)",
                    unknown
                )));
            }
            let kind = map
                .get(&type_key)
                .and_then(|v| v.as_str())
                .ok_or_else(|| D::Error::custom("WatchType `type` must be a string"))?;
            if !VALID_WATCH_TYPES.contains(&kind) {
                return Err(D::Error::custom(format!(
                    "unknown watcher type `{}` (valid: {})",
                    kind,
                    VALID_WATCH_TYPES.join(", ")
                )));
            }
            let rules = map.get(&rules_key).ok_or_else(|| {
                D::Error::custom(format!("watcher type `{}` requires a `rules` list", kind))
            })?;
            return watch_type_from_kind(kind, rules.clone()).map_err(D::Error::custom);
        }

        // External form: a single type key like `video:` with a nested `rules:` list.
        let mut type_keys = map
            .iter()
            .filter(|(key, _)| key.as_str().is_some_and(|k| VALID_WATCH_TYPES.contains(&k)));
        if let Some((key, value)) = type_keys.next() {
            let key_str = key.as_str().unwrap();
            let extra = type_keys.count()
                + map
                    .iter()
                    .filter(|(k, _)| k.as_str().is_none_or(|k| !VALID_WATCH_TYPES.contains(&k)))
                    .count();
            if extra > 0 {
                let bad = map
                    .keys()
                    .filter(|k| k.as_str().is_none_or(|k| !VALID_WATCH_TYPES.contains(&k)))
                    .map(|k| match k {
                        serde_yaml::Value::String(s) => s.as_str(),
                        _other => "non-string key",
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(D::Error::custom(format!(
                    "unknown field(s) `{}` in watcher config",
                    bad
                )));
            }
            let rules = value.get(&rules_key).ok_or_else(|| {
                D::Error::custom(format!(
                    "watcher type `{}` requires a `rules` list",
                    key_str
                ))
            })?;
            return watch_type_from_kind(key_str, rules.clone()).map_err(D::Error::custom);
        }

        Err(D::Error::custom(format!(
            "WatchType: expected `type:` + `rules:` or a single type key (one of {})",
            VALID_WATCH_TYPES.join(", ")
        )))
    }
}

impl WatchType {
    pub fn type_name(&self) -> &str {
        match self {
            WatchType::Video { .. } => "video",
            WatchType::Image { .. } => "image",
            WatchType::Audio { .. } => "audio",
            WatchType::Pdf { .. } => "pdf",
            WatchType::Document { .. } => "document",
            WatchType::Custom { .. } => "custom",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VideoRule {
    pub preset: String,

    /// Subfolder name — files in `->{subfolder}/` match this rule.
    /// If None, matches files in the root watch folder by extension.
    #[serde(default)]
    pub subfolder: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default)]
    pub output_ext: Option<String>,

    #[serde(default)]
    pub codec: Option<String>,

    #[serde(default)]
    pub quality: Option<String>,

    #[serde(default)]
    pub audio_codec: Option<String>,

    #[serde(default)]
    pub audio_bitrate: Option<String>,

    #[serde(default)]
    pub output_name: Option<String>,

    #[serde(default)]
    pub check_duration: Option<bool>,

    #[serde(default)]
    pub min_duration_ratio: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRule {
    pub preset: String,

    #[serde(default)]
    pub subfolder: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default)]
    pub output_ext: Option<String>,

    #[serde(default)]
    pub quality: Option<u32>,

    #[serde(default)]
    pub transparent: Option<bool>,

    #[serde(default)]
    pub output_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioRule {
    pub preset: String,

    #[serde(default)]
    pub subfolder: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default)]
    pub output_ext: Option<String>,

    #[serde(default)]
    pub audio_codec: Option<String>,

    #[serde(default)]
    pub audio_bitrate: Option<String>,

    #[serde(default)]
    pub sample_rate: Option<u32>,

    #[serde(default)]
    pub channels: Option<u8>,

    #[serde(default)]
    pub output_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PdfRule {
    pub preset: String,

    #[serde(default)]
    pub subfolder: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default)]
    pub output_ext: Option<String>,

    #[serde(default)]
    pub mode: Option<PdfMode>,

    #[serde(default)]
    pub quality: Option<PdfQuality>,

    #[serde(default)]
    pub pdfa_version: Option<String>,

    #[serde(default)]
    pub resolution: Option<u32>,

    #[serde(default)]
    pub password: Option<String>,

    #[serde(default)]
    pub output_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PdfMode {
    #[default]
    Compress,
    PdfA,
    ExtractText,
    ExtractImages,
    ImageToPdf,
    PdfToImages,
    Merge,
    Linearize,
    Encrypt,
    Decrypt,
    Analyze,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfQuality {
    Screen,
    Ebook,
    Printer,
    Prepress,
    Default,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentRule {
    pub preset: String,

    #[serde(default)]
    pub subfolder: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default)]
    pub output_ext: Option<String>,

    #[serde(default)]
    pub toc: Option<bool>,

    #[serde(default)]
    pub toc_depth: Option<u8>,

    #[serde(default)]
    pub css: Option<String>,

    #[serde(default)]
    pub template: Option<String>,

    #[serde(default)]
    pub standalone: Option<bool>,

    #[serde(default)]
    pub metadata: Option<Vec<String>>,

    #[serde(default)]
    pub pdf_engine: Option<String>,

    #[serde(default)]
    pub options: Option<Vec<String>>,

    #[serde(default)]
    pub output_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CustomRule {
    pub preset: String,

    #[serde(default)]
    pub subfolder: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default)]
    pub output_ext: Option<String>,

    #[serde(default)]
    pub command: Option<String>,

    #[serde(default)]
    pub output_name: Option<String>,

    #[serde(default)]
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_rule() -> &'static str {
        "- preset: libx264\n    input_extensions:\n      - .mp4\n"
    }

    #[test]
    fn deserialize_watch_type_internal_form() {
        let yaml = format!("type: video\nrules:\n  {}", minimal_rule());
        let parsed: WatchType = serde_yaml::from_str(&yaml).unwrap();
        match parsed {
            WatchType::Video { rules } => {
                assert_eq!(rules.len(), 1);
                assert_eq!(rules[0].preset, "libx264");
            }
            other => panic!("expected Video, got {:?}", other.type_name()),
        }
    }

    #[test]
    fn deserialize_watch_type_external_form() {
        let yaml = format!("video:\n  rules:\n  {}", minimal_rule());
        let parsed: WatchType = serde_yaml::from_str(&yaml).unwrap();
        match parsed {
            WatchType::Video { rules } => {
                assert_eq!(rules.len(), 1);
                assert_eq!(rules[0].preset, "libx264");
            }
            other => panic!("expected Video, got {:?}", other.type_name()),
        }
    }

    #[test]
    fn deserialize_watch_config_internal_form() {
        let yaml = "name: test_watcher\nwatch_folder: /app/inputs/test/\noutput_folder: /app/outputs/test/\nsubfolders:\n  - name: hd\ntype: audio\nrules:\n  - preset: mp3_192\n    input_extensions:\n      - .wav\n";
        let parsed: WatchConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.name, "test_watcher");
        assert_eq!(parsed.subfolders.len(), 1);
        match parsed.watch_type {
            WatchType::Audio { rules } => assert_eq!(rules[0].preset, "mp3_192"),
            other => panic!("expected Audio, got {:?}", other.type_name()),
        }
    }

    #[test]
    fn serialize_roundtrip_uses_external_form() {
        let parsed: WatchConfig = serde_yaml::from_str(&format!(
            "name: w\nwatch_folder: /a\noutput_folder: /b\ntype: video\nrules:\n  {}",
            minimal_rule()
        ))
        .unwrap();
        let out = serde_yaml::to_string(&parsed).unwrap();
        assert!(out.contains("video:"), "got serialized output:\n{}", out);
    }

    #[test]
    fn unknown_type_is_rejected() {
        let yaml = "type: hologram\nrules:\n  - preset: x\n";
        let result: Result<WatchType, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_rule_field_is_rejected() {
        let yaml = format!(
            "type: video\nrules:\n  {}  bogus_field: 1\n",
            minimal_rule()
        );
        let result: Result<WatchConfig, _> = serde_yaml::from_str(&format!(
            "name: w\nwatch_folder: /a\noutput_folder: /b\n{}",
            yaml
        ));
        assert!(result.is_err(), "unknown rule field should be rejected");
    }

    #[test]
    fn unknown_watcher_field_is_rejected() {
        let yaml = "name: w\nwatch_folder: /a\noutput_folder: /b\nunknown_thing: 1\nvideo:\n  rules:\n  - preset: x\n";
        let result: Result<WatchConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "unknown watcher field should be rejected");
    }
}
