# ConvWatcher — Health Server & Dashboard

## Module: `src/health/server.rs`

Embedded HTTP server using `tiny_http`. Serves health checks, API endpoints, log access, and a web dashboard.

### Startup

```rust
let health_server = Arc::new(
    HealthServer::new(health_port)
        .with_error_logger(error_log_path)
        .with_hardware_info(hw_info)
        .with_history_persistence(&global_config.history.file, global_config.history.persistent).await
);

// Register watchers
for watch_config in &watch_configs {
    health_server.add_watcher_with_config(watch_config).await;
}

// Spawn in background
let health_handle = tokio::spawn(async move {
    if let Err(e) = health_server.run().await {
        error!("Health server error: {}", e);
    }
});
```

### HealthServer Struct

```rust
pub struct HealthServer {
    port: u16,
    running: Arc<AtomicBool>,

    // Stats
    watchers: Arc<Mutex<Vec<WatcherInfo>>>,
    queue: Arc<Mutex<HashMap<String, Vec<String>>>>,     // watcher → queue
    processing: Arc<Mutex<HashMap<String, String>>>,     // watcher → current file
    history: Arc<Mutex<Vec<ConversionRecord>>>,

    // Error log tail
    error_log_path: Option<String>,
    error_log_cache: Arc<Mutex<String>>,

    // Hardware info
    hw_info: Arc<Mutex<Option<HardwareAccelInfo>>>,
}

pub struct WatcherInfo {
    pub watch_folder: String,
    pub output_folder: String,
    pub video_watch: bool,
    pub image_watch: bool,
    pub audio_watch: bool,         // NEW
    pub doc_watch: bool,           // NEW
    pub custom_watch: bool,        // NEW
    pub video_rules: Vec<String>,
    pub image_rules: Vec<String>,
    pub audio_rules: Vec<String>,  // NEW
    pub pdf_rules: Vec<String>,    // NEW
    pub document_rules: Vec<String>, // NEW
    pub custom_rules: Vec<String>,
}

pub struct ConversionRecord {
    pub time: String,
    pub watcher: String,
    pub file: String,
    pub status: String,  // "done" or "error"
    pub output: String,
}
```

### HTTP Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | JSON health status (`{"status": "ok", "uptime": ..., "watchers": ..., "queue": ..., "processed": ..., "errors": ...}`) |
| GET | `/dashboard` | HTML dashboard (self-contained, dark/light mode) |
| GET | `/api/watchers` | JSON list of all watchers with their rules |
| GET | `/api/queue` | JSON queue status per watcher |
| GET | `/api/history` | JSON conversion history (last N records) |
| GET | `/logs` | Latest app log lines |
| GET | `/logs/errors` | Latest error log lines |
| GET | `/logs/app` | Full app log file |

### API Response Examples

**GET /health**
```json
{
  "status": "ok",
  "uptime": "2h 34m 12s",
  "watchers": 2,
  "queue": 3,
  "processing": 1,
  "processed": 147,
  "errors": 2,
  "disk_space": {
    "output": {"available_gb": 234.5, "status": "ok"},
    "watch": {"available_gb": 500.2, "status": "ok"}
  }
}
```

**GET /api/watchers**
```json
[
  {
    "watch_folder": "/data/media",
    "output_folder": "/data/converted",
    "video_rules": ["h264 (libx264, .mp4)", "prores (prores_ks, .mov)"],
    "audio_rules": ["mp3 (libmp3lame, .mp3, 320k)"],
    "image_rules": ["jpg (.jpg, q90)", "png (.png, q100)"]
  }
]
```

### watcher_info_from_config()

```rust
pub fn watcher_info_from_config(config: &WatchConfig) -> WatcherInfo {
    WatcherInfo {
        watch_folder: config.watch_folder.clone(),
        output_folder: config.output_folder.clone(),
        video_watch: config.video_watch,
        image_watch: config.image_watch,
        audio_watch: config.audio_watch,
        doc_watch: config.doc_watch,
        custom_watch: config.custom_watch,
        video_rules: config.video_rules.iter().map(|r| {
            if let Some(ref fmt) = r.format {
                format!("{} ({}), .{})", fmt, r.codec, r.output_ext)
            } else {
                format!("{:?} → {} ({})", r.input_extensions, r.output_ext, r.codec)
            }
        }).collect(),
        audio_rules: config.audio_rules.iter().map(|r| {
            if let Some(ref fmt) = r.format {
                format!("{} ({}), .{})", fmt, r.audio_codec, r.output_ext)
            } else {
                format!("{:?} → {} ({})", r.input_extensions, r.output_ext, r.audio_codec)
            }
        }).collect(),
        pdf_rules: config.pdf_rules.iter().map(|r| {
            if let Some(ref fmt) = r.format {
                format!("{} ({:?})", fmt, r.mode)
            } else {
                format!("{:?} → {} ({:?})", r.input_extensions, r.output_ext, r.mode)
            }
        }).collect(),
        document_rules: config.document_rules.iter().map(|r| {
            if let Some(ref fmt) = r.format {
                format!("{} → .{}", fmt, r.output_ext)
            } else {
                format!("{:?} → .{}", r.input_extensions, r.output_ext)
            }
        }).collect(),
        custom_rules: config.custom_rules.iter().map(|r| {
            if let Some(ref fmt) = r.format {
                format!("{}: {}", fmt, r.description.as_deref().unwrap_or(&r.command))
            } else {
                format!("{:?}: {}", r.input_extensions, r.description.as_deref().unwrap_or(&r.command))
            }
        }).collect(),
    }
}
```

---

## Module: `src/health/mod.rs`

```rust
pub mod server;
```

---

## Module: `src/health/dashboard.html`

Self-contained HTML file (~435 lines) with:
- Dark/light mode toggle
- Watcher cards showing current status
- Real-time stats (processed, errors, queue, processing)
- Conversion history table
- Log viewer
- Auto-refresh via JavaScript (every 5 seconds)

The dashboard is embedded into the binary at compile time using `include_str!()`:

```rust
// In server.rs, serving /dashboard:
fn serve_dashboard(&self, response: tiny_http::ResponseBox) {
    let html = include_str!("dashboard.html");
    // Replace placeholders with live data via JS
    // Return as text/html
}
```
