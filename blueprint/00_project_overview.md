# ConvWatcher — Project Overview

## Identity

| Field | Value |
|-------|-------|
| **Name** | ConvWatcher |
| **Former Name** | DOKCONV-WATCHER |
| **Purpose** | CLI daemon that watches folders for new/modified files and automatically converts them based on YAML rules |
| **Language** | Rust (edition 2021, minimum 1.85+) |
| **License** | MIT |
| **Original Author** | Katzzero |
| **GitHub** | https://github.com/katzzero/DOKCONV-WATCHER |

## Vision

A single binary that monitors folders and converts files automatically. Drop a file into a watched folder and it comes out converted on the other side. No manual steps, no UI needed — runs as a background daemon.

## What It Does

1. Watches one or more folders for file changes (create/modify)
2. Waits for files to finish uploading (stable size + stable time)
3. Matches files to conversion rules by extension or subfolder name
4. Converts using the appropriate processor (FFmpeg, Rust image crate, Ghostscript, Pandoc, or custom commands)
5. Places the output in a configurable output folder
6. Tracks history, exposes a health dashboard on HTTP port 8080

## Tech Stack

| Category | Technology | Purpose |
|----------|-----------|---------|
| Language | Rust (edition 2021) | Core application |
| Async Runtime | Tokio (full features) | Concurrency, I/O, task spawning |
| CLI Parsing | Clap 4.5 (derive macros) | CLI argument parsing |
| Serialization | Serde + serde_yaml + serde_json | Config parsing, HTTP JSON responses |
| File Watching | Notify 6.1 | Filesystem event detection |
| Logging | log + fern (colored) + chrono | Structured logging |
| Image Processing | image 0.25 (Rust crate) | Native image format conversion |
| FFmpeg | External binary (tokio::process::Command) | Video/audio transcoding |
| Ghostscript | External binary | PDF compression, PDF/A, merge |
| QPDF | External binary | PDF linearization, encryption |
| Poppler-utils | External binary | PDF-to-text, PDF-to-images |
| img2pdf | External binary | Image-to-PDF conversion |
| Pandoc | External binary | Document format conversion |
| HTTP Server | tiny_http 0.12 | Embedded health/dashboard server |
| Error Handling | anyhow 1.0 | Flexible error propagation |
| Docker | Alpine 3.23 runtime | Containerized deployment |

## Supported Conversions

### Native Modules (Rust crate `image`)
- **Image**: JPEG, PNG, GIF, BMP, TIFF, WebP, ICO, QOI, TGA

### FFmpeg Modules
- **Video**: MP4, AVI, MKV, MOV, WebM, FLV, WMV, MPEG, TS, MTS → multiple codecs
- **Audio**: MP3, FLAC, WAV, AAC, OGG, Opus, WMA → multiple codecs/configs

### Ghostscript/QPDF/Poppler/PDF Module
- **PDF Compress**: Compress/optimize PDF (screen/ebook/printer/prepress)
- **PDF/A**: Convert to PDF/A archival format
- **PDF to Text**: Extract text via pdftotext
- **PDF to Images**: Extract pages as PNG/JPEG
- **Image to PDF**: Create PDF from images via img2pdf
- **PDF Merge**: Combine multiple PDFs
- **PDF Linearize**: Web-optimize via QPDF
- **PDF Encrypt/Decrypt**: Password protection via QPDF
- **PDF Analyze**: Extract metadata via pdfinfo

### Pandoc Document Module
- **Document Conversion**: DOCX, ODT, EPUB, Markdown, HTML, LaTeX, PDF, and many more
- Features: TOC, CSS styling, templates, metadata, PDF engine

### Custom External Commands
- **Arbitrary**: Any CLI command with template substitution
- Placeholders: `{input}`, `{output}`, `{basename}`, `{ext}`, `{output_folder}`

## Key Features

- **Hot Config Reload**: Scans config files every N seconds, dynamically restarts monitors
- **Embedded Config**: Drop a `mainconfig.yaml` in any folder to auto-register it as a watcher
- **Universal Matching**: All rule types (video/audio/image/pdf/document/custom) work in parallel
- **Subfolder Mode**: Create `->format/` subfolders for each rule variant
- **File Stability Detection**: Waits for files to finish uploading before processing
- **Worker Pool**: Configurable max concurrent conversions (semaphore-based)
- **Disk Space Monitor**: Halts on low disk, auto-resumes on recovery
- **Hardware Acceleration Detection**: Auto-detects VAAPI/NVENC/QSV
- **Health Dashboard**: Embedded HTTP server with /health, /dashboard, /api/*
- **Conversion History**: Persistent JSON history with last 500 records
- **Graceful Shutdown**: Broadcast channel signals monitors for clean shutdown
- **Daemon Mode**: Background execution with log-file-only output
- **Docker**: Multi-arch containers (AMD64 + ARM64)

## Configuration Files

| File | Purpose |
|------|---------|
| `config/global.yaml` | Global daemon settings |
| `config/watchers.yaml` | All watchers with their rules |
| `config/watchN.yaml` | Individual watcher files (alternative) |
| `mainconfig.yaml` | Embedded config inside any folder (auto-detected) |

## Directory Structure

```
ConvWatcher/
├── Cargo.toml
├── Dockerfile
├── docker-compose.yml
├── docker-bake.json
├── .dockerignore
├── .github/workflows/docker.yml
├── config/
│   ├── global.yaml
│   └── watchers.yaml
├── examples/
│   └── watcher_sample.yaml
├── scripts/
│   ├── install_linux.sh
│   ├── install_macos.sh
│   ├── install_windows.ps1
│   ├── build-arm64.sh
│   └── build-docker-arm64.sh
├── src/
│   ├── main.rs
│   ├── cli.rs
│   ├── config/
│   │   ├── mod.rs
│   │   ├── global.rs
│   │   ├── watch.rs
│   │   └── embedded.rs
│   ├── processor/
│   │   ├── mod.rs
│   │   ├── job.rs
│   │   ├── video.rs
│   │   ├── image.rs
│   │   ├── audio.rs
│   │   ├── pdf.rs
│   │   ├── document.rs
│   │   ├── external.rs
│   │   ├── disk.rs
│   │   └── namer.rs
│   ├── watcher/
│   │   ├── mod.rs
│   │   ├── monitor.rs
│   │   └── embedded.rs
│   ├── health/
│   │   ├── mod.rs
│   │   ├── server.rs
│   │   └── dashboard.html
│   ├── logs/
│   │   ├── mod.rs
│   │   └── error_logger.rs
│   └── utils/
│       ├── mod.rs
│       ├── hardware.rs
│       └── path.rs
└── AI/
    ├── project_blueprint.json
    ├── development_roadmap.json
    └── technical_debt_audit.json
```

## Dependencies (Cargo.toml)

```toml
[package]
name = "convwatcher"
version = "2.0.0"
edition = "2021"
description = "A daemon that watches folders and automatically converts files"
license = "MIT"

[dependencies]
tokio = { version = "1", features = ["full"] }
clap = { version = "4.5", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_yaml = "0.9"
serde_json = "1"
notify = "6.1"
log = "0.4"
fern = { version = "0.7", features = ["colored"] }
chrono = "0.4"
image = "0.25"
tiny_http = "0.12"
anyhow = "1.0"

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
```

## System Dependencies (runtime)

| Tool | Required For | Install |
|------|-------------|---------|
| `ffmpeg` + `ffprobe` | Video + Audio conversion | `apt install ffmpeg` / `brew install ffmpeg` |
| `gs` (Ghostscript) | PDF compress, PDF/A, merge | `apt install ghostscript` / `brew install ghostscript` |
| `qpdf` | PDF linearize, encrypt, decrypt | `apt install qpdf` / `brew install qpdf` |
| `pdftotext`, `pdftoppm`, `pdfimages`, `pdfinfo` (poppler-utils) | PDF extract text/images, analyze | `apt install poppler-utils` / `brew install poppler` |
| `img2pdf` | Image to PDF | `pip install img2pdf` or `apt install img2pdf` |
| `pandoc` | Document conversion | `apt install pandoc` / `brew install pandoc` |
