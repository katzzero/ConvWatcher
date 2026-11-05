//! Default codec-preset files (video/audio/image/pdf/document/custom) and
//! their embedded YAML contents, generated on first run.

use std::path::Path;

use anyhow::{Context, Result};
use log::info;

use super::generate::write_secure_file;

pub(crate) fn generate_preset_files(config_dir: &Path) -> Result<()> {
    // Generate video_codecs.yaml
    let video_path = config_dir.join("video_codecs.yaml");
    if !video_path.exists() {
        write_secure_file(&video_path, DEFAULT_VIDEO_PRESETS)
            .with_context(|| format!("Cannot write {}", video_path.display()))?;
        info!("Created default video presets: {}", video_path.display());
    }

    // Generate audio_codecs.yaml
    let audio_path = config_dir.join("audio_codecs.yaml");
    if !audio_path.exists() {
        write_secure_file(&audio_path, DEFAULT_AUDIO_PRESETS)
            .with_context(|| format!("Cannot write {}", audio_path.display()))?;
        info!("Created default audio presets: {}", audio_path.display());
    }

    // Generate image_codecs.yaml
    let image_path = config_dir.join("image_codecs.yaml");
    if !image_path.exists() {
        write_secure_file(&image_path, DEFAULT_IMAGE_PRESETS)
            .with_context(|| format!("Cannot write {}", image_path.display()))?;
        info!("Created default image presets: {}", image_path.display());
    }

    // Generate pdf_presets.yaml
    let pdf_path = config_dir.join("pdf_presets.yaml");
    if !pdf_path.exists() {
        write_secure_file(&pdf_path, DEFAULT_PDF_PRESETS)
            .with_context(|| format!("Cannot write {}", pdf_path.display()))?;
        info!("Created default PDF presets: {}", pdf_path.display());
    }

    // Generate document_presets.yaml
    let doc_path = config_dir.join("document_presets.yaml");
    if !doc_path.exists() {
        write_secure_file(&doc_path, DEFAULT_DOCUMENT_PRESETS)
            .with_context(|| format!("Cannot write {}", doc_path.display()))?;
        info!("Created default document presets: {}", doc_path.display());
    }

    // Generate custom_presets.yaml
    let custom_path = config_dir.join("custom_presets.yaml");
    if !custom_path.exists() {
        write_secure_file(&custom_path, DEFAULT_CUSTOM_PRESETS)
            .with_context(|| format!("Cannot write {}", custom_path.display()))?;
        info!("Created default custom presets: {}", custom_path.display());
    }

    Ok(())
}
// Default preset file contents (generated on first run)
const DEFAULT_VIDEO_PRESETS: &str = r#"# Video codec presets — reference by name in watcher rules.
# Preset names match the FFmpeg codec name for clarity.

presets:
  # ── CPU Encoding ──
  libx264:
    codec: libx264
    quality: crf 23
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
    description: "libx264 H.264/AVC — crf 23, aac 128k, .mp4 — general purpose"

  libx264_high:
    codec: libx264
    quality: crf 18
    audio_codec: aac
    audio_bitrate: 192k
    output_ext: .mp4
    description: "libx264 H.264/AVC — crf 18, aac 192k, .mp4 — high quality archival"

  libx265:
    codec: libx265
    quality: crf 28
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
    description: "libx265 H.265/HEVC — crf 28, aac 128k, .mp4 — smaller files, slower"

  libx265_high:
    codec: libx265
    quality: crf 22
    audio_codec: aac
    audio_bitrate: 192k
    output_ext: .mkv
    description: "libx265 H.265/HEVC — crf 22, aac 192k, .mkv — high quality archival"

  libvpx-vp9:
    codec: libvpx-vp9
    quality: crf 30
    audio_codec: libopus
    audio_bitrate: 128k
    output_ext: .webm
    description: "libvpx-vp9 VP9 — crf 30, libopus 128k, .webm — web streaming"

  libaom-av1:
    codec: libaom-av1
    quality: crf 30
    audio_codec: libopus
    audio_bitrate: 128k
    output_ext: .mp4
    description: "libaom-av1 AV1 — crf 30, libopus 128k, .mp4 — best compression, very slow"

  # ── VAAPI (Intel / AMD integrated GPU) ──
  h264_vaapi:
    codec: h264_vaapi
    quality: qp 25
    audio_codec: copy
    output_ext: .mp4
    description: "h264_vaapi H.264/AVC — qp 25, copy audio, .mp4 — Intel/AMD GPU"

  hevc_vaapi:
    codec: hevc_vaapi
    quality: qp 28
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_vaapi H.265/HEVC — qp 28, copy audio, .mkv — Intel/AMD GPU"

  vp9_vaapi:
    codec: vp9_vaapi
    quality: qp 28
    audio_codec: libopus
    audio_bitrate: 128k
    output_ext: .webm
    description: "vp9_vaapi VP9 — qp 28, libopus 128k, .webm — web streaming via GPU"

  av1_vaapi:
    codec: av1_vaapi
    quality: qp 30
    audio_codec: copy
    output_ext: .mp4
    description: "av1_vaapi AV1 — qp 30, copy audio, .mp4 — best compression via GPU (Arc / newer Intel)"

  # ── NVENC (NVIDIA GPU) ──
  h264_nvenc:
    codec: h264_nvenc
    quality: cq 23
    audio_codec: copy
    output_ext: .mp4
    description: "h264_nvenc H.264/AVC — cq 23, copy audio, .mp4 — NVIDIA GPU fast"

  h264_nvenc_high:
    codec: h264_nvenc
    quality: cq 18
    audio_codec: copy
    output_ext: .mp4
    description: "h264_nvenc H.264/AVC — cq 18, copy audio, .mp4 — NVIDIA GPU high quality"

  hevc_nvenc:
    codec: hevc_nvenc
    quality: cq 28
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_nvenc H.265/HEVC — cq 28, copy audio, .mkv — NVIDIA GPU"

  hevc_nvenc_high:
    codec: hevc_nvenc
    quality: cq 22
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_nvenc H.265/HEVC — cq 22, copy audio, .mkv — NVIDIA GPU high quality"

  av1_nvenc:
    codec: av1_nvenc
    quality: cq 28
    audio_codec: copy
    output_ext: .mp4
    description: "av1_nvenc AV1 — cq 28, copy audio, .mp4 — NVIDIA GPU (RTX 40-series+)"

  # ── AMF (AMD GPU) ──
  h264_amf:
    codec: h264_amf
    quality: qp_i 25
    audio_codec: copy
    output_ext: .mp4
    description: "h264_amf H.264/AVC — qp_i 25, copy audio, .mp4 — AMD GPU"

  hevc_amf:
    codec: hevc_amf
    quality: qp_i 28
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_amf H.265/HEVC — qp_i 28, copy audio, .mkv — AMD GPU"

  av1_amf:
    codec: av1_amf
    quality: qp_i 28
    audio_codec: copy
    output_ext: .mp4
    description: "av1_amf AV1 — qp_i 28, copy audio, .mp4 — AMD GPU (RX 7000+)"

  # ── QSV (Intel QuickSync via MediaSDK) ──
  h264_qsv:
    codec: h264_qsv
    quality: qp 25
    audio_codec: copy
    output_ext: .mp4
    description: "h264_qsv H.264/AVC — qp 25, copy audio, .mp4 — Intel QuickSync"

  hevc_qsv:
    codec: hevc_qsv
    quality: qp 28
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_qsv H.265/HEVC — qp 28, copy audio, .mkv — Intel QuickSync"

  vp9_qsv:
    codec: vp9_qsv
    quality: qp 28
    audio_codec: libopus
    audio_bitrate: 128k
    output_ext: .webm
    description: "vp9_qsv VP9 — qp 28, libopus 128k, .webm — Intel QuickSync"

  av1_qsv:
    codec: av1_qsv
    quality: qp 30
    audio_codec: copy
    output_ext: .mp4
    description: "av1_qsv AV1 — qp 30, copy audio, .mp4 — Intel QuickSync (Arc / 12th gen+)"

  # ── VideoToolbox (macOS) ──
  h264_videotoolbox:
    codec: h264_videotoolbox
    quality: constant_bit_rate 3000k
    audio_codec: copy
    output_ext: .mp4
    description: "h264_videotoolbox H.264/AVC — b:v 3000k, copy audio, .mp4 — macOS VideoToolbox"

  hevc_videotoolbox:
    codec: hevc_videotoolbox
    quality: constant_bit_rate 5000k
    audio_codec: copy
    output_ext: .mp4
    description: "hevc_videotoolbox H.265/HEVC — b:v 5000k, copy audio, .mp4 — macOS VideoToolbox"


  # ── RKMPP (Rockchip MPP — RK3588, NanoPi R6S, etc.) ──
  h264_rkmpp:
    codec: h264_rkmpp
    quality: qp 25
    audio_codec: copy
    output_ext: .mp4
    description: "h264_rkmpp H.264/AVC — qp 25, copy audio, .mp4 — Rockchip MPP hardware"

  hevc_rkmpp:
    codec: hevc_rkmpp
    quality: qp 28
    audio_codec: copy
    output_ext: .mkv
    description: "hevc_rkmpp HEVC/HEVC — qp 28, copy audio, .mkv — Rockchip MPP hardware"

  # ── Raspberry Pi (V4L2 mem2mem) ──
  h264_v4l2m2m:
    codec: h264_v4l2m2m
    quality: 3000k
    audio_codec: aac
    output_ext: .mp4
    description: "h264_v4l2m2m H.264/AVC — V4L2 mem2mem, Raspberry Pi hardware"

  hevc_v4l2m2m:
    codec: hevc_v4l2m2m
    quality: 3000k
    audio_codec: aac
    output_ext: .mkv
    description: "hevc_v4l2m2m HEVC/H.265 — V4L2 mem2mem, Raspberry Pi 4/5 hardware"

  # ── Legacy ──
  mpeg4:
    codec: mpeg4
    quality: qscale 4
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .avi
    description: "mpeg4 MPEG-4 Part 2 — qscale 4, aac 128k, .avi — legacy compatibility"

  mpeg2video:
    codec: mpeg2video
    quality: qscale 4
    audio_codec: mp2
    audio_bitrate: 192k
    output_ext: .mpg
    description: "mpeg2video MPEG-2 — qscale 4, mp2 192k, .mpg — DVD / broadcast"

  # ── Pass-through ──
  copy:
    codec: copy
    audio_codec: copy
    output_ext: .mp4
    description: "copy — stream copy all streams, no re-encoding, remux only, .mp4"

  copy_aac:
    codec: copy
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
    description: "copy_aac — copy video stream, re-encode audio to aac 128k, .mp4"
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
    transparent: true
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
    transparent: true
    description: "WebP 90% — modern web format"

  webp_80:
    output_ext: .webp
    quality: 80
    transparent: true
    description: "WebP 80% — balanced"

  webp_lossless:
    output_ext: .webp
    quality: 100
    transparent: true
    description: "WebP lossless"

  avif_90:
    output_ext: .avif
    quality: 90
    transparent: true
    description: "AVIF 90% — next-gen format, best compression"

  avif_80:
    output_ext: .avif
    quality: 80
    transparent: true
    description: "AVIF 80% — balanced"

  heif:
    output_ext: .heic
    quality: 90
    transparent: true
    description: "HEIF/HEIC — Apple format"

  tiff:
    output_ext: .tiff
    transparent: true
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

const DEFAULT_CUSTOM_PRESETS: &str = r#"# Custom command presets — reference by name in watcher rules.
# Define arbitrary CLI commands for file conversion.
# Placeholders: {input}, {output}, {basename}, {ext}, {output_folder}

presets:
  handbrake:
    command: "HandBrakeCLI -i {input} -o {output} --preset 'Fast 1080p30'"
    output_ext: .mp4
    description: "HandBrake CLI — Fast 1080p30 preset"

  # NOTE: The 'imagemagick' preset was removed due to RCE vulnerability.
  # ImageMagick's 'convert' and 'magick' binaries allow arbitrary file read
  # via the caption:@ DSL syntax (e.g., caption:@/etc/passwd).
  # Use alternative image processing tools or custom commands instead.

  ghostscript_compress:
    command: "gs -sDEVICE=pdfwrite -dCompatibilityLevel=1.4 -dPDFSETTINGS=/ebook -o {output} {input}"
    output_ext: .pdf
    description: "Ghostscript PDF compression (ebook quality)"
"#;
