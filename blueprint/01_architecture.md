# ConvWatcher — Architecture

## High-Level Architecture

```
                    ┌──────────────────────────────────────────────────────┐
                    │                      main.rs                        │
                    │  CLI → config → hardware check → spawn subsystems   │
                    └────┬────┬────┬────┬────┬────┬────┬────┬────┬───────┘
                         │    │    │    │    │    │    │    │    │
              ┌──────────┘    │    │    │    │    │    │    │    └──────────┐
              ▼               ▼    │    │    │    │    │    ▼               ▼
     ┌────────────────┐ ┌────────┐ │    │    │    │    │ ┌────────────────┐
     │  Watchers (N)  │ │ Health │ │    │    │    │    │ │ Worker Pool    │
     │  monitor.rs    │ │ Server │ │    │    │    │    │ │ process_jobs() │
     │  ┌──────────┐  │ │ port   │ │    │    │    │    │ │ Semaphore(N)   │
     │  │ notify   │  │ │ 8080   │ │    │    │    │    │ │                │
     │  │ scan_dir │  │ └────────┘ │    │    │    │    │ │ ┌──────────┐   │
     │  │ stability│  │            │    │    │    │    │ │ │ Video    │   │
     │  │ create_j │  │            │    │    │    │    │ │ │ Image    │   │
     │  └──────────┘  │            │    │    │    │    │ │ │ Audio    │   │
     └───────┬────────┘            │    │    │    │    │ │ │ PDF      │   │
             │                     │    │    │    │    │ │ │ Document │   │
             │ mpsc::channel       │    │    │    │    │ │ │ External │   │
             │ ConversionJob       │    │    │    │    │ │ └──────────┘   │
             └─────────────────────┘    │    │    │    │ └────────────────┘
                                        │    │    │    │
              ┌─────────────────────────┘    │    │    └──────────────────┐
              ▼                              ▼    ▼                       ▼
     ┌────────────────┐           ┌──────────────────┐    ┌──────────────────┐
     │ Config Hot-    │           │ Disk Space       │    │ Embedded Config  │
     │ Reloader       │           │ Monitor          │    │ Scanner (NEW)    │
     │ (background    │           │ (background      │    │ (scans for       │
     │  tick every N  │           │  check, halt/    │    │  mainconfig.yaml)   │
     │  seconds)      │           │  resume)         │    │                  │
     └────────┬───────┘           └──────────────────┘    └────────┬─────────┘
              │                                                   │
              │ reload_tx (mpsc)                                  │ reload_tx
              ▼                                                   ▼
     ┌────────────────────────────────────────────────────────────────────────┐
     │                      monitor_manager task                             │
     │  Receives new WatchConfigs → shutdown old monitors → spawn new ones   │
     └────────────────────────────────────────────────────────────────────────┘
```

## Startup Sequence (main.rs)

```
1. Parse CLI args (clap)
   │
2. Setup logging (fern — colored console + file)
   │
3. Load global.yaml → GlobalConfig
   │
4. Load watchers.yaml (or watch*.yaml) → Vec<WatchConfig>
   │
5. Scan for embedded mainconfig.yaml files → merge into watch_configs
   │
6. Create all watch/output folders + subfolders (->format/)
   │
7. Initialize ErrorLogger
   │
8. Detect hardware acceleration (VAAPI/NVENC/QSV) via ffmpeg -encoders
   │
9. Validate configured video codecs against available FFmpeg encoders
   │
10. Start HealthServer (HTTP, port 8080)
    │
11. Register watcher info in health server
    │
12. Create mpsc::channel<ConversionJob> for job dispatch
    │
13. Spawn file monitors (one per watch_config → notify + scan_directory)
    │
14. Spawn worker pool (process_jobs — semaphore-limited)
    │
15. Spawn disk space monitor (background check)
    │
16. Spawn config hot-reloader (periodic config scan)
    │
17. Spawn embedded config scanner (periodic scan for mainconfig.yaml)
    │
18. Spawn monitor_manager (receives reload_tx → manage watchers)
    │
19. Wait for Ctrl+C → broadcast shutdown → cleanup
```

## Data Flow

### File Detection
```
notify crate event (Create/Modify)
    │
    ▼
Update file_states HashMap
  key: PathBuf
  value: (first_seen: Instant, last_size: u64, initial_size: u64)
    │
    ▼
Periodic scan_directory() also updates file_states
    │
    ▼
Every check_interval, evaluate ready files:
  - size unchanged since last check AND
  - age >= stable_time
    │
    ▼
Ready file → create_job() → ConversionJob
    │
    ▼
send via mpsc::channel to worker pool
```

### Job Dispatch (process_jobs in main.rs)
```
job_rx.recv() → ConversionJob
    │
    ▼
tokio::spawn with semaphore permit
    │
    ▼
match ConversionJob variant:
  │
  ├── Video   → processor::video::process_video()
  ├── Image   → processor::image::process_image()
  ├── Audio   → processor::audio::process_audio()
  ├── Pdf     → processor::pdf::process_pdf()
  ├── Document→ processor::document::process_document()
  └── External→ processor::external::process_external()
    │
    ▼
Update health server (processed/error/history)
```

### Config Reload
```
Background ticker every config_refresh_interval_s
    │
    ▼
Load global.yaml → update shared GlobalConfig
    │
    ▼
Load watchers.yaml → compare with current
    │
    ▼
If changed → send new Vec<WatchConfig> via reload_tx
    │
    ▼
monitor_manager task:
  → broadcast shutdown to all monitors
  → await old monitor handles
  → create folders for new configs
  → spawn new monitors
```

### Embedded Config Scan
```
Background ticker every embedded_scan_interval_s
    │
    ▼
Walk configured base paths (or /) for mainconfig.yaml files
    │
    ▼
Compare with known embedded_configs set
    │
    ▼
New/Changed: parse → build WatchConfig → send via reload_tx
Removed: remove from known set → notify monitor_manager
```

## Key Design Decisions

### Why mpsc channel for jobs?
- Decouples file detection from processing
- Allows backpressure (channel capacity 100)
- Worker pool can be sized independently

### Why semaphore for concurrency?
- Prevents resource exhaustion from too many simultaneous FFmpeg/Pandoc processes
- Configurable via max_concurrent_conversions in global.yaml
- Each worker acquires a permit before processing

### Why both notify + periodic scan?
- `notify` crate is fast but unreliable in Docker (volume mounts may not propagate events)
- Periodic `scan_directory()` catches missed events
- File stability tracking prevents processing partially-written files

### Why subfolder mode (->format/)?
- User organizes by intent: drop a file in the folder for the output format they want
- No need to match by extension — works with any input format
- Parallel to how image_watch/video_watch worked in v1, extended to all types

### Why embedded config?
- Zero-configuration per-folder: just drop a `mainconfig.yaml` and start using
- Enables ad-hoc watch folders without editing main config
- Auto-cleanup when the config file is removed
- Perfect for Docker volumes where the user wants to self-configure

## Threading Model

All async, single tokio runtime. No manual threads.

| Task | Role |
|------|------|
| `main()` | Orchestrator, shutdown handler |
| Health server | `tokio::spawn` — HTTP listener |
| N file monitors | `tokio::spawn` — one per watch folder |
| Worker pool | `tokio::spawn` — processes jobs |
| Disk monitor | `tokio::spawn` — periodic disk check |
| Config reloader | `tokio::spawn` — periodic config check |
| Embedded scanner | `tokio::spawn` — periodic embedded config scan |
| Monitor manager | `tokio::spawn` — manages watcher lifecycle |

Shared state is protected by:
- `Arc<Mutex<GlobalConfig>>` — hot-reloaded config
- `Arc<Mutex<Vec<WatchConfig>>>` — hot-reloaded watch configs
- `Arc<HealthServer>` — internally synchronized stats
- `Arc<Semaphore>` — worker pool permits
