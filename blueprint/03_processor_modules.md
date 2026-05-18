# ConvWatcher — Processor Modules

## Module: `src/processor/mod.rs`

Exports all processor modules:

```rust
pub mod job;
pub mod video;
pub mod image;
pub mod audio;      // NEW
pub mod pdf;        // NEW
pub mod document;   // NEW
pub mod external;
pub mod disk;
pub mod namer;
```

---

## Module: `src/processor/job.rs`

The `ConversionJob` enum is the message type passed from watchers to workers via mpsc channel.

```rust
use std::path::PathBuf;
use crate::config::watch::{VideoRule, ImageRule, AudioRule, PdfRule, DocumentRule, CustomRule};

pub enum ConversionJob {
    Video {
        watcher_name: String,
        file_name: String,
        file_path: PathBuf,
        rule: VideoRule,
        output_folder: String,
        watch_folder: String,
    },
    Image {
        watcher_name: String,
        file_name: String,
        file_path: PathBuf,
        rule: ImageRule,
        output_folder: String,
        watch_folder: String,
    },
    Audio {                          // NEW
        watcher_name: String,
        file_name: String,
        file_path: PathBuf,
        rule: AudioRule,
        output_folder: String,
        watch_folder: String,
    },
    Pdf {                             // NEW
        watcher_name: String,
        file_name: String,
        file_path: PathBuf,
        rule: PdfRule,
        output_folder: String,
        watch_folder: String,
    },
    Document {                        // NEW
        watcher_name: String,
        file_name: String,
        file_path: PathBuf,
        rule: DocumentRule,
        output_folder: String,
        watch_folder: String,
    },
    External {
        watcher_name: String,
        file_name: String,
        file_path: PathBuf,
        rule: CustomRule,
        output_folder: String,
        watch_folder: String,
    },
}
```

---

## Universal Processor Pattern

Every processor function follows this exact signature pattern:

```rust
pub async fn process_<type>(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    rule: &<TypeRule>,
    output_folder: &str,
    watch_folder: &str,
    error_logger: Arc<ErrorLogger>,
    health_server: Arc<HealthServer>,
    disk_config: &DiskSpaceConfig,
) {
    // 1. Check disk space
    if let Err(e) = check_disk_space(output_folder, watch_folder, disk_config).await {
        error!("Disk space check failed: {}", e);
        let _ = health_server.increment_error(&watcher_name);
        return;
    }

    // 2. Mark as processing
    let _ = health_server.set_processing(watcher_name.clone(), file_name.clone());

    // 3. Generate output path using OutputNamer
    let output_folder_path = PathBuf::from(output_folder);
    let base_name = get_base_name(file_name);
    let output_path = match OutputNamer::generate_path(
        &output_folder_path,
        &base_name,
        &rule.output_name_template,
        &rule.output_ext.trim_start_matches('.'),
        &rule.output_ext.trim_start_matches('.'),
    ) {
        Ok(p) => p,
        Err(_) => OutputNamer::generate_with_counter(
            &output_folder_path,
            &base_name,
            "<type>",                          // e.g., "video", "audio", etc.
            &rule.output_ext.trim_start_matches('.'),
        ),
    };

    // 4. Call the type-specific conversion logic
    match convert_internal(file_name, file_path, &output_path, rule).await {
        Ok(()) => {
            info!("Conversion succeeded: {}", file_name);
            let _ = health_server.increment_processed(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "done".to_string(),
                output: output_path.to_string_lossy().to_string(),
            }).await;
        }
        Err(e) => {
            let msg = format!("Conversion failed: {}", e);
            error!("{}", msg);
            error_logger.log(&msg, &file_name, "<type>::process");
            let _ = health_server.increment_error(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "error".to_string(),
                output: String::new(),
            }).await;
        }
    }

    // 5. Clear processing flag
    let _ = health_server.clear_processing(&watcher_name);
}
```

---

## Video Processor (`src/processor/video.rs`)

Uses FFmpeg via `tokio::process::Command`.

### Quality Parsing

```rust
pub fn parse_quality_value(quality_str: &str) -> Vec<String> {
    // Supports:
    // "crf 23"       → ["-crf", "23"]
    // "23"            → ["-crf", "23"]  (implicit CRF)
    // "2M" or "5M"    → ["-b:v", "2M"]  (bitrate mode)
    // "vbr 4"        → ["-q:v", "4"]   (VBR mode)
}
```

### FFmpeg Command Construction

```rust
// Basic structure:
// ffmpeg -i {input} -c:v {codec} {quality_args} -c:a {audio_codec} -b:a {audio_bitrate} {output}

// Example for libx264 CRF 23:
// ffmpeg -i input.mp4 -c:v libx264 -crf 23 -preset medium -c:a aac -b:a 128k output.mp4
```

### Duration Check

After conversion, uses `ffprobe` to compare output duration with input. If `output_duration < input_duration * min_duration_ratio`, marks as failed (truncated output).

```rust
pub async fn get_video_duration(path: &Path) -> Result<f64> {
    // ffprobe -v error -show_entries format=duration -of default=noprint_wrappers=1:nokey=1 {path}
}
```

### Input Extensions

`.mp4`, `.avi`, `.mkv`, `.mov`, `.webm`, `.flv`, `.wmv`, `.mpeg`, `.mpg`, `.ts`, `.mts`

---

## Image Processor (`src/processor/image.rs`)

Uses the Rust `image` crate — pure Rust, no external binary.

### Supported Formats

| Format | Read | Write | Notes |
|--------|------|-------|-------|
| JPEG | Yes | Yes | Quality 0-100, no alpha |
| PNG | Yes | Yes | Quality 0-100 (compression), alpha supported |
| GIF | Yes | Yes | |
| BMP | Yes | Yes | |
| TIFF | Yes | Yes | Alpha supported |
| WebP | Yes | Yes | Quality 0-100, alpha supported |
| ICO | Yes | No | |
| QOI | Yes | Yes | |
| TGA | Yes | No | |

### Processing Logic

```rust
pub async fn process_image(...) {
    // 1. Check disk space
    // 2. Generate output path
    // 3. Open input image
    // 4. Handle transparency based on rule.transparent
    // 5. Save with format-specific encoding settings
    // 6. Update health server
}

fn save_image(
    img: &DynamicImage,
    path: &Path,
    format: ImageFormat,
    quality: u32,
) -> Result<()> {
    // JPEG: set quality on encoder
    // PNG: set compression level based on quality
    // WebP: set quality on encoder
    // Others: use default encoder
}
```

---

## Audio Processor (`src/processor/audio.rs`) — NEW

Uses FFmpeg (same binary as video) via `tokio::process::Command`.

### FFmpeg Command Construction

```rust
// Basic structure:
// ffmpeg -i {input} -vn -c:a {codec} -b:a {bitrate} {sample_rate} {channels} {quality} {output}

// Example: WAV → MP3 320kbps stereo
// ffmpeg -i input.wav -vn -c:a libmp3lame -b:a 320k -ar 44100 -ac 2 output.mp3

// Example: FLAC (lossless)
// ffmpeg -i input.wav -vn -c:a flac output.flac

// Example: Vorbis quality mode
// ffmpeg -i input.wav -vn -c:a libvorbis -q:a 4 output.ogg
```

### Codec Mapping

```rust
// Common audio codecs:
// libmp3lame  → .mp3
// aac         → .m4a, .aac
// libvorbis   → .ogg
// libopus     → .opus
// flac        → .flac
// pcm_s16le   → .wav
// pcm_s24le   → .wav (high bit depth)
// ac3         → .ac3
// eac3        → .eac3
// libfdk_aac  → .m4a (higher quality AAC, if available)
```

### Quality Handling

```rust
fn build_audio_quality_args(rule: &AudioRule) -> Vec<String> {
    let mut args = Vec::new();

    // Video/audio stream selection: drop video streams
    args.push("-vn".to_string());

    // Codec
    args.push("-c:a".to_string());
    args.push(rule.audio_codec.clone());

    // Bitrate (CBR)
    if !rule.audio_bitrate.is_empty() {
        args.push("-b:a".to_string());
        args.push(rule.audio_bitrate.clone());
    }

    // Sample rate
    if let Some(sr) = rule.sample_rate {
        args.push("-ar".to_string());
        args.push(sr.to_string());
    }

    // Channels
    if let Some(ch) = rule.channels {
        args.push("-ac".to_string());
        args.push(ch.to_string());
    }

    // VBR quality override (for codecs that support it)
    if let Some(ref q) = rule.quality {
        // For libmp3lame: -q:a 0-9 (0=best)
        // For libvorbis: -q:a -1 to 10
        // For libopus: -vbr on -b:a (bitrate still used)
        args.push("-q:a".to_string());
        args.push(q.clone());
    }

    args
}
```

### Input Extensions

`.mp3`, `.wav`, `.flac`, `.aac`, `.ogg`, `.opus`, `.wma`, `.m4a`, `.aiff`, `.caf`

---

## PDF Processor (`src/processor/pdf.rs`) — NEW

Uses external tools via `tokio::process::Command`: Ghostscript (`gs`), QPDF (`qpdf`), Poppler utilities, and `img2pdf`.

### Mode Dispatch

```rust
pub async fn process_pdf(...) {
    match rule.mode {
        PdfMode::Compress       => compress_pdf(input, output, rule.quality).await,
        PdfMode::PdfA           => convert_to_pdfa(input, output, &rule.pdfa_version).await,
        PdfMode::ExtractText    => extract_text(input, output).await,
        PdfMode::ExtractImages  => extract_images(input, output, rule.resolution).await,
        PdfMode::ImageToPdf     => images_to_pdf(input, output, rule.resolution).await,
        PdfMode::Merge          => merge_pdfs(input_dir, output).await,  // needs special handling
        PdfMode::Linearize      => linearize_pdf(input, output).await,
        PdfMode::Encrypt        => encrypt_pdf(input, output, &rule.password).await,
        PdfMode::Decrypt        => decrypt_pdf(input, output, &rule.password).await,
        PdfMode::Analyze        => analyze_pdf(input, output).await,
    }
}
```

### Individual Mode Implementations

```rust
/// Compress PDF using Ghostscript
/// gs -sDEVICE=pdfwrite -dCompatibilityLevel=1.4 \
///    -dPDFSETTINGS=/screen|/ebook|/printer|/prepress \
///    -dNOPAUSE -dQUIET -dBATCH -sOutputFile={output} {input}
async fn compress_pdf(input: &Path, output: &Path, quality: Option<&PdfQuality>) -> Result<()> {
    let setting = match quality {
        Some(PdfQuality::Screen)   => "/screen",
        Some(PdfQuality::Ebook)    => "/ebook",
        Some(PdfQuality::Printer)  => "/printer",
        Some(PdfQuality::Prepress) => "/prepress",
        _ => "/default",
    };

    tokio::process::Command::new("gs")
        .args(["-sDEVICE=pdfwrite", "-dCompatibilityLevel=1.4"])
        .arg(format!("-dPDFSETTINGS={}", setting))
        .args(["-dNOPAUSE", "-dQUIET", "-dBATCH"])
        .arg(format!("-sOutputFile={}", output.display()))
        .arg(input.as_os_str())
        .output()
        .await?;
    // Check success
}

/// Convert to PDF/A using Ghostscript
/// gs -sDEVICE=pdfwrite -dPDFA=2 -dPDFACompatibilityPolicy=1 \
///    -sOutputFile={output} {input}
async fn convert_to_pdfa(input: &Path, output: &Path, version: &Option<String>) -> Result<()> {
    let pdfa = match version.as_deref() {
        Some("2b") | None => "2",
        Some("3b") => "3",
        Some("4") => "4",
        _ => "2",
    };

    tokio::process::Command::new("gs")
        .args(["-sDEVICE=pdfwrite", &format!("-dPDFA={}", pdfa)])
        .args(["-dPDFACompatibilityPolicy=1", "-dNOPAUSE", "-dQUIET", "-dBATCH"])
        .arg(format!("-sOutputFile={}", output.display()))
        .arg(input.as_os_str())
        .output()
        .await?;
}

/// Extract text from PDF using pdftotext
/// pdftotext {input} {output}
async fn extract_text(input: &Path, output: &Path) -> Result<()> {
    tokio::process::Command::new("pdftotext")
        .arg(input.as_os_str())
        .arg(output.as_os_str())
        .output()
        .await?;
}

/// Extract images from PDF using pdfimages
/// pdfimages -png -r 300 {input} {output_prefix}
/// Outputs multiple files: {output_prefix}-000.png, {output_prefix}-001.png, etc.
async fn extract_images(input: &Path, output: &Path, resolution: Option<u32>) -> Result<()> {
    let dpi = resolution.unwrap_or(300);
    // Remove extension from output for prefix
    let prefix = output.with_extension("");

    tokio::process::Command::new("pdfimages")
        .arg(format!("-png"))
        .arg(format!("-r {}", dpi))
        .arg(input.as_os_str())
        .arg(prefix.as_os_str())
        .output()
        .await?;
    // Note: output is a directory of images, not a single file
}

/// Convert images to PDF using img2pdf
/// img2pdf {input} -o {output}
async fn images_to_pdf(input: &Path, output: &Path, resolution: Option<u32>) -> Result<()> {
    tokio::process::Command::new("img2pdf")
        .arg(input.as_os_str())
        .arg("-o")
        .arg(output.as_os_str())
        .output()
        .await?;
}

/// Linearize PDF (web-optimize) using QPDF
/// qpdf --linearize {input} {output}
async fn linearize_pdf(input: &Path, output: &Path) -> Result<()> {
    tokio::process::Command::new("qpdf")
        .arg("--linearize")
        .arg(input.as_os_str())
        .arg(output.as_os_str())
        .output()
        .await?;
}

/// Encrypt PDF using QPDF
/// qpdf --encrypt {password} {password} 256 -- {input} {output}
async fn encrypt_pdf(input: &Path, output: &Path, password: &Option<String>) -> Result<()> {
    let pw = password.as_deref().unwrap_or("default");
    tokio::process::Command::new("qpdf")
        .arg("--encrypt")
        .arg(pw).arg(pw)
        .arg("256")
        .arg("--")
        .arg(input.as_os_str())
        .arg(output.as_os_str())
        .output()
        .await?;
}

/// Decrypt PDF using QPDF
/// qpdf --decrypt --password={password} {input} {output}
async fn decrypt_pdf(input: &Path, output: &Path, password: &Option<String>) -> Result<()> {
    let mut cmd = tokio::process::Command::new("qpdf");
    cmd.arg("--decrypt");
    if let Some(pw) = password {
        cmd.arg(format!("--password={}", pw));
    }
    cmd.arg(input.as_os_str()).arg(output.as_os_str());
    cmd.output().await?;
}
```

---

## Document Processor (`src/processor/document.rs`) — NEW

Uses Pandoc via `tokio::process::Command`.

### Pandoc Command Construction

```rust
/// Pandoc auto-detects input format from extension.
/// Output format is specified by the output file extension.
///
/// pandoc {input} -o {output} {options}
///
/// Examples:
///   pandoc input.md -o output.pdf --pdf-engine=wkhtmltopdf --toc
///   pandoc input.docx -o output.epub --toc --css=style.css
///   pandoc input.html -o output.docx --standalone
async fn convert_document(
    input: &Path,
    output: &Path,
    rule: &DocumentRule,
) -> Result<()> {
    let mut cmd = tokio::process::Command::new("pandoc");

    cmd.arg(input.as_os_str());
    cmd.arg("-o");
    cmd.arg(output.as_os_str());

    // TOC
    if rule.toc {
        cmd.arg("--toc");
    }

    // TOC depth
    if let Some(depth) = rule.toc_depth {
        cmd.arg(format!("--toc-depth={}", depth));
    }

    // CSS
    if let Some(ref css) = rule.css {
        cmd.arg(format!("--css={}", css));
    }

    // Template
    if let Some(ref tmpl) = rule.template {
        cmd.arg(format!("--template={}", tmpl));
    }

    // Standalone (-s)
    if rule.standalone {
        cmd.arg("-s");
    }

    // Metadata (-M key=value)
    if let Some(ref meta) = rule.metadata {
        for m in meta {
            cmd.arg(format!("-M{}", m));
        }
    }

    // PDF engine
    if let Some(ref engine) = rule.pdf_engine {
        cmd.arg(format!("--pdf-engine={}", engine));
    }

    // Extra options
    if let Some(ref extra) = rule.options {
        for opt in extra {
            cmd.arg(opt);
        }
    }

    let output_result = cmd.output().await?;

    if output_result.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("Pandoc failed: {}", stderr);
    }
}
```

### Pandoc Format Support

Pandoc supports dozens of formats. The most commonly used:

| Input Formats | Output Formats |
|---------------|---------------|
| Markdown (.md) | PDF (.pdf) — via pdf-engine |
| DOCX (.docx) | EPUB (.epub) |
| ODT (.odt) | DOCX (.docx) |
| HTML (.html) | ODT (.odt) |
| EPUB (.epub) | HTML (.html) |
| LaTeX (.tex) | Markdown (.md) |
| Org (.org) | LaTeX (.tex) |
| Textile (.textile) | AsciiDoc (.asciidoc) |
| ReStructuredText (.rst) | RTF (.rtf) |
| CommonMark (.md) | Man page, groff, etc. |

---

## External Processor (`src/processor/external.rs`)

Unchanged from v1. Executes arbitrary commands with template substitution.

### Template Placeholders

| Placeholder | Replaced With |
|-------------|---------------|
| `{input}` | Full path to input file |
| `{output}` | Full path to generated output path |
| `{basename}` | Input filename without extension |
| `{ext}` | Output extension (without dot) |
| `{output_folder}` | Output folder path |

### Security Validations

```rust
fn validate_command_template(template: &str) -> Result<()> {
    // Blocks: ".." in program path
    // Requires absolute path or "./" if path contains "/"
}

fn validate_placeholder_values(...) -> Result<()> {
    // Blocks: ";", "&&", "||", "|", "`", "$(", "\n", "\r"
    // Blocks: ".." in paths
}
```

---

## Disk Space Monitor (`src/processor/disk.rs`)

```rust
/// Check if there's enough disk space
pub async fn check_disk_space(
    output_folder: &str,
    watch_folder: &str,
    config: &DiskSpaceConfig,
) -> Result<()> {
    // Get available space on output and/or watch mount points
    // Compare against threshold
    // Return Ok if enough, Err if low
}

/// Background task that periodically checks disk space
/// Halts processing (creates LOW_SPACE.txt warning files) when low
/// Resumes (clears flags) when space recovers
pub async fn disk_space_monitor(
    config: DiskSpaceConfig,
    output_folders: Vec<String>,
    watch_folders: Vec<String>,
) {
    // Loop every check_interval_s
    // Check all output + watch folders
    // Create LOW_DISK_SPACE.txt in affected folders
    // Remove it when space recovers
}
```

---

## Output Namer (`src/processor/namer.rs`)

```rust
pub struct OutputNamer;

impl OutputNamer {
    /// Generate output path from template
    /// Template variables: {base}, {codec}, {num}, {ext}
    pub fn generate_path(
        output_folder: &Path,
        base_name: &str,
        template: &str,
        codec: &str,
        ext: &str,
    ) -> Result<PathBuf> {
        let filename = template
            .replace("{base}", base_name)
            .replace("{codec}", codec)
            .replace("{num}", "0")
            .replace("{ext}", ext);
        Ok(output_folder.join(&filename))
    }

    /// Generate path with counter to avoid collisions
    pub fn generate_with_counter(
        output_folder: &Path,
        base_name: &str,
        codec: &str,
        ext: &str,
    ) -> PathBuf {
        // Try {base}_{codec}_{counter}.{ext} for counter 0, 1, 2...
        // Returns first path that doesn't exist
    }
}
```
