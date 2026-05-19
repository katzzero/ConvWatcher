use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
    pub rules: Vec<String>,
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

    pub fn with_app_log(mut self, path: String) -> Self {
        self.app_log_path = Some(path);
        self
    }

    pub fn with_hardware_info(self, info: HardwareAccelInfo) -> Self {
        *self.hw_info.lock().unwrap() = Some(info);
        self
    }

    pub fn with_history_persistence(
        mut self,
        file: &str,
        persistent: bool,
    ) -> Self {
        if persistent {
            self.history_file = Some(file.to_string());
            self.history_persistent = true;

            if let Ok(content) = std::fs::read_to_string(file) {
                if let Ok(records) = serde_json::from_str::<Vec<ConversionRecord>>(&content) {
                    *self.history.lock().unwrap() = records;
                }
            }
        }
        self
    }

    pub async fn add_watcher_with_config(&self, config: &WatchConfig) {
        let info = watcher_info_from_config(config);
        let mut watchers = self.watchers.lock().unwrap();
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
        let mut map = self.processing.lock().unwrap();
        map.insert(watcher, file);
        Ok(())
    }

    pub fn clear_processing(&self, watcher: &str) -> Result<()> {
        let mut map = self.processing.lock().unwrap();
        map.remove(watcher);
        Ok(())
    }

    pub fn enqueue(&self, file: &str) -> Result<()> {
        let mut queue = self.queue.lock().unwrap();
        let entry = queue.entry("global".to_string()).or_default();
        entry.push(file.to_string());
        Ok(())
    }

    pub async fn add_history(&self, record: ConversionRecord) -> Result<()> {
        let mut history = self.history.lock().unwrap();
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

        if self.history_persistent {
            if let Some(ref file) = self.history_file {
                if let Ok(json) = serde_json::to_string_pretty(&*history) {
                    let _ = std::fs::write(file, json);
                }
            }
        }

        Ok(())
    }

    pub async fn run(&self) -> Result<()> {
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

        for request in server.incoming_requests() {
            let url = request.url().to_string();
            let method = request.method().clone();
            let path = url.as_str();

            let json_ct = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
            let html_ct = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..]).unwrap();
            let text_ct = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/plain"[..]).unwrap();

            match (method, path) {
                (tiny_http::Method::Get, "/") | (tiny_http::Method::Get, "/dashboard") => {
                    let html = include_str!("dashboard.html");
                    let _ = request.respond(
                        tiny_http::Response::from_string(html)
                            .with_header(html_ct.clone()),
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
                    let watchers = self.watchers.lock().unwrap();
                    let queue = self.queue.lock().unwrap();
                    let processing = self.processing.lock().unwrap();
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
                        tiny_http::Response::from_string(body)
                            .with_header(json_ct.clone()),
                    );
                }
                (tiny_http::Method::Get, "/api/watchers") => {
                    let watchers = self.watchers.lock().unwrap();
                    let body = serde_json::to_string_pretty(&*watchers).unwrap_or_default();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_header(json_ct.clone()),
                    );
                }
                (tiny_http::Method::Get, "/api/queue") => {
                    let queue = self.queue.lock().unwrap();
                    let processing = self.processing.lock().unwrap();
                    let response = serde_json::json!({
                        "queue": &*queue,
                        "processing": &*processing,
                    });
                    let body = serde_json::to_string_pretty(&response).unwrap_or_default();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_header(json_ct.clone()),
                    );
                }
                (tiny_http::Method::Get, "/api/history") => {
                    let history = self.history.lock().unwrap();
                    let body = serde_json::to_string_pretty(&*history).unwrap_or_default();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_header(json_ct.clone()),
                    );
                }
                (tiny_http::Method::Get, "/logs") => {
                    match &self.app_log_path {
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
                    }
                }
                (tiny_http::Method::Get, "/logs/errors") => {
                    match &self.error_log_path {
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
                    }
                }
                (tiny_http::Method::Get, "/logs/app") => {
                    match &self.app_log_path {
                        Some(path) => match std::fs::read_to_string(path) {
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
                    }
                }
                _ => {
                    let _ = request.respond(
                        tiny_http::Response::from_string("Not Found")
                            .with_status_code(404),
                    );
                }
            }
        }

        Ok(())
    }
}

fn read_tail(path: &str, lines: usize) -> Result<String> {
    let content = std::fs::read_to_string(path)?;
    let all_lines: Vec<&str> = content.lines().collect();
    let start = if all_lines.len() > lines {
        all_lines.len() - lines
    } else {
        0
    };
    Ok(all_lines[start..].join("\n"))
}

pub fn watcher_info_from_config(config: &WatchConfig) -> WatcherInfo {
    let (type_name, rules) = match &config.watch_type {
        WatchType::Video { video } => ("video", format_rules_video(video)),
        WatchType::Image { image } => ("image", format_rules_image(image)),
        WatchType::Audio { audio } => ("audio", format_rules_audio(audio)),
        WatchType::Pdf { pdf } => ("pdf", format_rules_pdf(pdf)),
        WatchType::Document { document } => ("document", format_rules_document(document)),
        WatchType::Custom { custom } => ("custom", format_rules_custom(custom)),
    };

    WatcherInfo {
        name: config.name.clone(),
        watch_folder: config.watch_folder.clone(),
        output_folder: config.output_folder.clone(),
        watch_type: type_name.to_string(),
        rules,
    }
}

fn format_rules_video(rules: &[crate::config::watch::VideoRule]) -> Vec<String> {
    rules.iter().map(|r| {
        if let Some(ref fmt) = r.format {
            format!("{} ({}, {})", fmt, r.codec, r.output_ext)
        } else {
            format!("{:?} -> {} ({})", r.input_extensions, r.output_ext, r.codec)
        }
    }).collect()
}

fn format_rules_image(rules: &[crate::config::watch::ImageRule]) -> Vec<String> {
    rules.iter().map(|r| {
        if let Some(ref fmt) = r.format {
            format!("{} ({}, q{})", fmt, r.output_ext, r.quality)
        } else {
            format!("{:?} -> {} (q{})", r.input_extensions, r.output_ext, r.quality)
        }
    }).collect()
}

fn format_rules_audio(rules: &[crate::config::watch::AudioRule]) -> Vec<String> {
    rules.iter().map(|r| {
        if let Some(ref fmt) = r.format {
            format!("{} ({}, {})", fmt, r.audio_codec, r.audio_bitrate)
        } else {
            format!("{:?} -> {} ({}, {})", r.input_extensions, r.output_ext, r.audio_codec, r.audio_bitrate)
        }
    }).collect()
}

fn format_rules_pdf(rules: &[crate::config::watch::PdfRule]) -> Vec<String> {
    rules.iter().map(|r| {
        if let Some(ref fmt) = r.format {
            format!("{} ({:?})", fmt, r.mode)
        } else {
            format!("{:?} -> {} ({:?})", r.input_extensions, r.output_ext, r.mode)
        }
    }).collect()
}

fn format_rules_document(rules: &[crate::config::watch::DocumentRule]) -> Vec<String> {
    rules.iter().map(|r| {
        if let Some(ref fmt) = r.format {
            format!("{} -> {}", fmt, r.output_ext)
        } else {
            format!("{:?} -> {}", r.input_extensions, r.output_ext)
        }
    }).collect()
}

fn format_rules_custom(rules: &[crate::config::watch::CustomRule]) -> Vec<String> {
    rules.iter().map(|r| {
        if let Some(ref fmt) = r.format {
            format!("{}: {}", fmt, r.description.as_deref().unwrap_or(&r.command))
        } else {
            format!("{:?}: {}", r.input_extensions, r.description.as_deref().unwrap_or(&r.command))
        }
    }).collect()
}
