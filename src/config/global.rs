use serde::{Deserialize, Serialize};

fn default_file_check_interval() -> u64 { 2000 }
fn default_stable_time() -> u64 { 5000 }
fn default_ffmpeg_path() -> String { "ffmpeg".to_string() }
fn default_max_concurrent() -> u32 { 4 }
fn default_config_refresh() -> u64 { 300 }
fn default_inputs_dir() -> String { "./inputs".to_string() }
fn default_outputs_dir() -> String { "./outputs".to_string() }
fn default_watchs_dir() -> String { "./config/watchs".to_string() }
fn default_log_config() -> LogConfig { LogConfig::default() }
fn default_healthcheck() -> HealthcheckConfig { HealthcheckConfig::default() }
fn default_disk_space() -> DiskSpaceConfig { DiskSpaceConfig::default() }
fn default_history() -> HistoryConfig { HistoryConfig::default() }

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GlobalConfig {
    #[serde(default = "default_file_check_interval")]
    pub file_check_interval_ms: u64,

    #[serde(default = "default_stable_time")]
    pub stable_time_ms: u64,

    #[serde(default = "default_ffmpeg_path")]
    pub ffmpeg_path: String,

    #[serde(default)]
    pub ffprobe_path: String,

    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_conversions: u32,

    #[serde(default = "default_config_refresh")]
    pub config_refresh_interval_s: u64,

    #[serde(default = "default_inputs_dir")]
    pub inputs_dir: String,

    #[serde(default = "default_outputs_dir")]
    pub outputs_dir: String,

    #[serde(default)]
    pub embedded_scan_interval_s: u64,

    #[serde(default)]
    pub embedded_secret: String,

    #[serde(default = "default_watchs_dir")]
    pub watchs_dir: String,

    #[serde(default)]
    pub scan_embedded_configs: bool,

    #[serde(default = "default_log_config")]
    pub log: LogConfig,

    #[serde(default = "default_healthcheck")]
    pub healthcheck: HealthcheckConfig,

    #[serde(default = "default_disk_space")]
    pub disk_space: DiskSpaceConfig,

    #[serde(default = "default_history")]
    pub history: HistoryConfig,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            file_check_interval_ms: default_file_check_interval(),
            stable_time_ms: default_stable_time(),
            ffmpeg_path: default_ffmpeg_path(),
            ffprobe_path: String::new(),
            max_concurrent_conversions: default_max_concurrent(),
            config_refresh_interval_s: default_config_refresh(),
            inputs_dir: default_inputs_dir(),
            outputs_dir: default_outputs_dir(),
            embedded_scan_interval_s: 0,
            embedded_secret: String::new(),
            watchs_dir: default_watchs_dir(),
            scan_embedded_configs: false,
            log: default_log_config(),
            healthcheck: default_healthcheck(),
            disk_space: default_disk_space(),
            history: default_history(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogConfig {
    pub errors_file: String,
    pub max_log_files: u32,
    pub max_log_size_mb: u64,
    pub max_error_log_size_mb: u64,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            errors_file: "./logs/errors.log".to_string(),
            max_log_files: 30,
            max_log_size_mb: 100,
            max_error_log_size_mb: 50,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HealthcheckConfig {
    pub http_port: u16,
    pub bind_address: String,
}

impl Default for HealthcheckConfig {
    fn default() -> Self {
        Self {
            http_port: 8080,
            bind_address: "0.0.0.0".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiskSpaceConfig {
    pub check_interval_s: u64,
    pub threshold: DiskSpaceThreshold,
    pub check_output: bool,
    pub check_watch: bool,
}

impl Default for DiskSpaceConfig {
    fn default() -> Self {
        Self {
            check_interval_s: 60,
            threshold: DiskSpaceThreshold::Mb(500),
            check_output: false,
            check_watch: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DiskSpaceThreshold {
    Mb(u64),
    Gb(f64),
    Percent(f64),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HistoryConfig {
    pub persistent: bool,
    pub file: String,
    pub max_records: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            persistent: false,
            file: "./logs/history.json".to_string(),
            max_records: 500,
        }
    }
}
