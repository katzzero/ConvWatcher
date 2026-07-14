use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use log::{error, info, warn};

use crate::config::global::InputFileAction;
use crate::health::server::{ConversionRecord, HealthServer};
use crate::logs::error_logger::ErrorLogger;

const DEFAULT_TIMEOUT_S: u64 = 3600;

/// Shared success/error bookkeeping for every processor.
///
/// Each `process_*` function builds the output path, then delegates the actual
/// conversion to `convert` (which must return the output path on success, or an
/// error). This helper handles health-server state, history records and
/// input-file post-processing so the individual processors stay tiny.
pub async fn run_conversion<F, Fut>(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    error_logger: Arc<ErrorLogger>,
    health_server: Arc<HealthServer>,
    input_file_action: InputFileAction,
    op_label: &'static str,
    convert: F,
) where
    F: FnOnce() -> Fut + Send,
    Fut: std::future::Future<Output = Result<String>> + Send,
{
    let _ = health_server.set_processing(watcher_name.clone(), file_name.clone());
    let _ = health_server.dequeue(&file_name);
    info!("[Processor] Processing started: {}", file_name);

    let timeout = Duration::from_secs(DEFAULT_TIMEOUT_S);
    let result = match tokio::time::timeout(timeout, convert()).await {
        Ok(result) => result,
        Err(_) => {
            let msg = format!("{} conversion timed out after {}s: {}", op_label, DEFAULT_TIMEOUT_S, file_name);
            error!("{}", msg);
            error_logger.log(&msg, &file_name, op_label);
            let _ = health_server.increment_error(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "error".to_string(),
                output: String::new(),
            }).await;
            crate::utils::path::handle_input_file(&file_path, &input_file_action, false);
            info!("[Processor] Job finished: {}", file_name);
            let _ = health_server.clear_processing(&watcher_name);
            return;
        }
    };

    match result {
        Ok(output) => {
            info!("{} conversion succeeded: {}", op_label, file_name);
            let _ = health_server.increment_processed(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "done".to_string(),
                output,
            }).await;
            crate::utils::path::handle_input_file(&file_path, &input_file_action, true);
        }
        Err(e) => {
            let msg = format!("{} conversion failed: {}", op_label, e);
            error!("{}", msg);
            warn!("[Processor] Error discarded, continuing: {}", file_name);
            error_logger.log(&msg, &file_name, op_label);
            let _ = health_server.increment_error(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "error".to_string(),
                output: String::new(),
            }).await;
            crate::utils::path::handle_input_file(&file_path, &input_file_action, false);
        }
    }

    info!("[Processor] Job finished: {}", file_name);
    let _ = health_server.clear_processing(&watcher_name);
}
