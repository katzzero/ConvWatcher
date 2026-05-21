pub mod codec_registry;
pub mod global;
pub mod watch;
pub mod embedded;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::{info, warn};

use codec_registry::{CodecRegistry, resolve_watchers};
use global::GlobalConfig;
use watch::{WatchConfig, WatchType};

const CONFIG_PATH: &str = "config/config.yaml";

fn invalidate_and_replace(path: &Path, default_yaml: &str, label: &str) {
    let invalid_path = path.with_extension("yaml.invalid");
    if let Err(e) = fs::rename(path, &invalid_path) {
        warn!("Failed to rename invalid {} {:?}: {}", label, path, e);
    } else {
        warn!(
            "{} {:?} is incompatible — renamed to {:?}",
            label, path, invalid_path
        );
    }
    if let Err(e) = fs::write(path, default_yaml) {
        warn!("Failed to write default {}: {}", label, e);
    } else {
        info!("Created fresh default {}: {}", label, path.display());
    }
}

/// Load all configuration from a single config.yaml file.
/// Returns (GlobalConfig, Vec<WatchConfig>, CodecRegistry).
pub fn load_config(custom_path: Option<&Path>) -> Result<(GlobalConfig, Vec<WatchConfig>, CodecRegistry)> {
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

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", path.display()))?;

    #[derive(serde::Deserialize)]
    struct ConfigFile {
        global: Option<GlobalConfig>,
        watchers: Vec<WatchConfig>,
    }

    let config_file: ConfigFile = match serde_yaml::from_str(&content) {
        Ok(cf) => cf,
        Err(e) => {
            warn!("Failed to parse {}: {}", path.display(), e);
            let default = generate_default_config(&path)?;
            return Ok(default);
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
    ].iter().all(|p| p.exists());

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
    create_directories(&global, &watchers)?;

    Ok((global, watchers, registry))
}

fn validate_config(global: &GlobalConfig, watchers: &[WatchConfig]) -> Result<()> {
    // Validate global paths
    validate_absolute_path(&global.log.errors_file, "log.errors_file")?;
    validate_absolute_path(&global.history.file, "history.file")?;

    // Validate watcher paths
    for watcher in watchers {
        if watcher.watch_folder.is_empty() {
            anyhow::bail!(
                "Watcher '{}': watch_folder must be an absolute path, got empty string",
                watcher.name
            );
        }
        validate_absolute_path(&watcher.watch_folder, &format!("watcher '{}'.watch_folder", watcher.name))?;

        if watcher.output_folder.is_empty() {
            anyhow::bail!(
                "Watcher '{}': output_folder must be an absolute path, got empty string",
                watcher.name
            );
        }
        validate_absolute_path(&watcher.output_folder, &format!("watcher '{}'.output_folder", watcher.name))?;

        // Validate rules have non-empty input_extensions
        match &watcher.watch_type {
            WatchType::Video { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', video rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
            WatchType::Image { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', image rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
            WatchType::Audio { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', audio rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
            WatchType::Pdf { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', pdf rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
            WatchType::Document { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', document rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
            WatchType::Custom { rules } => {
                for (i, rule) in rules.iter().enumerate() {
                    if rule.input_extensions.is_empty() {
                        anyhow::bail!(
                            "Watcher '{}', custom rule {}: input_extensions must not be empty",
                            watcher.name, i
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

fn validate_absolute_path(path: &str, label: &str) -> Result<()> {
    let p = Path::new(path);
    if !p.is_absolute() {
        anyhow::bail!(
            "{}: must be an absolute path, got '{}'. Example: /app/inputs/default/",
            label, path
        );
    }
    Ok(())
}

fn create_directories(global: &GlobalConfig, watchers: &[WatchConfig]) -> Result<()> {
    // Create log directory
    if let Some(parent) = Path::new(&global.log.errors_file).parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create log directory {}", parent.display()))?;
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

    // Create embedded configs directory
    fs::create_dir_all("config/watchs")
        .with_context(|| "Cannot create config/watchs directory")?;

    Ok(())
}

/// Generate default config files on first run.
fn generate_default_config(config_path: &Path) -> Result<(GlobalConfig, Vec<WatchConfig>, CodecRegistry)> {
    let config_dir = config_path.parent().unwrap_or(Path::new("config"));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let abs = cwd.canonicalize().unwrap_or(cwd);

    let inputs_base = abs.join("inputs");
    let outputs_base = abs.join("outputs");
    let logs_base = abs.join("logs");

    let default_global = GlobalConfig {
        log: global::LogConfig {
            errors_file: logs_base.join("errors.log").to_string_lossy().to_string(),
            ..Default::default()
        },
        healthcheck: global::HealthcheckConfig::default(),
        disk_space: global::DiskSpaceConfig::default(),
        history: global::HistoryConfig {
            file: logs_base.join("history.json").to_string_lossy().to_string(),
            ..Default::default()
        },
        ..Default::default()
    };

    let default_watchers = vec![WatchConfig {
        name: "default".to_string(),
        watch_folder: inputs_base.join("default").to_string_lossy().to_string() + "/",
        output_folder: outputs_base.join("default-output").to_string_lossy().to_string() + "/",
        subfolders: vec![
            watch::Subfolder {
                name: "gpu".to_string(),
                description: Some("GPU-accelerated encoding".to_string()),
            },
            watch::Subfolder {
                name: "archive".to_string(),
                description: Some("High-quality archival".to_string()),
            },
        ],
        watch_type: WatchType::Video {
            rules: vec![
                watch::VideoRule {
                    preset: "h264_cpu".to_string(),
                    subfolder: None,
                    input_extensions: vec![
                        ".mp4".into(), ".avi".into(), ".mkv".into(), ".mov".into(),
                        ".webm".into(), ".flv".into(), ".wmv".into(), ".mpeg".into(),
                        ".mpg".into(), ".ts".into(), ".mts".into(), ".mxf".into(),
                    ],
                    output_ext: None,
                    codec: None,
                    quality: None,
                    audio_codec: None,
                    audio_bitrate: None,
                    output_name: None,
                    check_duration: Some(true),
                    min_duration_ratio: Some(0.9),
                },
                watch::VideoRule {
                    preset: "h264_nvenc".to_string(),
                    subfolder: Some("gpu".to_string()),
                    input_extensions: vec![".mxf".into(), ".mts".into(), ".mov".into()],
                    output_ext: None,
                    codec: None,
                    quality: None,
                    audio_codec: None,
                    audio_bitrate: None,
                    output_name: None,
                    check_duration: Some(true),
                    min_duration_ratio: Some(0.9),
                },
                watch::VideoRule {
                    preset: "h265_cpu-high".to_string(),
                    subfolder: Some("archive".to_string()),
                    input_extensions: vec![".mxf".into(), ".mts".into(), ".mov".into(), ".mkv".into()],
                    output_ext: None,
                    codec: None,
                    quality: None,
                    audio_codec: None,
                    audio_bitrate: None,
                    output_name: None,
                    check_duration: Some(true),
                    min_duration_ratio: Some(0.95),
                },
            ],
        },
    }];

    let default_yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping({
        let mut map = serde_yaml::Mapping::new();
        map.insert(
            serde_yaml::Value::String("global".into()),
            serde_yaml::to_value(&default_global).unwrap(),
        );
        map.insert(
            serde_yaml::Value::String("watchers".into()),
            serde_yaml::to_value(&default_watchers).unwrap(),
        );
        map
    }))?;

    fs::write(config_path, &default_yaml)
        .with_context(|| format!("Cannot write {}", config_path.display()))?;
    info!("Created default config: {}", config_path.display());

    // Generate codec preset files
    generate_preset_files(config_dir)?;

    // Load the registry from generated files
    let registry = CodecRegistry::load(config_dir, &default_global.codec_presets)?;

    // Resolve watchers
    let resolved = resolve_watchers(default_watchers, &registry)?;

    // Validate and create directories
    validate_config(&default_global, &resolved)?;
    create_directories(&default_global, &resolved)?;

    Ok((default_global, resolved, registry))
}

fn generate_preset_files(config_dir: &Path) -> Result<()> {
    // Generate video_codecs.yaml
    let video_path = config_dir.join("video_codecs.yaml");
    if !video_path.exists() {
        fs::write(&video_path, DEFAULT_VIDEO_PRESETS)
            .with_context(|| format!("Cannot write {}", video_path.display()))?;
        info!("Created default video presets: {}", video_path.display());
    }

    // Generate audio_codecs.yaml
    let audio_path = config_dir.join("audio_codecs.yaml");
    if !audio_path.exists() {
        fs::write(&audio_path, DEFAULT_AUDIO_PRESETS)
            .with_context(|| format!("Cannot write {}", audio_path.display()))?;
        info!("Created default audio presets: {}", audio_path.display());
    }

    // Generate image_codecs.yaml
    let image_path = config_dir.join("image_codecs.yaml");
    if !image_path.exists() {
        fs::write(&image_path, DEFAULT_IMAGE_PRESETS)
            .with_context(|| format!("Cannot write {}", image_path.display()))?;
        info!("Created default image presets: {}", image_path.display());
    }

    // Generate pdf_presets.yaml
    let pdf_path = config_dir.join("pdf_presets.yaml");
    if !pdf_path.exists() {
        fs::write(&pdf_path, DEFAULT_PDF_PRESETS)
            .with_context(|| format!("Cannot write {}", pdf_path.display()))?;
        info!("Created default PDF presets: {}", pdf_path.display());
    }

    // Generate document_presets.yaml
    let doc_path = config_dir.join("document_presets.yaml");
    if !doc_path.exists() {
        fs::write(&doc_path, DEFAULT_DOCUMENT_PRESETS)
            .with_context(|| format!("Cannot write {}", doc_path.display()))?;
        info!("Created default document presets: {}", doc_path.display());
    }

    Ok(())
}

// Default preset file contents (generated on first run)
const DEFAULT_VIDEO_PRESETS: &str = r#"# Video codec presets — reference by name in watcher rules.
# Each preset defines: codec, quality, audio settings, output container.

presets:
  # ── CPU Encoding ──
  h264_cpu:
    codec: libx264
    quality: crf 23
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
    description: "H.264 CPU — general purpose, wide compatibility"

  h264_cpu-high:
    codec: libx264
    quality: crf 18
    audio_codec: aac
    audio_bitrate: 192k
    output_ext: .mp4
    description: "H.264 CPU — high quality, archival"

  h265_cpu:
    codec: libx265
    quality: crf 28
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
    description: "H.265/HEVC CPU — smaller files, slower"

  h265_cpu-high:
    codec: libx265
    quality: crf 22
    audio_codec: aac
    audio_bitrate: 192k
    output_ext: .mkv
    description: "H.265/HEVC CPU — high quality archival"

  vp9_cpu:
    codec: libvpx-vp9
    quality: crf 30
    audio_codec: libopus
    audio_bitrate: 128k
    output_ext: .webm
    description: "VP9 CPU — web streaming"

  av1_cpu:
    codec: libaom-av1
    quality: crf 30
    audio_codec: libopus
    audio_bitrate: 128k
    output_ext: .mp4
    description: "AV1 CPU — best compression, very slow"

  # ── VAAPI (Intel / AMD integrated GPU) ──
  h264_vaapi:
    codec: h264_vaapi
    quality: qp 25
    audio_codec: copy
    output_ext: .mp4
    description: "H.264 VAAPI — Intel/AMD GPU hardware encoding"

  h265_vaapi:
    codec: hevc_vaapi
    quality: qp 28
    audio_codec: copy
    output_ext: .mkv
    description: "H.265 VAAPI — Intel/AMD GPU, smaller files"

  vp9_vaapi:
    codec: vp9_vaapi
    quality: qp 28
    audio_codec: copy
    output_ext: .webm
    description: "VP9 VAAPI — web streaming via GPU"

  av1_vaapi:
    codec: av1_vaapi
    quality: qp 30
    audio_codec: copy
    output_ext: .mp4
    description: "AV1 VAAPI — best compression via GPU (Arc / newer Intel)"

  # ── NVENC (NVIDIA GPU) ──
  h264_nvenc:
    codec: h264_nvenc
    quality: cq 23
    audio_codec: copy
    output_ext: .mp4
    description: "H.264 NVENC — NVIDIA GPU, fast"

  h264_nvenc-high:
    codec: h264_nvenc
    quality: cq 18
    audio_codec: copy
    output_ext: .mp4
    description: "H.264 NVENC — NVIDIA GPU, high quality"

  h265_nvenc:
    codec: hevc_nvenc
    quality: cq 28
    audio_codec: copy
    output_ext: .mkv
    description: "H.265 NVENC — NVIDIA GPU, smaller files"

  h265_nvenc-high:
    codec: hevc_nvenc
    quality: cq 22
    audio_codec: copy
    output_ext: .mkv
    description: "H.265 NVENC — NVIDIA GPU, high quality"

  av1_nvenc:
    codec: av1_nvenc
    quality: cq 28
    audio_codec: copy
    output_ext: .mp4
    description: "AV1 NVENC — NVIDIA GPU (RTX 40-series+)"

  # ── AMF (AMD GPU) ──
  h264_amf:
    codec: h264_amf
    quality: qp_i 25
    audio_codec: copy
    output_ext: .mp4
    description: "H.264 AMF — AMD GPU hardware encoding"

  h265_amf:
    codec: hevc_amf
    quality: qp_i 28
    audio_codec: copy
    output_ext: .mkv
    description: "H.265 AMF — AMD GPU, smaller files"

  av1_amf:
    codec: av1_amf
    quality: qp_i 28
    audio_codec: copy
    output_ext: .mp4
    description: "AV1 AMF — AMD GPU (RX 7000+)"

  # ── QSV (Intel QuickSync via MediaSDK) ──
  h264_qsv:
    codec: h264_qsv
    quality: qp 25
    audio_codec: copy
    output_ext: .mp4
    description: "H.264 QuickSync — Intel GPU via MediaSDK"

  h265_qsv:
    codec: hevc_qsv
    quality: qp 28
    audio_codec: copy
    output_ext: .mkv
    description: "H.265 QuickSync — Intel GPU via MediaSDK"

  vp9_qsv:
    codec: vp9_qsv
    quality: qp 28
    audio_codec: copy
    output_ext: .webm
    description: "VP9 QuickSync — Intel GPU"

  av1_qsv:
    codec: av1_qsv
    quality: qp 30
    audio_codec: copy
    output_ext: .mp4
    description: "AV1 QuickSync — Intel GPU (Arc / 12th gen+)"

  # ── VideoToolbox (macOS) ──
  h264_videotoolbox:
    codec: h264_videotoolbox
    quality: constant_bit_rate 3000
    audio_codec: copy
    output_ext: .mp4
    description: "H.264 VideoToolbox — macOS hardware encoding"

  h265_videotoolbox:
    codec: hevc_videotoolbox
    quality: constant_bit_rate 5000
    audio_codec: copy
    output_ext: .mp4
    description: "H.265 VideoToolbox — macOS hardware encoding"

  # ── OMX (Raspberry Pi) ──
  h264_omx:
    codec: h264_omx
    quality: qp 25
    audio_codec: copy
    output_ext: .mp4
    description: "H.264 OMX — Raspberry Pi hardware encoding"

  # ── Legacy ──
  mpeg4:
    codec: mpeg4
    quality: qscale 4
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .avi
    description: "MPEG-4 Part 2 — legacy compatibility"

  mpeg2:
    codec: mpeg2video
    quality: qscale 4
    audio_codec: mp2
    audio_bitrate: 192k
    output_ext: .mpg
    description: "MPEG-2 — DVD / broadcast"

  # ── Pass-through ──
  copy_video:
    codec: copy
    audio_codec: copy
    output_ext: .mp4
    description: "Stream copy — no re-encoding, remux only"

  copy_video_aac:
    codec: copy
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
    description: "Copy video, re-encode audio to AAC"
"#;

const DEFAULT_AUDIO_PRESETS: &str = r#"# Audio codec presets — reference by name in watcher rules.

presets:
  # ── Lossy ──
  mp3_128:
    audio_codec: libmp3lame
    audio_bitrate: 128k
    output_ext: .mp3
    description: "MP3 128kbps — standard quality"

  mp3_192:
    audio_codec: libmp3lame
    audio_bitrate: 192k
    output_ext: .mp3
    description: "MP3 192kbps — good quality"

  mp3_320:
    audio_codec: libmp3lame
    audio_bitrate: 320k
    output_ext: .mp3
    description: "MP3 320kbps — maximum MP3 quality"

  aac_128:
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .m4a
    description: "AAC 128kbps — standard quality"

  aac_192:
    audio_codec: aac
    audio_bitrate: 192k
    output_ext: .m4a
    description: "AAC 192kbps — good quality"

  aac_256:
    audio_codec: aac
    audio_bitrate: 256k
    output_ext: .m4a
    description: "AAC 256kbps — high quality"

  opus_96:
    audio_codec: libopus
    audio_bitrate: 96k
    output_ext: .opus
    description: "Opus 96kbps — efficient, voice"

  opus_128:
    audio_codec: libopus
    audio_bitrate: 128k
    output_ext: .opus
    description: "Opus 128kbps — efficient, general purpose"

  opus_192:
    audio_codec: libopus
    audio_bitrate: 192k
    output_ext: .opus
    description: "Opus 192kbps — efficient, music"

  vorbis_192:
    audio_codec: libvorbis
    audio_bitrate: 192k
    output_ext: .ogg
    description: "Vorbis 192kbps — open source"

  ac3_192:
    audio_codec: ac3
    audio_bitrate: 192k
    output_ext: .ac3
    description: "AC3 192kbps — Dolby Digital"

  ac3_6ch:
    audio_codec: ac3
    audio_bitrate: 448k
    channels: 6
    output_ext: .ac3
    description: "AC3 5.1 surround — 448kbps"

  # ── Lossless ──
  flac:
    audio_codec: flac
    output_ext: .flac
    description: "FLAC — lossless compression"

  alac:
    audio_codec: alac
    output_ext: .m4a
    description: "ALAC — Apple lossless"

  pcm_s16le:
    audio_codec: pcm_s16le
    output_ext: .wav
    description: "PCM 16-bit — uncompressed WAV"

  # ── Pass-through ──
  copy_audio:
    audio_codec: copy
    output_ext: .mp3
    description: "Stream copy — no re-encoding"
"#;

const DEFAULT_IMAGE_PRESETS: &str = r#"# Image format presets — reference by name in watcher rules.

presets:
  png:
    output_ext: .png
    quality: 90
    description: "PNG — lossless, supports transparency"

  jpeg_90:
    output_ext: .jpg
    quality: 90
    description: "JPEG 90% — high quality"

  jpeg_80:
    output_ext: .jpg
    quality: 80
    description: "JPEG 80% — balanced quality/size"

  jpeg_70:
    output_ext: .jpg
    quality: 70
    description: "JPEG 70% — web optimized"

  webp_90:
    output_ext: .webp
    quality: 90
    description: "WebP 90% — modern web format"

  webp_80:
    output_ext: .webp
    quality: 80
    description: "WebP 80% — balanced"

  webp_lossless:
    output_ext: .webp
    quality: 100
    description: "WebP lossless"

  avif_90:
    output_ext: .avif
    quality: 90
    description: "AVIF 90% — next-gen format, best compression"

  avif_80:
    output_ext: .avif
    quality: 80
    description: "AVIF 80% — balanced"

  heif:
    output_ext: .heic
    quality: 90
    description: "HEIF/HEIC — Apple format"

  tiff:
    output_ext: .tiff
    description: "TIFF — uncompressed, archival"

  gif:
    output_ext: .gif
    description: "GIF — animated, 256 colors"
"#;

const DEFAULT_PDF_PRESETS: &str = r#"# PDF processing presets — reference by name in watcher rules.

presets:
  # ── Compression ──
  pdf_screen:
    mode: compress
    pdf_quality: screen
    output_ext: .pdf
    description: "PDF — screen quality, smallest size (72dpi)"

  pdf_ebook:
    mode: compress
    pdf_quality: ebook
    output_ext: .pdf
    description: "PDF — ebook quality (150dpi)"

  pdf_printer:
    mode: compress
    pdf_quality: printer
    output_ext: .pdf
    description: "PDF — print quality (300dpi)"

  pdf_prepress:
    mode: compress
    pdf_quality: prepress
    output_ext: .pdf
    description: "PDF — prepress quality (300dpi, color preserved)"

  # ── PDF/A (archival) ──
  pdfa_1b:
    mode: pdf_a
    pdfa_version: "1B"
    output_ext: .pdf
    description: "PDF/A-1b — archival standard"

  pdfa_2b:
    mode: pdf_a
    pdfa_version: "2B"
    output_ext: .pdf
    description: "PDF/A-2b — modern archival"

  # ── Extraction ──
  pdf_extract_text:
    mode: extract_text
    output_ext: .txt
    description: "Extract text from PDF"

  pdf_extract_images:
    mode: extract_images
    output_ext: .png
    description: "Extract images from PDF"

  # ── Conversion ──
  image_to_pdf:
    mode: image_to_pdf
    output_ext: .pdf
    description: "Images to PDF"

  pdf_to_images:
    mode: pdf_to_images
    output_ext: .png
    resolution: 150
    description: "PDF pages to images (150dpi)"

  pdf_to_images_hd:
    mode: pdf_to_images
    output_ext: .png
    resolution: 300
    description: "PDF pages to images (300dpi)"

  # ── Optimization ──
  pdf_linearize:
    mode: linearize
    output_ext: .pdf
    description: "Fast web view (linearized PDF)"

  pdf_merge:
    mode: merge
    output_ext: .pdf
    description: "Merge multiple PDFs"

  # ── Security ──
  pdf_encrypt:
    mode: encrypt
    output_ext: .pdf
    description: "Encrypt PDF with password"

  pdf_decrypt:
    mode: decrypt
    output_ext: .pdf
    description: "Remove PDF encryption"

  # ── Analysis ──
  pdf_analyze:
    mode: analyze
    description: "Analyze PDF structure (no output file)"
"#;

const DEFAULT_DOCUMENT_PRESETS: &str = r#"# Document conversion presets — reference by name in watcher rules.

presets:
  # ── To PDF ──
  docx_to_pdf:
    output_ext: .pdf
    pdf_engine: weasyprint
    description: "DOCX to PDF via WeasyPrint"

  md_to_pdf:
    output_ext: .pdf
    pdf_engine: weasyprint
    toc: true
    description: "Markdown to PDF with table of contents"

  html_to_pdf:
    output_ext: .pdf
    pdf_engine: weasyprint
    description: "HTML to PDF"

  odt_to_pdf:
    output_ext: .pdf
    pdf_engine: weasyprint
    description: "ODT to PDF"

  epub_to_pdf:
    output_ext: .pdf
    pdf_engine: weasyprint
    description: "EPUB to PDF"

  # ── To Markdown ──
  docx_to_md:
    output_ext: .md
    standalone: true
    description: "DOCX to Markdown"

  html_to_md:
    output_ext: .md
    standalone: true
    description: "HTML to Markdown"

  # ── To HTML ──
  docx_to_html:
    output_ext: .html
    standalone: true
    description: "DOCX to standalone HTML"

  md_to_html:
    output_ext: .html
    standalone: true
    description: "Markdown to HTML"

  # ── To EPUB ──
  docx_to_epub:
    output_ext: .epub
    description: "DOCX to EPUB"

  md_to_epub:
    output_ext: .epub
    description: "Markdown to EPUB"

  # ── To DOCX ──
  md_to_docx:
    output_ext: .docx
    description: "Markdown to DOCX"

  html_to_docx:
    output_ext: .docx
    description: "HTML to DOCX"
"#;

#[cfg(test)]
mod tests {
    use super::*;

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
            watch_type: WatchType::Video {
                rules: vec![],
            },
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
            watch_type: WatchType::Video {
                rules: vec![],
            },
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
                    preset: "h264_cpu".to_string(),
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
                    preset: "h264_cpu".to_string(),
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
                        preset: "h264_cpu".to_string(),
                        subfolder: None,
                        input_extensions: vec![".mp4".into()],
                        output_ext: None, codec: None, quality: None,
                        audio_codec: None, audio_bitrate: None,
                        output_name: None, check_duration: None,
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
                        output_ext: None, quality: None, transparent: None,
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
                        output_ext: None, audio_codec: None, audio_bitrate: None,
                        sample_rate: None, channels: None, output_name: None,
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
                        output_ext: None, mode: None, quality: None,
                        pdfa_version: None, resolution: None, password: None,
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
                        output_ext: None, toc: None, toc_depth: None,
                        css: None, template: None, standalone: None,
                        metadata: None, pdf_engine: None, options: None,
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
                        output_ext: None, command: None, output_name: None,
                        description: None,
                    }],
                },
            },
        ];
        let result = validate_config(&global, &watchers);
        assert!(result.is_ok());
    }
}
