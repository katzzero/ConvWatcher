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
├── config.yaml              # Single config file: global settings + watchers
├── video_codecs.yaml        # Video codec presets
├── audio_codecs.yaml        # Audio codec presets
├── image_codecs.yaml        # Image format presets
├── pdf_presets.yaml         # PDF processing presets
├── document_presets.yaml    # Document conversion presets
└── watchs/                  # Per-watcher overrides (promoted automatically)

./inputs/                    # Watch folders
├── videos/
│   ├── ->gpu/               # Subfolder mode: drop files here for GPU encoding
│   └── ->archive/           # Subfolder mode: drop files here for archival
├── images/
├── audio/
└── ...

./outputs/                   # Output folders
├── videos-output/
├── images-output/
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

The multi-arch image (`linux/amd64` + `linux/arm64`) is published to both **GitHub Container Registry** and **Docker Hub**:

```
ghcr.io/katzzero/convwatcher:latest
katzzero/convwatcher:latest
```

- **amd64**: Ubuntu-based with **VAAPI** (Intel/AMD GPUs) **and NVENC** (NVIDIA GPUs) support
- **arm64**: Ubuntu-based with **Rockchip MPP** (RK3588 — NanoPi R6S, Orange Pi 5, etc.) **and Raspberry Pi V4L2 mem2mem** (h264/hevc) support

### Hardware acceleration matrix

| Backend | Hardware | Arch | Presets | Device / runtime requirement |
|---------|----------|------|---------|------------------------------|
| VAAPI | Intel / AMD GPU | amd64 | `h264_vaapi`, `hevc_vaapi`, `vp9_vaapi`, `av1_vaapi` | `/dev/dri` mounted |
| NVENC | NVIDIA GPU | amd64 | `h264_nvenc`, `h264_nvenc_high`, `hevc_nvenc`, `hevc_nvenc_high`, `av1_nvenc` | run with `docker run --gpus all` + host `nvidia-container-toolkit` |
| RKMPP | Rockchip (RK3588, etc.) | arm64 | `h264_rkmpp`, `hevc_rkmpp` | `/dev/dri` mounted |
| V4L2 mem2mem | Raspberry Pi (3/4/5) | arm64 | `h264_v4l2m2m`, `hevc_v4l2m2m` | `/dev/videoN` mounted (e.g. `/dev/video11`) |

ConvWatcher detects the available encoders at startup and logs them
(`VAAPI=… NVENC=… RKMPP=…`); route files to the matching preset via a
subfolder or root rule. The legacy `h264_omx` (Raspberry Pi OMX) preset is
deprecated — OMX was removed in FFmpeg 6.x+, use `h264_v4l2m2m` instead.

For Rockchip devices, ensure `/dev/dri` is mapped in docker-compose:

```yaml
services:
  convwatcher:
    image: ghcr.io/katzzero/convwatcher:latest
    devices:
      - /dev/dri:/dev/dri
```

See `examples/10_gpu_rkmpp.yaml` for a Rockchip MPP configuration.

---

## Configuration

### Single Config File (`config/config.yaml`)

All settings live in one file: global daemon settings + watcher definitions.

```yaml
global:
  file_check_interval: 2s       # Scan interval
  stable_time: 5s               # File stability wait
  ffmpeg_path: /usr/bin/ffmpeg
  max_concurrent: 4             # Worker pool size
  refresh_interval: 5m          # Hot-reload interval

  codec_presets:                # Preset files (relative to config/)
    video: video_codecs.yaml
    audio: audio_codecs.yaml
    image: image_codecs.yaml
    pdf: pdf_presets.yaml
    document: document_presets.yaml

  log:
    errors_file: /app/logs/errors.log

  healthcheck:
    http_port: 8080
    bind_address: 0.0.0.0

  disk_space:
    check_interval: 60s
    threshold: 500              # MB

  history:
    persistent: false
    file: /app/logs/history.json
    max_records: 500

watchers:
  - name: videos
    watch_folder: /app/inputs/videos/
    output_folder: /app/outputs/videos-output/
    type: video
    subfolders:
      - name: gpu
        description: "NVIDIA GPU encoding"
      - name: archive
        description: "HEVC archival"
    rules:
      - input_extensions: [.mp4, .avi, .mkv, .mov, .mxf]
        preset: libx264
        output_name: "{base}_{codec}_{num}.{ext}"
        check_duration: true
        min_duration_ratio: 0.9

      - subfolder: gpu
        input_extensions: [.mxf, .mts, .mov]
        preset: h264_nvenc

      - subfolder: archive
        input_extensions: [.mxf, .mts, .mkv]
        preset: libx265_high
```

### Key Changes from v0.8

| Old (v0.8) | New (v0.9) |
|------------|------------|
| `config/global.yaml` + `config/watchers.yaml` | Single `config/config.yaml` |
| Inline codec settings | `preset` references named presets |
| `format` field for subfolder matching | `subfolder` field + explicit `subfolders` list |
| Relative paths allowed | **Absolute paths required** |
| Codec settings in rules | Codec presets in separate YAML files |

### Config Reference

#### Global Config Reference

| Campo | Tipo | Obrigatório | Default | Descrição |
|-------|------|-------------|---------|-----------|
| `file_check_interval` | string (duração) | não | `2s` | Intervalo de varredura das pastas |
| `stable_time` | string (duração) | não | `5s` | Tempo de estabilidade após upload |
| `ffmpeg_path` | string (path) | não | `/usr/bin/ffmpeg` | Caminho do FFmpeg |
| `ffprobe_path` | string (path) | não | — | Caminho do FFprobe (auto-detecta se omitido) |
| `max_concurrent_conversions` | integer | não | `4` | Máx. conversões simultâneas |
| `config_refresh_interval_s` | string (duração) | não | `5m` | Intervalo de hot-reload |
| `embedded_secret` | string | não | `""` | Secret para validação de overrides |
| `embedded_scan_interval_s` | integer | não | `0` | Intervalo de scan de overrides |
| `codec_presets.*` | string (path) | não | `video_codecs.yaml` etc | Arquivos de presets |
| `log.errors_file` | string (path) | não | `<CWD>/logs/errors.log` | Path do log de erros |
| `log.max_log_files` | integer | não | `30` | Máx. arquivos de log rotacionados |
| `log.max_log_size_mb` | integer | não | `100` | Tamanho máx. por arquivo de log (MB) |
| `healthcheck.http_port` | integer | não | `8080` | Porta do dashboard |
| `healthcheck.bind_address` | string | não | `0.0.0.0` | Endereço de bind |
| `disk_space.check_interval` | string (duração) | não | `60s` | Intervalo de verificação de disco |
| `disk_space.threshold` | string/int | não | `500` | Limiar de espaço livre (MB, Gb, %) |
| `disk_space.check_output` | boolean | não | `false` | Verificar disco da pasta de saída |
| `disk_space.check_watch` | boolean | não | `false` | Verificar disco da pasta de entrada |
| `history.persistent` | boolean | não | `false` | Persistir histórico em disco |
| `history.file` | string (path) | não | `<CWD>/logs/history.json` | Path do histórico |
| `history.max_records` | integer | não | `500` | Máx. registros no histórico |

#### Watcher Config Reference

| Campo | Tipo | Obrigatório | Descrição |
|-------|------|-------------|-----------|
| `name` | string | sim | Rótulo do watcher (exibido no dashboard) |
| `watch_folder` | string (path) | sim | Pasta a monitorar (deve ser absoluto) |
| `output_folder` | string (path) | sim | Pasta de saída (deve ser absoluto) |
| `type` | enum | sim | `video`, `image`, `audio`, `pdf`, `document`, `custom` |
| `subfolders[].name` | string | não | Nome da subpasta (cria `->{name}/`) |
| `subfolders[].description` | string | não | Descrição da subpasta |

#### Rule Config Reference

| Campo | Tipo | Obrigatório | Default | Descrição |
|-------|------|-------------|---------|-----------|
| `preset` | string | **sim** | — | Nome do preset no arquivo de codecs |
| `input_extensions` | string[] | sim | — | Extensões de arquivo a processar |
| `subfolder` | string | não | — | Subpasta alvo (omitir = pasta raiz) |
| `output_ext` | string | não | (do preset) | Sobrescrever extensão de saída |
| `output_name` | string | não | `{base}_{codec}_{num}.{ext}` | Template do nome de saída |
| `check_duration` | bool | não | `true` | Verificar duração output vs input (vídeo) |
| `min_duration_ratio` | float | não | `0.9` | Razão mínima de duração aceitável (vídeo) |

**Override fields (opcionais, sobrescrevem o preset):**

**Video:** `codec`, `quality`, `audio_codec`, `audio_bitrate`  
**Image:** `quality`, `transparent`  
**Audio:** `audio_codec`, `audio_bitrate`, `sample_rate`, `channels`  
**PDF:** `mode`, `quality`, `pdfa_version`, `resolution`, `password`  
**Document:** `toc`, `toc_depth`, `css`, `template`, `standalone`, `pdf_engine`, `metadata`  
**Custom:** `command`, `description`

### Codec Presets

Presets are defined in separate YAML files under `config/`. Rules reference them by name:

```yaml
# config/video_codecs.yaml
presets:
  libx264:
    codec: libx264
    quality: crf 23
    audio_codec: aac
    audio_bitrate: 128k
    output_ext: .mp4
    description: "H.264 CPU — general purpose"

  libx264_high:
    codec: libx264
    quality: crf 18
    audio_codec: aac
    audio_bitrate: 192k
    output_ext: .mp4
    description: "H.264 CPU — high quality"

  h264_nvenc:
    codec: h264_nvenc
    quality: cq 23
    audio_codec: copy
    output_ext: .mp4
    description: "H.264 NVENC — NVIDIA GPU"
```

**Built-in presets** cover: CPU (libx264, libx265, VP9, AV1), GPU (NVENC, VAAPI, AMF, QSV, VideoToolbox, RKMPP), audio (MP3, AAC, Opus, FLAC), images (JPEG, WebP, AVIF, PNG), PDF modes, and document conversions.

**`audio_codec: copy`** passes through audio without re-encoding (skips bitrate flag).

### Watcher Rules

Every rule **must** specify a `preset`. Optional fields override the preset:

```yaml
rules:
  # Minimal — just reference a preset
  - input_extensions: [.mp4, .avi]
    preset: libx264

  # Override specific settings
  - input_extensions: [.mxf]
    preset: libx264
    quality: "crf 18"           # Override preset quality
    audio_codec: copy           # Pass through audio
```

### Subfolder Mode

Declare subfolders in the watcher config. Files dropped in `->{name}/` match rules with `subfolder: <name>`:

```yaml
watchers:
  - name: videos
    watch_folder: /app/inputs/videos/
    output_folder: /app/outputs/videos-output/
    type: video
    subfolders:
      - name: gpu
        description: "GPU encoding"
      - name: archive
        description: "High-quality archival"
    rules:
      - input_extensions: [.mp4, .avi, .mkv]
        preset: libx264                     # Root folder files

      - subfolder: gpu
        input_extensions: [.mxf, .mov]
        preset: h264_nvenc                  # ->gpu/ files

      - subfolder: archive
        input_extensions: [.mxf, .mkv]
        preset: libx265_high                # ->archive/ files
```

Directory structure:
```
./inputs/videos/
├── video.mp4          → matches root rule → libx264
├── ->gpu/
│   └── broadcast.mxf  → matches gpu rule → h264_nvenc
└── ->archive/
    └── master.mxf     → matches archive rule → libx265_high
```

### Absolute Paths

All paths **must** be absolute. Relative paths are rejected at startup:

```yaml
# Valid
watch_folder: /app/inputs/videos/
output_folder: /app/outputs/videos-output/

# Invalid — will fail validation
watch_folder: ./inputs/videos/
output_folder: ../outputs/videos-output/
```

---

## Per-Watcher Overrides (`config/watchs/`)

To override rules for a specific watcher without editing the main config:

1. Create a `.yaml` file named after the watcher: `config/watchs/videos.yaml`
2. Include a `secret` field matching `global.embedded_secret`
3. The type MUST match the main config type
4. Rules in the override REPLACE the main config rules for that watcher

**Manual (admin):** Place the file directly in `config/watchs/videos.yaml`.

**Self-service (user):** Drop `videos.yaml` in the watch folder root (`./inputs/videos/videos.yaml`). The system auto-validates and promotes it.

### Override format

```yaml
# config/watchs/videos.yaml
secret: "changeme"
output_folder: /app/outputs/special/
type: video
rules:
  - input_extensions: [.mp4]
    preset: libx265_high
    quality: "crf 18"
```

---

## Supported Conversions

| Type | Input | Output | Engine |
|------|-------|--------|--------|
| **Video** | MP4, AVI, MKV, MOV, WebM, FLV, WMV, MPEG, TS, MTS, MXF | Any codec | FFmpeg |
| **Image** | JPEG, PNG, GIF, BMP, TIFF, WebP, ICO, QOI, TGA, HEIC | JPEG/PNG/GIF/BMP/TIFF/WebP/QOI/AVIF/HEIF | Rust `image` crate |
| **Audio** | MP3, WAV, FLAC, AAC, OGG, Opus, WMA, M4A, AIFF, CAF | MP3/FLAC/WAV/AAC/OGG/Opus/AC3 | FFmpeg |
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

- **Single config file** — `config/config.yaml` replaces split files
- **Codec presets** — named presets in separate YAML files, referenced by rules
- **Explicit subfolders** — declare `subfolders` in config, auto-creates `->name/` directories
- **Absolute paths** — validates all paths at startup, rejects relative paths
- **Per-watcher overrides** — `config/watchs/<name>.yaml` with secret validation
- **Auto-promotion** — drop a config file in the watch folder, it gets validated and promoted
- **Type safety** — overrides locked to watcher type
- **Hot Config Reload** — rescans configs periodically, dynamically restarts monitors
- **File Stability Detection** — waits for files to finish uploading
- **Worker Pool** — semaphore-based concurrency limiting
- **Disk Space Monitor** — halts on low disk, auto-resumes
- **Hardware Acceleration** — detects VAAPI/NVENC/QSV/AMF/VideoToolbox/RKMPP and Raspberry Pi V4L2 mem2mem
- **Graceful Shutdown** — clean broadcast-channel shutdown
- **Daemon Mode** — background execution with log-file output
- **Multi-arch Docker** — AMD64 (VAAPI + NVENC) + ARM64 (Rockchip MPP + Raspberry Pi V4L2)

---

## Examples

See `examples/` for ready-to-use configurations:

| File | Description |
|------|-------------|
| `01_minimal_video.yaml` | Single video watcher, CPU encoding |
| `02_full_pipeline.yaml` | All 6 watcher types with subfolders |
| `03_gpu_nvenc.yaml` | NVIDIA GPU encoding presets |
| `04_gpu_vaapi.yaml` | Intel/AMD GPU via VAAPI |
| `05_broadcast_mxf.yaml` | Broadcast MXF workflow |
| `06_audio_podcast.yaml` | Audio conversion pipeline |
| `07_image_processing.yaml` | Image format conversions |
| `08_pdf_processing.yaml` | PDF compression and extraction |
| `09_document_conversion.yaml` | Document to PDF pipeline |
| `10_gpu_rkmpp.yaml` | Rockchip MPP hardware encoding (RK3588/NanoPi R6S) |
| `custom_presets.yaml` | Custom preset definitions |

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
