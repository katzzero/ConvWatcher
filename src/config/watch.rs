use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchConfigCollection {
    pub watchers: Vec<WatchConfig>,
}

/// Declares a subfolder that the watcher will create and use for rule matching.
/// Files placed in `->{name}/` are processed by rules with `subfolder: <name>`.
#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchType {
    #[serde(rename = "video")]
    Video {
        rules: Vec<VideoRule>,
    },
    #[serde(rename = "image")]
    Image {
        rules: Vec<ImageRule>,
    },
    #[serde(rename = "audio")]
    Audio {
        rules: Vec<AudioRule>,
    },
    #[serde(rename = "pdf")]
    Pdf {
        rules: Vec<PdfRule>,
    },
    #[serde(rename = "document")]
    Document {
        rules: Vec<DocumentRule>,
    },
    #[serde(rename = "custom")]
    Custom {
        rules: Vec<CustomRule>,
    },
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PdfMode {
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

impl Default for PdfMode {
    fn default() -> Self {
        PdfMode::Compress
    }
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
