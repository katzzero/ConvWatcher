use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use log::{error, info};

use crate::config::watch::{WatchConfig, WatchType};
use crate::utils::hardware::HardwareAccelInfo;

pub struct HealthServer {
    port: u16,
    bind_address: String,
    running: Arc<AtomicBool>,
    start_time: Instant,

    watchers: Arc<Mutex<Vec<WatcherInfo>>>,
    queue: Arc<Mutex<HashMap<String, Vec<String>>>>,
    processing: Arc<Mutex<HashMap<String, String>>>,
    history: Arc<Mutex<Vec<ConversionRecord>>>,
    processed: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,

    error_log_path: Option<String>,
    app_log_path: Option<String>,
    hw_info: Arc<Mutex<Option<HardwareAccelInfo>>>,

    history_file: Option<String>,
    history_persistent: bool,
    max_records: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WatcherInfo {
    pub name: String,
    pub watch_folder: String,
    pub output_folder: String,
    pub watch_type: String,
    pub video_rules: Vec<String>,
    pub audio_rules: Vec<String>,
    pub image_rules: Vec<String>,
    pub pdf_rules: Vec<String>,
    pub document_rules: Vec<String>,
    pub custom_rules: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConversionRecord {
    pub time: String,
    pub watcher: String,
    pub file: String,
    pub status: String,
    pub output: String,
}

impl HealthServer {
    pub fn new(port: u16, bind_address: String, max_records: usize) -> Self {
        Self {
            port,
            bind_address,
            running: Arc::new(AtomicBool::new(false)),
            start_time: Instant::now(),
            watchers: Arc::new(Mutex::new(Vec::new())),
            queue: Arc::new(Mutex::new(HashMap::new())),
            processing: Arc::new(Mutex::new(HashMap::new())),
            history: Arc::new(Mutex::new(Vec::new())),
            processed: Arc::new(AtomicU64::new(0)),
            errors: Arc::new(AtomicU64::new(0)),
            error_log_path: None,
            app_log_path: None,
            hw_info: Arc::new(Mutex::new(None)),
            history_file: None,
            history_persistent: false,
            max_records,
        }
    }

    pub fn with_error_logger(mut self, path: String) -> Self {
        self.error_log_path = Some(path);
        self
    }

    pub fn with_hardware_info(self, info: HardwareAccelInfo) -> Self {
        *self.hw_info.lock().unwrap_or_else(|p| p.into_inner()) = Some(info);
        self
    }

    pub fn with_app_log(mut self, path: String) -> Self {
        self.app_log_path = Some(path);
        self
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn with_history_persistence(mut self, file: &str, persistent: bool) -> Self {
        if persistent {
            self.history_file = Some(file.to_string());
            self.history_persistent = true;

            if let Ok(content) = std::fs::read_to_string(file) {
                if let Ok(records) = serde_json::from_str::<Vec<ConversionRecord>>(&content) {
                    *self.history.lock().unwrap_or_else(|p| p.into_inner()) = records;
                }
            }
        }
        self
    }

    pub async fn add_watcher_with_config(&self, config: &WatchConfig) {
        let info = watcher_info_from_config(config);
        let mut watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
        watchers.retain(|w| w.watch_folder != info.watch_folder);
        watchers.push(info);
    }

    pub fn increment_processed(&self, _watcher: &str) -> Result<()> {
        self.processed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn increment_error(&self, _watcher: &str) -> Result<()> {
        self.errors.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn set_processing(&self, watcher: String, file: String) -> Result<()> {
        let mut map = self.processing.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(watcher, file);
        Ok(())
    }

    pub fn clear_processing(&self, watcher: &str) -> Result<()> {
        let mut map = self.processing.lock().unwrap_or_else(|p| p.into_inner());
        map.remove(watcher);
        Ok(())
    }

    pub fn enqueue(&self, file: &str) -> Result<()> {
        let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        let entry = queue.entry("global".to_string()).or_default();
        if !entry.contains(&file.to_string()) {
            entry.push(file.to_string());
        }
        Ok(())
    }

    pub fn dequeue(&self, file: &str) -> Result<()> {
        let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = queue.get_mut("global") {
            entry.retain(|f| f != file);
        }
        Ok(())
    }

    pub async fn add_history(&self, record: ConversionRecord) -> Result<()> {
        let snapshot = {
            let mut history = self.history.lock().unwrap_or_else(|p| p.into_inner());
            history.push(record);

            let max = self.max_records;
            let excess = if history.len() > max {
                history.len() - max
            } else {
                0
            };
            if excess > 0 {
                history.drain(0..excess);
            }

            history.clone()
        };

        if self.history_persistent {
            if let Some(ref file) = self.history_file {
                if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
                    let _ = std::fs::write(file, json);
                }
            }
        }

        Ok(())
    }

    pub fn run(&self) -> Result<()> {
        let addr = format!("{}:{}", self.bind_address, self.port);
        let server = match tiny_http::Server::http(&addr) {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to start health server on {}: {}", addr, e);
                return Err(anyhow::anyhow!("Failed to bind server: {}", e));
            }
        };

        self.running.store(true, Ordering::SeqCst);
        info!("Health server listening on http://{}", addr);

        loop {
            if !self.running.load(Ordering::Relaxed) {
                info!("Health server stopping");
                break;
            }

            let request = match server.try_recv() {
                Ok(Some(r)) => r,
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(e) => {
                    error!("Health server error: {}", e);
                    break;
                }
            };

            let url = request.url().to_string();
            let method = request.method().clone();
            let path = url.as_str();

            let json_ct =
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .unwrap();
            let html_ct = tiny_http::Header::from_bytes(
                &b"Content-Type"[..],
                &b"text/html; charset=utf-8"[..],
            )
            .unwrap();
            let text_ct =
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/plain"[..]).unwrap();

            match (method, path) {
                (tiny_http::Method::Get, "/") | (tiny_http::Method::Get, "/dashboard") => {
                    let html = include_str!("dashboard.html");
                    let _ = request.respond(
                        tiny_http::Response::from_string(html).with_header(html_ct.clone()),
                    );
                }
                (tiny_http::Method::Get, "/health") => {
                    let uptime = self.start_time.elapsed();
                    let uptime_str = format!(
                        "{}h {}m {}s",
                        uptime.as_secs() / 3600,
                        (uptime.as_secs() % 3600) / 60,
                        uptime.as_secs() % 60
                    );
                    let watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
                    let queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
                    let processing = self.processing.lock().unwrap_or_else(|p| p.into_inner());
                    let queue_count: usize = queue.values().map(|v| v.len()).sum();
                    let response = serde_json::json!({
                        "status": "ok",
                        "uptime": uptime_str,
                        "watchers": watchers.len(),
                        "queue": queue_count,
                        "processing": processing.len(),
                        "processed": self.processed.load(Ordering::SeqCst),
                        "errors": self.errors.load(Ordering::SeqCst),
                        "disk_space": {}
                    });
                    let body = serde_json::to_string_pretty(&response).unwrap_or_default();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body).with_header(json_ct.clone()),
                    );
                }
                (tiny_http::Method::Get, "/api/watchers") => {
                    let watchers = self.watchers.lock().unwrap_or_else(|p| p.into_inner());
                    let body = serde_json::to_string_pretty(&*watchers).unwrap_or_default();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body).with_header(json_ct.clone()),
                    );
                }
                (tiny_http::Method::Get, "/api/queue") => {
                    let queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
                    let processing = self.processing.lock().unwrap_or_else(|p| p.into_inner());
                    let response = serde_json::json!({
                        "queue": &*queue,
                        "processing": &*processing,
                    });
                    let body = serde_json::to_string_pretty(&response).unwrap_or_default();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body).with_header(json_ct.clone()),
                    );
                }
                (tiny_http::Method::Get, "/api/history") => {
                    let history = self.history.lock().unwrap_or_else(|p| p.into_inner());
                    let body = serde_json::to_string_pretty(&*history).unwrap_or_default();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body).with_header(json_ct.clone()),
                    );
                }
                (tiny_http::Method::Get, "/logs") => match &self.app_log_path {
                    Some(path) => match read_tail(path, 100) {
                        Ok(content) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(content)
                                    .with_header(text_ct.clone()),
                            );
                        }
                        Err(e) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(format!("Error: {}", e))
                                    .with_status_code(500),
                            );
                        }
                    },
                    None => {
                        let _ = request.respond(
                            tiny_http::Response::from_string("No log file configured")
                                .with_status_code(404),
                        );
                    }
                },
                (tiny_http::Method::Get, "/logs/errors") => match &self.error_log_path {
                    Some(path) => match read_tail(path, 100) {
                        Ok(content) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(content)
                                    .with_header(text_ct.clone()),
                            );
                        }
                        Err(e) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(format!("Error: {}", e))
                                    .with_status_code(500),
                            );
                        }
                    },
                    None => {
                        let _ = request.respond(
                            tiny_http::Response::from_string("No log file configured")
                                .with_status_code(404),
                        );
                    }
                },
                (tiny_http::Method::Get, "/logs/app") => match &self.app_log_path {
                    Some(path) => match read_tail(path, 100) {
                        Ok(content) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(content)
                                    .with_header(text_ct.clone()),
                            );
                        }
                        Err(e) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(format!("Error: {}", e))
                                    .with_status_code(500),
                            );
                        }
                    },
                    None => {
                        let _ = request.respond(
                            tiny_http::Response::from_string("No log file configured")
                                .with_status_code(404),
                        );
                    }
                },
                _ => {
                    let _ = request.respond(
                        tiny_http::Response::from_string("Not Found").with_status_code(404),
                    );
                }
            }
        }

        Ok(())
    }
}

fn read_tail(path: &str, max_lines: usize) -> Result<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();

    let read_size = std::cmp::min(16_384, file_len) as usize;
    if read_size == 0 {
        return Ok(String::new());
    }
    file.seek(SeekFrom::End(-(read_size as i64)))?;

    let mut buf = vec![0u8; read_size];
    file.read_exact(&mut buf)?;

    let content = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = content.lines().collect();
    let start = if lines.len() > max_lines {
        lines.len() - max_lines
    } else {
        0
    };
    Ok(lines[start..].join("\n"))
}

pub fn watcher_info_from_config(config: &WatchConfig) -> WatcherInfo {
    let (type_name, mut video_rules, mut audio_rules, mut image_rules, mut pdf_rules, mut document_rules, mut custom_rules) = match &config.watch_type {
        WatchType::Video { rules } => ("video", format_rules_video(rules), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        WatchType::Image { rules } => ("image", Vec::new(), Vec::new(), format_rules_image(rules), Vec::new(), Vec::new(), Vec::new()),
        WatchType::Audio { rules } => ("audio", Vec::new(), format_rules_audio(rules), Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        WatchType::Pdf { rules } => ("pdf", Vec::new(), Vec::new(), Vec::new(), format_rules_pdf(rules), Vec::new(), Vec::new()),
        WatchType::Document { rules } => ("document", Vec::new(), Vec::new(), Vec::new(), Vec::new(), format_rules_document(rules), Vec::new()),
        WatchType::Custom { rules } => ("custom", Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), format_rules_custom(rules)),
    };

    for sf in &config.subfolders {
        let desc = sf.description.as_deref().unwrap_or("");
        let entry = format!("->{}: {}", sf.name, desc);
        match type_name {
            "video" => video_rules.push(entry),
            "audio" => audio_rules.push(entry),
            "image" => image_rules.push(entry),
            "pdf" => pdf_rules.push(entry),
            "document" => document_rules.push(entry),
            "custom" => custom_rules.push(entry),
            _ => {}
        }
    }

    WatcherInfo {
        name: config.name.clone(),
        watch_folder: config.watch_folder.clone(),
        output_folder: config.output_folder.clone(),
        watch_type: type_name.to_string(),
        video_rules,
        audio_rules,
        image_rules,
        pdf_rules,
        document_rules,
        custom_rules,
    }
}

fn format_rules_video(rules: &[crate::config::watch::VideoRule]) -> Vec<String> {
    rules
        .iter()
        .map(|r| {
            let codec = r.codec.as_deref().unwrap_or("(preset)");
            let ext = r.output_ext.as_deref().unwrap_or("(preset)");
            if let Some(ref fmt) = r.subfolder {
                format!("{} ({}, {})", fmt, codec, ext)
            } else {
                format!("{:?} -> {} ({})", r.input_extensions, ext, codec)
            }
        })
        .collect()
}

fn format_rules_image(rules: &[crate::config::watch::ImageRule]) -> Vec<String> {
    rules
        .iter()
        .map(|r| {
            let ext = r.output_ext.as_deref().unwrap_or("(preset)");
            if let Some(ref fmt) = r.subfolder {
                format!("{} ({})", fmt, ext)
            } else {
                format!("{:?} -> {}", r.input_extensions, ext)
            }
        })
        .collect()
}

fn format_rules_audio(rules: &[crate::config::watch::AudioRule]) -> Vec<String> {
    rules
        .iter()
        .map(|r| {
            let codec = r.audio_codec.as_deref().unwrap_or("(preset)");
            let ext = r.output_ext.as_deref().unwrap_or("(preset)");
            if let Some(ref fmt) = r.subfolder {
                format!("{} ({}, {})", fmt, codec, ext)
            } else {
                format!("{:?} -> {} ({})", r.input_extensions, ext, codec)
            }
        })
        .collect()
}

fn format_rules_pdf(rules: &[crate::config::watch::PdfRule]) -> Vec<String> {
    rules
        .iter()
        .map(|r| {
            let ext = r.output_ext.as_deref().unwrap_or("(preset)");
            if let Some(ref fmt) = r.subfolder {
                format!("{} ({:?})", fmt, r.mode)
            } else {
                format!("{:?} -> {} ({:?})", r.input_extensions, ext, r.mode)
            }
        })
        .collect()
}

fn format_rules_document(rules: &[crate::config::watch::DocumentRule]) -> Vec<String> {
    rules
        .iter()
        .map(|r| {
            let ext = r.output_ext.as_deref().unwrap_or("(preset)");
            if let Some(ref fmt) = r.subfolder {
                format!("{} -> {}", fmt, ext)
            } else {
                format!("{:?} -> {}", r.input_extensions, ext)
            }
        })
        .collect()
}

fn format_rules_custom(rules: &[crate::config::watch::CustomRule]) -> Vec<String> {
    rules
        .iter()
        .map(|r| {
            let desc = r
                .description
                .as_deref()
                .unwrap_or(r.command.as_deref().unwrap_or("(preset)"));
            if let Some(ref fmt) = r.subfolder {
                format!("{}: {}", fmt, desc)
            } else {
                format!("{:?}: {}", r.input_extensions, desc)
            }
        })
        .collect()
}
