# ConvWatcher — Complete File Manifest

Every file in the project, its purpose, and its key contents.

---

## Root Files

### `Cargo.toml`
- Package metadata (name: convwatcher, version: 2.0.0)
- Dependencies: tokio, clap, serde, serde_yaml, serde_json, notify, log, fern, chrono, image, tiny_http, anyhow
- Release profile: LTO, opt-level 3, strip symbols

### `Dockerfile`
- Multi-stage: Rust builder (rust:1.90-alpine3.22) → Alpine 3.23 runtime
- Installs runtime deps: ffmpeg, ghostscript, qpdf, poppler-utils, img2pdf, pandoc
- Non-root user (convwatcher:1000)
- Healthcheck: `curl -f http://localhost:8080/health`
- Cross-compiles for aarch64-unknown-linux-musl

### `docker-compose.yml`
- Volume mounts for config, watch folders, output folders, logs
- Device passthrough for /dev/dri (VAAPI)
- Resource limits
- Network: host mode recommended for simple setups

### `docker-bake.json`
- Multi-arch build configuration: linux/amd64 + linux/arm64
- GHCR + Docker Hub registries

### `.dockerignore`
- Ignores: target/, .git, .github, scripts, blueprint/, AI/

### `.github/workflows/docker.yml`
- Trigger: version tag (v*.*.*)
- Build + push multi-arch images to GHCR (ghcr.io/katzzero/convwatcher) and Docker Hub
- QEMU setup for ARM64 cross-compilation

---

## Source Files (`src/`)

### `src/main.rs` (~490 lines)

**Entry point. Orchestrates all subsystems.**

Key functions:
- `main()` → calls `run()`, exits on fatal error
- `run()` → full startup sequence:
  1. Parse CLI args
  2. Setup logging (fern with colored console + file rotation)
  3. Load global config
  4. Load watch configs (watchers.yaml or watch*.yaml)
  5. Scan for embedded configs (mainconfig.yaml)
  6. Create all watch/output folders + subfolders
  7. Init error logger
  8. Detect hardware acceleration
  9. Validate configured codecs
  10. Start health server
  11. Register watchers in health server
  12. Create mpsc channel for jobs
  13. Spawn file monitors (one per watch config)
  14. Spawn worker pool (process_jobs)
  15. Spawn disk space monitor
  16. Spawn config hot-reloader
  17. Spawn embedded config scanner
  18. Spawn monitor_manager
  19. Wait for Ctrl+C → graceful shutdown
- `process_jobs()` → receives ConversionJob from mpsc, dispatches to correct processor via semaphore-guarded tokio::spawn
- `setup_logging()` → configures fern with colored console output + file logging
- `collect_missing_codecs()` → validates configured video codecs against FFmpeg
- `spawn_monitors()` → closure that creates per-watcher monitor tasks

**Match arms (process_jobs):**
```rust
match job {
    ConversionJob::Video { ... }     => video::process_video(...)
    ConversionJob::Image { ... }     => image::process_image(...)
    ConversionJob::Audio { ... }     => audio::process_audio(...)
    ConversionJob::Pdf { ... }       => pdf::process_pdf(...)
    ConversionJob::Document { ... }  => document::process_document(...)
    ConversionJob::External { ... }  => external::process_external(...)
}
```

### `src/cli.rs` (~40 lines)

CLAP derive struct for CLI arguments: `--daemon`, `--no-daemon`, `--level`, `--config`, `--watch`.

### `src/config/mod.rs` (~80 lines)

- `pub mod global; pub mod watch; pub mod embedded;`
- `load_global_config()` → reads `config/global.yaml`
- `load_watch_configs()` → reads `config/watchers.yaml` or `config/watch*.yaml`

### `src/config/global.rs` (~120 lines)

- `GlobalConfig` struct with all daemon-wide settings
- `LogConfig`, `HealthcheckConfig`, `DiskSpaceConfig`, `HistoryConfig`
- `DiskSpaceThreshold` enum: `Mb(u64)`, `Gb(f64)`, `Percent(f64)`
- Default values for all fields

### `src/config/watch.rs` (~350 lines)

- `WatchConfigCollection` with `watchers: Vec<WatchConfig>`
- `WatchConfig` with all rule lists + subfolder mode booleans
- `VideoRule` with optional `format`, `VideoWatchRule` removed (unified)
- `ImageRule` with optional `format`
- `AudioRule` (NEW) with codec, bitrate, sample rate, channels, quality
- `PdfRule` (NEW) with mode, quality, pdfa_version, page_range, resolution, password, options
- `PdfMode` enum (NEW): Compress, PdfA, ExtractText, ExtractImages, ImageToPdf, Merge, Linearize, Encrypt, Decrypt, Analyze
- `PdfQuality` enum (NEW): Screen, Ebook, Printer, Prepress, Default
- `DocumentRule` (NEW) with toc, css, template, metadata, pdf_engine, options
- `CustomRule` with optional `format` (NEW)
- Default value functions for all fields

### `src/config/embedded.rs` (NEW, ~60 lines)

- `EmbeddedConfig` — subset of WatchConfig for `mainconfig.yaml` files
- `to_watch_config()` — converts to full WatchConfig using containing folder as watch_folder

### `src/processor/mod.rs` (~10 lines)

- `pub mod job; pub mod video; pub mod image; pub mod audio; pub mod pdf; pub mod document; pub mod external; pub mod disk; pub mod namer;`

### `src/processor/job.rs` (~50 lines)

- `ConversionJob` enum with 6 variants: Video, Image, **Audio** (new), **Pdf** (new), **Document** (new), External
- Each variant has: watcher_name, file_name, file_path, rule, output_folder, watch_folder

### `src/processor/video.rs` (~300 lines)

- `process_video()` main entry point
- `parse_quality_value()` — CRF / bitrate / VBR parsing
- `get_video_duration()` — ffprobe-based duration check
- FFmpeg command construction with all video/audio codec parameters
- Post-conversion duration validation

### `src/processor/image.rs` (~200 lines)

- `process_image()` main entry point
- `save_image()` — format-specific encoding (JPEG quality, PNG compression, WebP quality)
- Alpha channel handling based on `rule.transparent`

### `src/processor/audio.rs` (NEW, ~250 lines)

- `process_audio()` main entry point
- `build_audio_quality_args()` — constructs FFmpeg audio args
- FFmpeg: `-vn -c:a {codec} -b:a {bitrate} [-ar {sample_rate}] [-ac {channels}] [-q:a {quality}]`
- Supports: libmp3lame, aac, flac, libvorbis, libopus, pcm_s16le

### `src/processor/pdf.rs` (NEW, ~350 lines)

- `process_pdf()` dispatches to mode-specific functions
- `compress_pdf()` — Ghostscript: `-sDEVICE=pdfwrite -dPDFSETTINGS=/ebook`
- `convert_to_pdfa()` — Ghostscript: `-dPDFA=2 -dPDFACompatibilityPolicy=1`
- `extract_text()` — pdftotext
- `extract_images()` — pdfimages (outputs multiple files)
- `images_to_pdf()` — img2pdf
- `linearize_pdf()` — qpdf --linearize
- `encrypt_pdf()` — qpdf --encrypt
- `decrypt_pdf()` — qpdf --decrypt
- `analyze_pdf()` — pdfinfo (outputs metadata as JSON/YAML)

### `src/processor/document.rs` (NEW, ~200 lines)

- `process_document()` main entry point
- `convert_document()` — Pandoc command builder with all options
- Supports: toc, css, template, standalone, metadata, pdf_engine, extra options

### `src/processor/external.rs` (~320 lines)

- `process_external()` main entry point
- `ExternalProcessor` struct with template substitution
- `validate_command_template()` — security validation for program paths
- `validate_placeholder_values()` — blocks shell injection (`;`, `&&`, `` ` ``, `$()`, etc.)

### `src/processor/disk.rs` (~100 lines)

- `check_disk_space()` — checks available space on output/watch mount points
- `disk_space_monitor()` — background task that halts/resumes based on space

### `src/processor/namer.rs` (~80 lines)

- `OutputNamer::generate_path()` — template-based output path generation
- `OutputNamer::generate_with_counter()` — collision resolution with incremental counter
- Template variables: `{base}`, `{codec}`, `{num}`, `{ext}`

### `src/watcher/mod.rs`
- `pub mod monitor; pub mod embedded;`

### `src/watcher/monitor.rs` (~450 lines)

- `run_file_monitor()` — async per-watcher task
- `scan_directory()` — periodic file system scan
- `create_job()` — universal matching logic (subfolder → extension)
- `create_folders()` — creates watch/output folders + subfolders
- `reload_watch_configs()` — config reload helper
- File stability tracking via `HashMap<PathBuf, (Instant, u64, u64)>`

### `src/watcher/embedded.rs` (NEW, ~200 lines)

- `EmbeddedScanner` struct — manages discovered embedded configs
- `scan()` — walks scan paths for `mainconfig.yaml`, compares with known set
- `find_config_files()` — recursive directory walk
- `parse_config()` — reads and converts to WatchConfig
- `run_embedded_scanner()` — background task entry point

### `src/health/server.rs` (~350 lines)

- `HealthServer` struct with all stats, history, and endpoint handlers
- `add_watcher_with_config()` — registers watcher info
- `increment_processed() / increment_error()` — stats tracking
- `set_processing() / clear_processing()` — current file tracking
- `add_history()` — conversion record with persistence
- `run()` — main HTTP loop
- `watcher_info_from_config()` — converts WatchConfig to WatcherInfo

### `src/health/dashboard.html` (~435 lines)

Self-contained HTML with embedded CSS/JS:
- Dark/light mode toggle (persisted in localStorage)
- Watcher cards with name, folder, rules, stats
- Real-time queue, processing, history
- Auto-refresh every 5s via fetch API
- Log viewer with auto-scroll

### `src/health/mod.rs`

- `pub mod server;`

### `src/logs/error_logger.rs` (~80 lines)

- `ErrorLogger` with timestamped, contextualized error log
- Auto-rotation on size limit
- Thread-safe via Mutex<File>

### `src/logs/mod.rs`

- `pub mod error_logger;`

### `src/utils/hardware.rs` (~100 lines)

- `check_hardware_accel()` — detects VAAPI/NVENC/QSV
- `is_codec_available()` — FFmpeg codec validation

### `src/utils/path.rs` (~10 lines)

- `get_base_name()` — filename without extension

### `src/utils/mod.rs`

- `pub mod hardware; pub mod path;`

---

## Configuration Files

### `config/global.yaml`
```yaml
file_check_interval_ms: 2000
stable_time_ms: 5000
ffmpeg_path: ffmpeg
ffprobe_path: ffprobe
max_concurrent_conversions: 4
config_refresh_interval_s: 300
scan_embedded_configs: true
embedded_config_name: "mainconfig.yaml"
embedded_scan_interval_s: 30
embedded_scan_paths: ["/data", "."]
log:
  errors_file: "./logs/errors.log"
  max_log_files: 30
  max_log_size_mb: 100
  max_error_log_size_mb: 50
healthcheck:
  http_port: 8080
  bind_address: "0.0.0.0"
disk_space:
  check_interval_s: 60
  threshold:
    Gb: 5.0
  check_output: true
  check_watch: true
history:
  persistent: false
  file: "./logs/history.json"
  max_records: 500
```

### `config/watchers.yaml`
Collection of WatchConfig entries (see 02_config_structs.md for full format and examples/watcher_sample.yaml for annotated examples).

### `examples/watcher_sample.yaml`
Annotated example showing all options for all rule types.

---

## Scripts

### `scripts/install_linux.sh`
- Detects distro (apt/yum/dnf/pacman/zypper)
- Installs: ffmpeg, ghostscript, qpdf, poppler-utils, pandoc, python3-pip
- Installs img2pdf via pip

### `scripts/install_macos.sh`
- Uses Homebrew
- Installs: ffmpeg, ghostscript, qpdf, poppler, pandoc, img2pdf

### `scripts/install_windows.ps1`
- Uses Chocolatey
- Installs: ffmpeg, ghostscript, qpdf, poppler, pandoc
- Note: img2pdf requires Python + pip

### `scripts/build-arm64.sh`
- Cross-compiles for aarch64-unknown-linux-musl
- Requires: cross or zigbuild

### `scripts/build-docker-arm64.sh`
- Docker buildx multi-arch builder setup

---

## Blueprint Files (this directory)

| File | Content |
|------|---------|
| `00_project_overview.md` | Vision, tech stack, supported conversions, features |
| `01_architecture.md` | High-level architecture, data flow, startup sequence |
| `02_config_structs.md` | All config structs with default values |
| `03_processor_modules.md` | All 6 processor modules with command examples |
| `04_watcher_system.md` | File monitoring, universal matching, embedded config |
| `05_health_server.md` | HTTP endpoints, dashboard, API responses |
| `06_utility_modules.md` | Hardware detection, CLI, path utils, error logging |
| `07_file_manifest.md` | Every file with purpose and contents (this file) |
| `08_build_deploy.md` | Docker, CI/CD, scripts, cross-compilation |
| `09_migration_guide.md` | Migration from DOKCONV-WATCHER v1 to ConvWatcher v2 |
