use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize};

use super::codec_registry::CodecPresetPaths;

fn default_file_check_interval() -> u64 {
    2000
}
fn default_stable_time() -> u64 {
    5000
}
fn default_ffmpeg_path() -> String {
    "/usr/bin/ffmpeg".to_string()
}
fn default_max_concurrent() -> u32 {
    4
}
fn default_config_refresh() -> u64 {
    300
}
fn default_log_config() -> LogConfig {
    LogConfig::default()
}
fn default_healthcheck() -> HealthcheckConfig {
    HealthcheckConfig::default()
}
fn default_disk_space() -> DiskSpaceConfig {
    DiskSpaceConfig::default()
}
fn default_history() -> HistoryConfig {
    HistoryConfig::default()
}
fn default_codec_presets() -> CodecPresetPaths {
    CodecPresetPaths::default()
}
fn default_input_action() -> InputFileAction {
    InputFileAction::Mark
}
fn default_worker() -> WorkerConfig {
    WorkerConfig::default()
}

/// Parse a duration string like "2s", "5m", "500ms", "1h" into milliseconds.
/// A bare integer is treated as milliseconds.
fn parse_duration_to_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(ms_str) = s.strip_suffix("ms") {
        return ms_str.trim().parse().ok();
    }
    if let Some(s_str) = s.strip_suffix('s') {
        return s_str
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|n| *n >= 0.0)
            .map(|n| (n * 1000.0) as u64);
    }
    if let Some(m_str) = s.strip_suffix('m') {
        return m_str
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|n| *n >= 0.0)
            .map(|n| (n * 60_000.0) as u64);
    }
    if let Some(h_str) = s.strip_suffix('h') {
        return h_str
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|n| *n >= 0.0)
            .map(|n| (n * 3_600_000.0) as u64);
    }
    let v: u64 = s.parse().ok()?;
    Some(v)
}

/// Parse a duration string into seconds. A bare integer is treated as seconds.
fn parse_duration_to_s(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(ms_str) = s.strip_suffix("ms") {
        return ms_str
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|n| *n >= 0.0)
            .map(|n| (n / 1000.0).ceil() as u64);
    }
    if let Some(s_str) = s.strip_suffix('s') {
        return s_str.trim().parse::<f64>().ok().filter(|n| *n >= 0.0).map(|n| n as u64);
    }
    if let Some(m_str) = s.strip_suffix('m') {
        return m_str.trim().parse::<f64>().ok().filter(|n| *n >= 0.0).map(|n| (n * 60.0) as u64);
    }
    if let Some(h_str) = s.strip_suffix('h') {
        return h_str
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|n| *n >= 0.0)
            .map(|n| (n * 3600.0) as u64);
    }
    let v: u64 = s.parse().ok()?;
    Some(v)
}

struct DurationMsVisitor;

impl<'de> Visitor<'de> for DurationMsVisitor {
    type Value = u64;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a duration string (e.g. '2s', '5m', '500ms') or an integer (milliseconds)")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
        Ok(v)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> {
        if v < 0 {
            return Err(E::custom("duration must be non-negative"));
        }
        Ok(v as u64)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
        parse_duration_to_ms(v).ok_or_else(|| E::custom(format!("invalid duration: '{v}'")))
    }
}

fn deserialize_duration_ms<'de, D>(d: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    d.deserialize_any(DurationMsVisitor)
}

struct DurationSVisitor;

impl<'de> Visitor<'de> for DurationSVisitor {
    type Value = u64;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a duration string (e.g. '2s', '5m', '1h') or an integer (seconds)")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
        Ok(v)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> {
        if v < 0 {
            return Err(E::custom("duration must be non-negative"));
        }
        Ok(v as u64)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
        parse_duration_to_s(v).ok_or_else(|| E::custom(format!("invalid duration: '{v}'")))
    }
}

fn deserialize_duration_s<'de, D>(d: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    d.deserialize_any(DurationSVisitor)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalConfig {
    #[serde(
        default = "default_file_check_interval",
        alias = "file_check_interval",
        deserialize_with = "deserialize_duration_ms"
    )]
    pub file_check_interval_ms: u64,

    #[serde(
        default = "default_stable_time",
        alias = "stable_time",
        deserialize_with = "deserialize_duration_ms"
    )]
    pub stable_time_ms: u64,

    #[serde(default = "default_ffmpeg_path")]
    pub ffmpeg_path: String,

    #[serde(default)]
    pub ffprobe_path: Option<String>,

    #[serde(default = "default_max_concurrent", alias = "max_concurrent")]
    pub max_concurrent_conversions: u32,

    #[serde(
        default = "default_config_refresh",
        alias = "refresh_interval",
        deserialize_with = "deserialize_duration_s"
    )]
    pub config_refresh_interval_s: u64,

    #[serde(default)]
    pub embedded_secret: String,

    #[serde(default, alias = "embedded_scan_interval")]
    pub embedded_scan_interval_s: u64,

    #[serde(default = "default_codec_presets")]
    pub codec_presets: CodecPresetPaths,

    #[serde(default = "default_log_config")]
    pub log: LogConfig,

    #[serde(default = "default_healthcheck")]
    pub healthcheck: HealthcheckConfig,

    #[serde(default = "default_disk_space")]
    pub disk_space: DiskSpaceConfig,

    #[serde(default = "default_history")]
    pub history: HistoryConfig,

    #[serde(default = "default_input_action")]
    pub input_file_action: InputFileAction,

    #[serde(default = "default_worker")]
    pub worker: WorkerConfig,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            file_check_interval_ms: default_file_check_interval(),
            stable_time_ms: default_stable_time(),
            ffmpeg_path: default_ffmpeg_path(),
            ffprobe_path: None,
            max_concurrent_conversions: default_max_concurrent(),
            config_refresh_interval_s: default_config_refresh(),
            embedded_secret: String::new(),
            embedded_scan_interval_s: 0,
            codec_presets: default_codec_presets(),
            log: default_log_config(),
            healthcheck: default_healthcheck(),
            disk_space: default_disk_space(),
            history: default_history(),
            input_file_action: default_input_action(),
            worker: default_worker(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InputFileAction {
    Mark,
    Delete,
    None,
}

impl Default for InputFileAction {
    fn default() -> Self {
        Self::Mark
    }
}

/// Configuration for the coordinator's remote worker pool. Only used by the
/// `convwatcher-server` binary; ignored by the standalone daemon.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    /// Interface the coordinator binds its TCP listener + discovery to.
    #[serde(default = "default_worker_bind")]
    pub bind_address: String,

    /// Address advertised to agents in discovery replies (defaults to the
    /// coordinator's LAN IP; set explicitly if auto-detection is wrong).
    #[serde(default)]
    pub advertise_address: Option<String>,

    /// UDP discovery port.
    #[serde(default = "default_discovery_port")]
    pub discovery_port: u16,

    /// TCP port agents connect to.
    #[serde(default = "default_coordinator_port")]
    pub coordinator_port: u16,
}

fn default_worker_bind() -> String {
    "0.0.0.0".to_string()
}
fn default_discovery_port() -> u16 {
    8687
}
fn default_coordinator_port() -> u16 {
    8688
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            bind_address: default_worker_bind(),
            advertise_address: None,
            discovery_port: default_discovery_port(),
            coordinator_port: default_coordinator_port(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    #[serde(default = "default_errors_file")]
    pub errors_file: String,
    #[serde(default = "default_max_log_files")]
    pub max_log_files: u32,
    #[serde(default = "default_max_log_size_mb")]
    pub max_log_size_mb: u64,
    #[serde(default = "default_max_error_log_size_mb")]
    pub max_error_log_size_mb: u64,
}

fn default_errors_file() -> String {
    "./logs/errors.log".to_string()
}
fn default_max_log_files() -> u32 {
    30
}
fn default_max_log_size_mb() -> u64 {
    100
}
fn default_max_error_log_size_mb() -> u64 {
    50
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            errors_file: default_errors_file(),
            max_log_files: default_max_log_files(),
            max_log_size_mb: default_max_log_size_mb(),
            max_error_log_size_mb: default_max_error_log_size_mb(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HealthcheckConfig {
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
}

fn default_http_port() -> u16 {
    8080
}
fn default_bind_address() -> String {
    "127.0.0.1".to_string()
}

impl Default for HealthcheckConfig {
    fn default() -> Self {
        Self {
            http_port: default_http_port(),
            bind_address: default_bind_address(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiskSpaceConfig {
    #[serde(
        default = "default_disk_check_interval",
        alias = "check_interval",
        deserialize_with = "deserialize_duration_s"
    )]
    pub check_interval_s: u64,
    #[serde(default = "default_disk_threshold")]
    pub threshold: DiskSpaceThreshold,
    #[serde(default)]
    pub check_output: bool,
    #[serde(default)]
    pub check_watch: bool,
}

fn default_disk_check_interval() -> u64 {
    60
}
fn default_disk_threshold() -> DiskSpaceThreshold {
    DiskSpaceThreshold::Mb(500)
}

impl Default for DiskSpaceConfig {
    fn default() -> Self {
        Self {
            check_interval_s: default_disk_check_interval(),
            threshold: default_disk_threshold(),
            check_output: false,
            check_watch: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub enum DiskSpaceThreshold {
    Mb(u64),
    Gb(f64),
    Percent(f64),
}

struct DiskSpaceThresholdVisitor;

impl<'de> Visitor<'de> for DiskSpaceThresholdVisitor {
    type Value = DiskSpaceThreshold;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a disk-space threshold: integer (MB), '<n>Gb', or '<n>%'")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<DiskSpaceThreshold, E> {
        Ok(DiskSpaceThreshold::Mb(v))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<DiskSpaceThreshold, E> {
        Ok(DiskSpaceThreshold::Mb(v as u64))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<DiskSpaceThreshold, E> {
        let v = v.trim();
        if let Some(pct) = v.strip_suffix('%') {
            let n: f64 = pct.trim().parse().map_err(E::custom)?;
            return Ok(DiskSpaceThreshold::Percent(n));
        }
        if let Some(gb) = v.strip_suffix("Gb").or_else(|| v.strip_suffix("gb")).or_else(|| v.strip_suffix("GB")) {
            let n: f64 = gb.trim().parse().map_err(E::custom)?;
            return Ok(DiskSpaceThreshold::Gb(n));
        }
        let n: u64 = v.parse().map_err(|_| E::custom(format!("invalid disk-space threshold: '{v}'")))?;
        Ok(DiskSpaceThreshold::Mb(n))
    }
}

impl<'de> Deserialize<'de> for DiskSpaceThreshold {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(DiskSpaceThresholdVisitor)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryConfig {
    #[serde(default)]
    pub persistent: bool,
    #[serde(default = "default_history_file")]
    pub file: String,
    #[serde(default = "default_max_records")]
    pub max_records: usize,
}

fn default_history_file() -> String {
    "./logs/history.json".to_string()
}
fn default_max_records() -> usize {
    500
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            persistent: false,
            file: default_history_file(),
            max_records: default_max_records(),
        }
    }
}
