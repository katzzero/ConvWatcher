# ConvWatcher — Utility Modules

## Module: `src/utils/hardware.rs`

Detects available hardware acceleration and validates FFmpeg codecs.

```rust
#[derive(Debug, Clone)]
pub struct HardwareAccelInfo {
    pub ffmpeg_vaapi_support: bool,     // true: VAAPI encoders found in ffmpeg
    pub vaapi_available: bool,          // true: /dev/dri/renderD* exists
    pub vaapi_encoders: Vec<String>,    // list of VAAPI encoder names
    pub vaapi_devices: Vec<String>,
    pub nvenc_available: bool,
    pub nvenc_encoders: Vec<String>,
    pub nvenc_devices: Vec<String>,
    pub qsv_available: bool,
    pub qsv_encoders: Vec<String>,
    pub all_encoders: Vec<String>,      // raw output of ffmpeg -encoders
}

/// Detect hardware acceleration by running ffmpeg -encoders and checking /dev/dri
pub async fn check_hardware_accel() -> HardwareAccelInfo {
    // 1. Run "ffmpeg -hide_banner -encoders"
    // 2. Parse output for encoder names
    // 3. Look for VAAPI (h264_vaapi, hevc_vaapi), NVENC (*_nvenc), QSV (*_qsv)
    // 4. Check /dev/dri/* for VAAPI devices
    // 5. Check nvidia-smi or /proc/driver/nvidia for NVENC
    // 6. Return HardwareAccelInfo
}

/// Check if a specific codec is in the available encoders list
pub fn is_codec_available(codec: &str, all_encoders: &[String]) -> bool {
    all_encoders.iter().any(|line| {
        line.split_whitespace()
            .nth(1)
            .map(|e| e == codec)
            .unwrap_or(false)
    })
}
```

### Detection Output Example

```
VAAPI encoders: h264_vaapi, hevc_vaapi, mjpeg_vaapi, vp8_vaapi, vp9_vaapi
VAAPI devices: /dev/dri/renderD128
NVENC encoders: h264_nvenc, hevc_nvenc
NVENC devices: GPU-12345678
No QSV support detected
```

---

## Module: `src/utils/path.rs`

Simple utility for extracting base filename without extension.

```rust
use std::path::Path;

/// Returns the filename without extension.
/// "video.mp4" → "video"
/// "archive.tar.gz" → "archive.tar"
pub fn get_base_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string())
}
```

---

## Module: `src/logs/error_logger.rs`

Structured error logging with file output, timestamps, and context.

```rust
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct ErrorLogger {
    file: Mutex<PathBuf>,
    max_size_mb: u64,
}

impl ErrorLogger {
    pub fn new(global_config: &GlobalConfig) -> Result<Self> {
        let path = PathBuf::from(&global_config.log.errors_file);
        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self {
            file: Mutex::new(path),
            max_size_mb: global_config.log.max_error_log_size_mb,
        })
    }

    /// Log an error with context
    pub fn log(&self, message: &str, file_name: &str, context: &str) {
        let path = self.file.lock().unwrap();
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let line = format!("{} [{}] {} — {}\n", timestamp, context, file_name, message);

        // Rotate if too large
        if let Ok(metadata) = fs::metadata(&*path) {
            if metadata.len() > self.max_size_mb * 1024 * 1024 {
                let rotated = path.with_extension("log.old");
                let _ = fs::rename(&*path, &rotated);
            }
        }

        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&*path) {
            let _ = file.write_all(line.as_bytes());
        }
    }

    pub fn shutdown(&self) {
        // Flush any pending writes
    }
}
```

---

## Module: `src/cli.rs`

Command-line argument parsing using clap derive macros.

```rust
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "ConvWatcher")]
#[command(about = "File conversion watcher daemon", long_about = None)]
pub struct Cli {
    /// Run in daemon mode (background, log to file only)
    #[arg(long, default_value_t = false)]
    pub daemon: bool,

    /// Force non-daemon mode (foreground)
    #[arg(long, default_value_t = false)]
    pub no_daemon: bool,

    /// Log level (debug, info, warn, error)
    #[arg(long, default_value = "info")]
    pub level: LogLevel,

    /// Path to custom config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Path to custom watch folder (quick start, single watcher)
    #[arg(short, long)]
    pub watch: Option<String>,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}
```

### CLI Usage

```bash
# Run in foreground (default)
convwatcher

# Run in background
convwatcher --daemon

# With custom config directory
convwatcher --config ./myconfig

# Quick single-folder watch
convwatcher --watch ./watch --level debug

# Log level
convwatcher --level debug

# Force foreground (override daemon mode from config)
convwatcher --no-daemon
```

---

## Module: `src/utils/mod.rs`

```rust
pub mod hardware;
pub mod path;
```

---

## Module: `src/logs/mod.rs`

```rust
pub mod error_logger;
```
