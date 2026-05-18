use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchConfigCollection {
    pub watchers: Vec<WatchConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchConfig {
    pub name: String,

    #[serde(default)]
    pub watch_folder: String,

    #[serde(default)]
    pub output_folder: String,

    #[serde(flatten)]
    pub watch_type: WatchType,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchType {
    #[serde(rename = "video")]
    Video {
        #[serde(default)]
        video: Vec<VideoRule>,
    },
    #[serde(rename = "image")]
    Image {
        #[serde(default)]
        image: Vec<ImageRule>,
    },
    #[serde(rename = "audio")]
    Audio {
        #[serde(default)]
        audio: Vec<AudioRule>,
    },
    #[serde(rename = "pdf")]
    Pdf {
        #[serde(default)]
        pdf: Vec<PdfRule>,
    },
    #[serde(rename = "document")]
    Document {
        #[serde(default)]
        document: Vec<DocumentRule>,
    },
    #[serde(rename = "custom")]
    Custom {
        #[serde(default)]
        custom: Vec<CustomRule>,
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

fn default_output_ext() -> String { ".mp4".to_string() }
fn default_codec() -> String { "libx264".to_string() }
fn default_quality() -> String { "crf 23".to_string() }
fn default_audio_codec() -> String { "aac".to_string() }
fn default_audio_bitrate() -> String { "128k".to_string() }
fn default_video_template() -> String { "{base}_{codec}_{num}.{ext}".to_string() }
fn default_true() -> bool { true }
fn default_min_duration_ratio() -> f64 { 0.9 }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VideoRule {
    pub format: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default = "default_output_ext")]
    pub output_ext: String,

    #[serde(default = "default_codec")]
    pub codec: String,

    #[serde(default = "default_quality")]
    pub quality: String,

    #[serde(default = "default_audio_codec")]
    pub audio_codec: String,

    #[serde(default = "default_audio_bitrate")]
    pub audio_bitrate: String,

    #[serde(default = "default_video_template")]
    pub output_name_template: String,

    #[serde(default = "default_true")]
    pub check_duration: bool,

    #[serde(default = "default_min_duration_ratio")]
    pub min_duration_ratio: f64,
}

fn default_output_ext_png() -> String { ".png".to_string() }
fn default_image_quality() -> u32 { 90 }
fn default_image_template() -> String { "{base}_conv.{ext}".to_string() }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImageRule {
    pub format: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default = "default_output_ext_png")]
    pub output_ext: String,

    #[serde(default = "default_image_quality")]
    pub quality: u32,

    #[serde(default)]
    pub transparent: bool,

    #[serde(default = "default_image_template")]
    pub output_name_template: String,
}

fn default_audio_ext() -> String { ".mp3".to_string() }
fn default_audio_codec_rule() -> String { "libmp3lame".to_string() }
fn default_audio_bitrate_rule() -> String { "192k".to_string() }
fn default_audio_template() -> String { "{base}_{codec}_{num}.{ext}".to_string() }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioRule {
    pub format: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default = "default_audio_ext")]
    pub output_ext: String,

    #[serde(default = "default_audio_codec_rule")]
    pub audio_codec: String,

    #[serde(default = "default_audio_bitrate_rule")]
    pub audio_bitrate: String,

    #[serde(default)]
    pub sample_rate: Option<u32>,

    #[serde(default)]
    pub channels: Option<u8>,

    #[serde(default)]
    pub quality: Option<String>,

    #[serde(default = "default_audio_template")]
    pub output_name_template: String,
}

fn default_pdf_ext() -> String { ".pdf".to_string() }
fn default_pdf_template() -> String { "{base}_converted.{ext}".to_string() }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PdfRule {
    pub format: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default = "default_pdf_ext")]
    pub output_ext: String,

    #[serde(default)]
    pub mode: PdfMode,

    #[serde(default)]
    pub quality: Option<PdfQuality>,

    #[serde(default)]
    pub pdfa_version: Option<String>,

    #[serde(default)]
    pub page_range: Option<String>,

    #[serde(default)]
    pub resolution: Option<u32>,

    #[serde(default)]
    pub password: Option<String>,

    #[serde(default)]
    pub options: Option<Vec<String>>,

    #[serde(default = "default_pdf_template")]
    pub output_name_template: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfMode {
    Compress,
    PdfA,
    ExtractText,
    ExtractImages,
    ImageToPdf,
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

fn default_doc_ext() -> String { ".pdf".to_string() }
fn default_doc_template() -> String { "{base}_converted.{ext}".to_string() }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DocumentRule {
    pub format: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default = "default_doc_ext")]
    pub output_ext: String,

    #[serde(default)]
    pub toc: bool,

    #[serde(default)]
    pub toc_depth: Option<u8>,

    #[serde(default)]
    pub css: Option<String>,

    #[serde(default)]
    pub template: Option<String>,

    #[serde(default)]
    pub standalone: bool,

    #[serde(default)]
    pub metadata: Option<Vec<String>>,

    #[serde(default)]
    pub pdf_engine: Option<String>,

    #[serde(default)]
    pub options: Option<Vec<String>>,

    #[serde(default = "default_doc_template")]
    pub output_name_template: String,
}

fn default_custom_template() -> String { "{base}_custom.{ext}".to_string() }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CustomRule {
    pub format: Option<String>,

    #[serde(default)]
    pub input_extensions: Vec<String>,

    pub output_ext: String,

    pub command: String,

    #[serde(default = "default_custom_template")]
    pub output_name_template: String,

    #[serde(default)]
    pub description: Option<String>,
}
