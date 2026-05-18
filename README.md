# ConvWatcher

[![Rust](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Docker](https://img.shields.io/badge/docker-amd64%20%7C%20arm64-blue)](https://github.com/katzzero/ConvWatcher/pkgs/container/convwatcher)

A **daemon** that watches folders and automatically converts files based on YAML rules. Drop a file in, it comes out converted — no manual steps, no UI.

---

## What It Does

1. **Watches** folders for new/modified files (create/modify)
2. **Waits** for files to finish uploading (stable size + time)
3. **Matches** files to conversion rules by extension or subfolder name
4. **Converts** using the appropriate processor
5. **Places** output in a configurable folder
6. **Tracks** history and exposes a **health dashboard** on port 8080

---

## Architecture

```
config/
├── global.yaml              # Daemon-wide settings + secret
├── watchers.yaml            # Manifesto: declares watchers + default rules
└── watchs/                  # Per-watcher overrides (promoted automatically)
    └── videos.yaml

./inputs/                    # Watch folders (default root)
├── videos/                  # Each watcher = one type only
├── fotos/
├── musicas/
├── pdfs/
├── documentos/
└── scripts/

./outputs/                   # Output folders (default root)
├── videos-output/
├── fotos-output/
└── ...
```

**Key rule:** Each watcher is **single-type** — video, image, audio, pdf, document, or custom.

---

## Quick Start

### Prerequisites

```bash
# macOS
brew install ffmpeg ghostscript qpdf poppler pandoc
pip3 install img2pdf

# Linux (auto-detect)
./scripts/install_linux.sh
```

### Run

```bash
git clone https://github.com/katzzero/ConvWatcher.git
cd ConvWatcher

# Build and run
cargo build --release
./target/release/convwatcher

# Daemon mode
./target/release/convwatcher --daemon

# Quick single-folder watch
./target/release/convwatcher --watch /path/to/folder
```

### Docker

```bash
docker-compose up -d
```

---

## Configuration

### Global Config (`config/global.yaml`)

```yaml
file_check_interval_ms: 2000      # Scan interval (2s)
stable_time_ms: 5000              # File stability wait (5s)
max_concurrent_conversions: 4     # Worker pool size
config_refresh_interval_s: 300    # Hot-reload interval

inputs_dir: "./inputs"            # Root for watch folders
outputs_dir: "./outputs"          # Root for output folders

scan_embedded_configs: true
embedded_secret: "changeme"       # Secret for override validation
watchs_dir: "./config/watchs"     # Override files directory
embedded_scan_interval_s: 30      # Override scan interval

log:
  errors_file: "./logs/errors.log"

healthcheck:
  http_port: 8080

disk_space:
  check_interval_s: 60
  threshold:
    Gb: 5.0
```

### Watcher Config (`config/watchers.yaml`)

Declares watchers with their type and default rules. Each entry is **single-type**:

```yaml
watchers:
  - name: videos                  # watch_folder = ./inputs/videos/
    video:                        # TYPE: video only
      - input_extensions: [.mp4, .avi, .mkv]
        output_ext: .mp4
        codec: libx264
        quality: "crf 23"

  - name: fotos                   # watch_folder = ./inputs/fotos/
    output_folder: ./galeria      # Custom output folder
    image:                        # TYPE: image only
      - input_extensions: [.jpg, .png, .webp]
        output_ext: .png
        quality: 90

  - name: musicas
    audio:                        # TYPE: audio only
      - input_extensions: [.wav, .flac]
        output_ext: .mp3
        audio_codec: libmp3lame
        audio_bitrate: "320k"

  - name: pdfs
    pdf:                          # TYPE: pdf only
      - input_extensions: [.pdf]
        output_ext: .pdf
        mode: compress
        quality: ebook

  - name: documentos
    document:                     # TYPE: document only
      - input_extensions: [.md, .docx]
        output_ext: .pdf
        toc: true

  - name: scripts
    custom:                       # TYPE: custom command only
      - input_extensions: [.txt, .log]
        output_ext: ".zip"
        command: "zip -j {output} {input}"
```

### Default paths

| Field | Default |
|-------|---------|
| `watch_folder` | `./inputs/<name>/` |
| `output_folder` | `./outputs/<name>-output/` |

Both can be overridden per-watcher in the manifesto or via a `config/watchs/<name>.yaml` override.

---

## Per-Watcher Overrides (`config/watchs/`)

To override rules for a specific watcher without editing the main config:

1. Create a `.yaml` file named after the watcher: `config/watchs/videos.yaml`
2. Include a `secret` field matching `global.embedded_secret`
3. The type MUST match the manifesto type
4. Rules in the override REPLACE the manifesto rules for that watcher

**Manual (admin):** Place the file directly in `config/watchs/videos.yaml`.

**Self-service (user):** Drop `videos.yaml` in the watch folder root (`./inputs/videos/videos.yaml`). The system auto-validates and promotes it:

```
User drops ./inputs/videos/videos.yaml
        │
        ▼
  System validates secret + type
        │
   ┌────┴────┐
   ▼         ▼
 VALID     INVALID
   │         │
   ▼         ▼
 Copied    Creates
 to        videos.invalid
 config/   (empty file —
 watchs/   user deletes
           to retry)
   │
   ▼
 Original renamed to videos.yaml.old (backup)
   │
   ▼
 Embedded Scanner detects new override → applies rules
```

### Override format

```yaml
# config/watchs/videos.yaml
secret: "changeme"
output_folder: ./saida_especial
video:                                  # Must match manifesto type
  - input_extensions: [.mp4]
    output_ext: .mp4
    codec: libx265
    quality: "crf 28"
```

---

## Subfolder Mode

Create `->format/` subfolders inside watch folders. Drop any file in and it matches that rule:

```
./inputs/videos/
├── ->libx264/       # Drop any video → H.264 MP4
├── ->hevc/          # Drop any video → HEVC MP4
└── ->prores/        # Drop any video → ProRes MOV
```

The `format` field in the rule enables subfolder matching.

---

## Supported Conversions

| Type | Input | Output | Engine |
|------|-------|--------|--------|
| **Video** | MP4, AVI, MKV, MOV, WebM, FLV, WMV, MPEG, TS, MTS | Any codec | FFmpeg |
| **Image** | JPEG, PNG, GIF, BMP, TIFF, WebP, ICO, QOI, TGA | JPEG/PNG/GIF/BMP/TIFF/WebP/QOI | Rust `image` crate |
| **Audio** | MP3, WAV, FLAC, AAC, OGG, Opus, WMA, M4A, AIFF, CAF | MP3/FLAC/WAV/AAC/OGG/Opus | FFmpeg |
| **PDF** | PDF | Compress, PDF/A, extract, merge, encrypt | Ghostscript + QPDF + Poppler |
| **Document** | DOCX, ODT, EPUB, MD, HTML, LaTeX | PDF, EPUB, DOCX, ODT, HTML, MD, LaTeX | Pandoc |
| **Custom** | Any | Any | Arbitrary CLI command |

### PDF Modes

| Mode | Tool | Description |
|------|------|-------------|
| `compress` | Ghostscript | Optimize size (screen/ebook/printer/prepress) |
| `pdf_a` | Ghostscript | Convert to PDF/A archival |
| `extract_text` | pdftotext | Extract text to `.txt` |
| `extract_images` | pdfimages | Extract embedded images as PNG |
| `image_to_pdf` | img2pdf | Create PDF from images |
| `linearize` | QPDF | Web-optimize |
| `encrypt` | QPDF | Password-protect |
| `decrypt` | QPDF | Remove password |
| `analyze` | pdfinfo | Extract metadata |

---

## Health Dashboard

```
http://localhost:8080/dashboard
```

- Real-time stats: processed, errors, queue, processing
- Watcher cards with type + rules summary
- Conversion history (last 500 records)
- Auto-refresh every 5 seconds
- Dark/light mode

### API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | JSON health status |
| GET | `/dashboard` | HTML dashboard |
| GET | `/api/watchers` | JSON watcher list |
| GET | `/api/queue` | JSON queue status |
| GET | `/api/history` | JSON history |
| GET | `/logs` | Latest 100 log lines |
| GET | `/logs/errors` | Latest error log lines |

---

## Key Features

- **Single-type watchers** — each watcher does one thing well
- **Per-watcher overrides** — `config/watchs/<name>.yaml` with secret validation
- **Auto-promotion** — drop a config file in the watch folder, it gets validated and promoted
- **Type safety** — overrides locked to manifesto type
- **Hot Config Reload** — rescans configs periodically, dynamically restarts monitors
- **File Stability Detection** — waits for files to finish uploading
- **Worker Pool** — semaphore-based concurrency limiting
- **Disk Space Monitor** — halts on low disk, auto-resumes
- **Hardware Acceleration** — detects VAAPI/NVENC/QSV
- **Graceful Shutdown** — clean broadcast-channel shutdown
- **Daemon Mode** — background execution with log-file output
- **Multi-arch Docker** — AMD64 + ARM64

---

## Tech Stack

| Category | Technology |
|----------|-----------|
| Language | Rust (edition 2021, MSRV 1.85+) |
| Async | Tokio (full features) |
| CLI | Clap 4.5 (derive) |
| Serialization | Serde + serde_yaml + serde_json |
| File Watching | Notify 6.1 |
| Logging | log + fern (colored) + chrono |
| Image Processing | image 0.25 |
| HTTP Server | tiny_http 0.12 |
| Error Handling | anyhow 1.0 |

---

## Building from Source

```bash
cargo build --release
# → target/release/convwatcher

# ARM64 cross-compile
cargo install cross
cross build --release --target aarch64-unknown-linux-musl
```

---

## License

MIT
