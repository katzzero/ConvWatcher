# ConvWatcher — Configuration Structs

## Overview

Configuration is split into two layers:
1. **Global config** (`config/global.yaml`) — daemon-wide settings
2. **Watch configs** (`config/watchers.yaml` or individual `watch*.yaml`) — per-watcher rules

Additionally, **embedded configs** (`mainconfig.yaml`) are auto-detected in any folder.

---

## Module: `src/config/global.rs`

### GlobalConfig

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GlobalConfig {
    #[serde(default = "default_file_check_interval")]
    pub file_check_interval_ms: u64,

    #[serde(default = "default_stable_time")]
    pub stable_time_ms: u64,

    #[serde(default = "default_ffmpeg_path")]
    pub ffmpeg_path: String,

    #[serde(default)]
    pub ffprobe_path: String,

    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_conversions: u32,

    #[serde(default = "default_config_refresh")]
    pub config_refresh_interval_s: u64,

    #[serde(default)]
    pub embedded_scan_interval_s: u64,      // NEW: scan for mainconfig.yaml every N seconds

    #[serde(default)]
    pub embedded_config_name: String,       // NEW: filename to scan for (default: "mainconfig.yaml")

    #[serde(default)]
    pub scan_embedded_configs: bool,         // NEW: enable embedded config detection

    #[serde(default)]
    pub embedded_scan_paths: Vec<String>,    // NEW: base paths to scan (default: ["/data", "."])

    #[serde(default = "default_log_config")]
    pub log: LogConfig,

    #[serde(default = "default_healthcheck")]
    pub healthcheck: HealthcheckConfig,

    #[serde(default = "default_disk_space")]
    pub disk_space: DiskSpaceConfig,

    #[serde(default = "default_history")]
    pub history: HistoryConfig,
}

// Defaults
fn default_file_check_interval() -> u64 { 2000 }        // 2 seconds
fn default_stable_time() -> u64 { 5000 }                 // 5 seconds
fn default_ffmpeg_path() -> String { "ffmpeg".to_string() }
fn default_max_concurrent() -> u32 { 4 }
fn default_config_refresh() -> u64 { 300 }               // 5 minutes
```

### Sub-configs

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogConfig {
    pub errors_file: String,          // default: "./logs/errors.log"
    pub max_log_files: u32,           // default: 30
    pub max_log_size_mb: u64,         // default: 100
    pub max_error_log_size_mb: u64,   // default: 50
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HealthcheckConfig {
    pub http_port: u16,               // default: 8080
    pub bind_address: String,         // default: "0.0.0.0"
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiskSpaceConfig {
    pub check_interval_s: u64,        // default: 60
    pub threshold: DiskSpaceThreshold,
    pub check_output: bool,
    pub check_watch: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DiskSpaceThreshold {
    Mb(u64),
    Gb(f64),
    Percent(f64),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HistoryConfig {
    pub persistent: bool,             // default: false
    pub file: String,                 // default: "./logs/history.json"
    pub max_records: usize,           // default: 500
}
```

---

## Module: `src/config/watch.rs`

### WatchConfigCollection

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchConfigCollection {
    pub watchers: Vec<WatchConfig>,
}
```

### WatchConfig — Universal

Breaking change from v1: `video_watch`, `image_watch`, `audio_watch`, `doc_watch`, `custom_watch` are boolean flags that ENABLE subfolder mode for that type. All rule lists work regardless of the flag (extension matching always works). When the flag is true AND rules have a `format` field, `->format/` subfolders are created and matched.

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WatchConfig {
    pub watch_folder: String,
    pub output_folder: String,

    // Subfolder mode flags (control whether ->format/ subfolders are active)
    #[serde(default)]
    pub video_watch: bool,

    #[serde(default)]
    pub image_watch: bool,

    #[serde(default)]
    pub audio_watch: bool,

    #[serde(default)]
    pub doc_watch: bool,

    #[serde(default)]
    pub custom_watch: bool,

    // Rule lists (all plural Vec — no more singular rules)
    #[serde(rename = "video_rules", default)]
    pub video_rules: Vec<VideoRule>,

    #[serde(rename = "image_rules", default)]
    pub image_rules: Vec<ImageRule>,

    #[serde(rename = "audio_rules", default)]
    pub audio_rules: Vec<AudioRule>,

    #[serde(rename = "pdf_rules", default)]
    pub pdf_rules: Vec<PdfRule>,

    #[serde(rename = "document_rules", default)]
    pub document_rules: Vec<DocumentRule>,

    #[serde(rename = "custom_rules", default)]
    pub custom_rules: Vec<CustomRule>,
}
```

### VideoRule

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VideoRule {
    pub format: Option<String>,      // Some("h264") = subfolder mode, None = extension match

    #[serde(default)]
    pub input_extensions: Vec<String>, // used when format is None

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

fn default_output_ext() -> String { ".mp4".to_string() }
fn default_codec() -> String { "libx264".to_string() }
fn default_quality() -> String { "crf 23".to_string() }
fn default_audio_codec() -> String { "aac".to_string() }
fn default_audio_bitrate() -> String { "128k".to_string() }
fn default_video_template() -> String { "{base}_{codec}_{num}.{ext}".to_string() }
fn default_true() -> bool { true }
fn default_min_duration_ratio() -> f64 { 0.9 }
```

### ImageRule

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImageRule {
    pub format: Option<String>,      // Some("jpg") = subfolder mode, None = extension

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

fn default_output_ext_png() -> String { ".png".to_string() }
fn default_image_quality() -> u32 { 90 }
fn default_image_template() -> String { "{base}_conv.{ext}".to_string() }
```

### AudioRule

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioRule {
    pub format: Option<String>,      // Some("mp3") = subfolder mode, None = extension

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default = "default_audio_ext")]
    pub output_ext: String,

    #[serde(default = "default_audio_codec_rule")]
    pub audio_codec: String,

    #[serde(default = "default_audio_bitrate_rule")]
    pub audio_bitrate: String,

    #[serde(default)]
    pub sample_rate: Option<u32>,     // 44100, 48000, 96000, etc.

    #[serde(default)]
    pub channels: Option<u8>,         // 1 (mono), 2 (stereo)

    #[serde(default)]
    pub quality: Option<String>,      // VBR quality: "0"-"9" for mp3, "-1"-"10" for vorbis

    #[serde(default = "default_audio_template")]
    pub output_name_template: String,
}

fn default_audio_ext() -> String { ".mp3".to_string() }
fn default_audio_codec_rule() -> String { "libmp3lame".to_string() }
fn default_audio_bitrate_rule() -> String { "192k".to_string() }
fn default_audio_template() -> String { "{base}_{codec}_{num}.{ext}".to_string() }
```

### PdfRule

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PdfRule {
    pub format: Option<String>,      // Some("compress") = subfolder mode, None = extension

    #[serde(default)]
    pub input_extensions: Vec<String>,

    #[serde(default = "default_pdf_ext")]
    pub output_ext: String,

    #[serde(default)]
    pub mode: PdfMode,

    #[serde(default)]
    pub quality: Option<PdfQuality>,

    #[serde(default)]
    pub pdfa_version: Option<String>,  // "2b", "3b", "4"

    #[serde(default)]
    pub page_range: Option<String>,    // "1-5", "3", "1,3,5"

    #[serde(default)]
    pub resolution: Option<u32>,       // DPI (default 300)

    #[serde(default)]
    pub password: Option<String>,      // for encrypted PDFs

    #[serde(default)]
    pub options: Option<Vec<String>>,  // extra CLI args

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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfQuality {
    Screen,
    Ebook,
    Printer,
    Prepress,
    Default,
}

fn default_pdf_ext() -> String { ".pdf".to_string() }
fn default_pdf_template() -> String { "{base}_converted.{ext}".to_string() }
```

### DocumentRule

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DocumentRule {
    pub format: Option<String>,      // Some("epub") = subfolder mode, None = extension

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

fn default_doc_ext() -> String { ".pdf".to_string() }
fn default_doc_template() -> String { "{base}_converted.{ext}".to_string() }
```

### CustomRule

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CustomRule {
    pub format: Option<String>,      // NEW: Some("compress") = subfolder mode

    #[serde(default)]
    pub input_extensions: Vec<String>,

    pub output_ext: String,

    pub command: String,             // template with {input}, {output}, {basename}, {ext}, {output_folder}

    #[serde(default = "default_custom_template")]
    pub output_name_template: String,

    #[serde(default)]
    pub description: Option<String>,
}

fn default_custom_template() -> String { "{base}_custom.{ext}".to_string() }
```

### Default helpers

```rust
fn default_false() -> bool { false }
fn default_output_ext_default() -> String { String::new() }
```

---

## Module: `src/config/embedded.rs`

The embedded config is a subset of WatchConfig. When a file named `mainconfig.yaml` (or configured name) is found in a folder, it auto-registers that folder as a watcher.

### EmbeddedConfig

```rust
/// Subset of WatchConfig used in embedded mainconfig.yaml files.
/// The watch_folder is always the parent directory of the config file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EmbeddedConfig {
    pub output_folder: String,

    #[serde(default)]
    pub video_watch: bool,

    #[serde(default)]
    pub image_watch: bool,

    #[serde(default)]
    pub audio_watch: bool,

    #[serde(default)]
    pub doc_watch: bool,

    #[serde(default)]
    pub custom_watch: bool,

    #[serde(rename = "video_rules", default)]
    pub video_rules: Vec<VideoRule>,

    #[serde(rename = "image_rules", default)]
    pub image_rules: Vec<ImageRule>,

    #[serde(rename = "audio_rules", default)]
    pub audio_rules: Vec<AudioRule>,

    #[serde(rename = "pdf_rules", default)]
    pub pdf_rules: Vec<PdfRule>,

    #[serde(rename = "document_rules", default)]
    pub document_rules: Vec<DocumentRule>,

    #[serde(rename = "custom_rules", default)]
    pub custom_rules: Vec<CustomRule>,
}

impl EmbeddedConfig {
    /// Converts to a full WatchConfig, using the containing folder as watch_folder.
    pub fn to_watch_config(&self, watch_folder: &str) -> WatchConfig {
        WatchConfig {
            watch_folder: watch_folder.to_string(),
            output_folder: self.output_folder.clone(),
            video_watch: self.video_watch,
            image_watch: self.image_watch,
            audio_watch: self.audio_watch,
            doc_watch: self.doc_watch,
            custom_watch: self.custom_watch,
            video_rules: self.video_rules.clone(),
            image_rules: self.image_rules.clone(),
            audio_rules: self.audio_rules.clone(),
            pdf_rules: self.pdf_rules.clone(),
            document_rules: self.document_rules.clone(),
            custom_rules: self.custom_rules.clone(),
        }
    }
}
```

---

## Module: `src/config/mod.rs`

```rust
pub mod global;
pub mod watch;
pub mod embedded;

use anyhow::Result;
use std::path::Path;

pub fn load_global_config() -> Result<global::GlobalConfig> {
    // Reads config/global.yaml, returns GlobalConfig
}

pub fn load_watch_configs(custom_path: Option<&Path>) -> Result<Vec<watch::WatchConfig>> {
    // If custom_path: load that single file
    // Otherwise: try config/watchers.yaml, then config/watch*.yaml
    // Returns Vec<WatchConfig>
}
```

---

## Matching Logic

In `watcher/monitor.rs`, the `create_job()` function implements universal matching:

### Subfolder Match (any type with `format`)
```
File is in ->format/ subfolder?
  ├─ Check video_rules: match(r.format == format)   → Video
  ├─ Check audio_rules: match(r.format == format)   → Audio
  ├─ Check document_rules: match(r.format == format)→ Document
  ├─ Check pdf_rules: match(r.format == format)     → Pdf
  ├─ Check image_rules: match(r.format == format)   → Image
  └─ Check custom_rules: match(r.format == format)  → External
```

### Extension Match (rules without `format`)
```
Check file extension against:
  ├─ custom_rules (highest priority)
  ├─ video_rules
  ├─ audio_rules
  ├─ pdf_rules
  ├─ document_rules
  └─ image_rules
```

### Folder Creation (create_folders)
```
For each watch_config:
  ├─ Create watch_folder if not exists
  ├─ Create output_folder if not exists
  ├─ If video_watch: for each rule with format → create ->format/ subfolder
  ├─ If audio_watch: for each rule with format → create ->format/ subfolder
  ├─ If doc_watch: for each rule with format → create ->format/ subfolder
  ├─ If custom_watch: for each rule with format → create ->format/ subfolder
  ├─ If image_watch: for each rule with format → create ->format/ subfolder
  └─ (pdf_rules always create subfolders if they have format)
```
