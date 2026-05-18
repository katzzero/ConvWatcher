use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::config::global::GlobalConfig;

pub struct ErrorLogger {
    file: Mutex<PathBuf>,
    max_size_mb: u64,
}

impl ErrorLogger {
    pub fn new(global_config: &GlobalConfig) -> Result<Self, std::io::Error> {
        let path = PathBuf::from(&global_config.log.errors_file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self {
            file: Mutex::new(path),
            max_size_mb: global_config.log.max_error_log_size_mb,
        })
    }

    pub fn log(&self, message: &str, file_name: &str, context: &str) {
        let path = self.file.lock().unwrap();
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let line = format!(
            "{} [{}] {} — {}\n",
            timestamp, context, file_name, message
        );

        if let Ok(metadata) = fs::metadata(&*path) {
            if metadata.len() > self.max_size_mb * 1024 * 1024 {
                let rotated = path.with_extension("log.old");
                let _ = fs::rename(&*path, &rotated);
            }
        }

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&*path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}
