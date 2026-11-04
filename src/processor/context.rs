use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::config::global::DiskSpaceConfig;
use crate::health::server::HealthServer;
use crate::logs::error_logger::ErrorLogger;
use crate::processor::job::ConversionJob;

/// Default timeout for conversion jobs (3600 seconds = 1 hour).
pub const DEFAULT_TIMEOUT_S: u64 = 3600;

/// Shared dependencies for all conversion jobs.
///
/// Created once at startup and cloned (Arc) into every job. The timeout is
/// stored as an `AtomicU64` so hot-reload can update it without rebuilding
/// the whole context.
#[derive(Clone)]
pub struct ProcessorDeps {
    pub health_server: Arc<HealthServer>,
    pub error_logger: Arc<ErrorLogger>,
    pub disk_config: DiskSpaceConfig,
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
    pub timeout_secs: Arc<AtomicU64>,
}

/// Per-job context: the job plus a handle to the shared dependencies.
pub struct JobContext {
    pub job: ConversionJob,
    pub deps: Arc<ProcessorDeps>,
}

impl ProcessorDeps {
    /// Resolve the effective timeout in seconds (0 -> DEFAULT_TIMEOUT_S).
    pub fn timeout_secs(&self) -> u64 {
        let v = self.timeout_secs.load(Ordering::Relaxed);
        if v == 0 {
            DEFAULT_TIMEOUT_S
        } else {
            v
        }
    }

    /// Resolve the effective timeout as a `Duration`.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs())
    }
}
