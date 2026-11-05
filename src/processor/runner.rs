use std::path::{Path, PathBuf};

use anyhow::Result;
use log::{error, info, warn};

use crate::config::global::InputFileAction;
use crate::health::server::ConversionRecord;
use crate::processor::context::ProcessorDeps;

/// Shared success/error bookkeeping for every processor.
///
/// Each `process_*` function builds the output path, then delegates the actual
/// conversion to `convert` (which must return the output path on success, or an
/// error). This helper handles health-server state, history records and
/// input-file post-processing so the individual processors stay tiny.
#[allow(clippy::too_many_arguments)]
pub async fn run_conversion<F, Fut>(
    deps: &ProcessorDeps,
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    output_path: &Path,
    input_file_action: InputFileAction,
    op_label: &'static str,
    convert: F,
) where
    F: FnOnce() -> Fut + Send,
    Fut: std::future::Future<Output = Result<String>> + Send,
{
    let _ = deps
        .health_server
        .set_processing(watcher_name.clone(), file_name.clone());
    let _ = deps.health_server.dequeue(&watcher_name, &file_name);
    info!("[Processor] Processing started: {}", file_name);

    let timeout_secs = deps.timeout_secs();
    let timeout = deps.timeout();
    let result = match tokio::time::timeout(timeout, convert()).await {
        Ok(result) => result,
        Err(_) => {
            let msg = format!(
                "{} conversion timed out after {}s: {}",
                op_label, timeout_secs, file_name
            );
            error!("{}", msg);
            deps.error_logger.log(&msg, &file_name, op_label);
            cleanup_partial_output(output_path);
            let _ = deps.health_server.increment_error(&watcher_name);
            let _ = deps
                .health_server
                .add_history(ConversionRecord {
                    time: chrono::Local::now().format("%H:%M:%S").to_string(),
                    watcher: watcher_name.clone(),
                    file: file_name.clone(),
                    status: "error".to_string(),
                    output: error_summary(&msg),
                })
                .await;
            crate::utils::path::handle_input_file(&file_path, &input_file_action, false);
            info!("[Processor] Job finished: {}", file_name);
            let _ = deps
                .health_server
                .clear_processing(&watcher_name, &file_name);
            return;
        }
    };

    match result {
        Ok(output) => {
            info!("{} conversion succeeded: {}", op_label, file_name);
            let _ = deps.health_server.increment_processed(&watcher_name);
            let _ = deps
                .health_server
                .add_history(ConversionRecord {
                    time: chrono::Local::now().format("%H:%M:%S").to_string(),
                    watcher: watcher_name.clone(),
                    file: file_name.clone(),
                    status: "done".to_string(),
                    output,
                })
                .await;
            crate::utils::path::handle_input_file(&file_path, &input_file_action, true);
        }
        Err(e) => {
            let msg = format!("{} conversion failed: {}", op_label, e);
            error!("{}", msg);
            warn!("[Processor] Error discarded, continuing: {}", file_name);
            deps.error_logger.log(&msg, &file_name, op_label);
            cleanup_partial_output(output_path);
            let _ = deps.health_server.increment_error(&watcher_name);
            let _ = deps
                .health_server
                .add_history(ConversionRecord {
                    time: chrono::Local::now().format("%H:%M:%S").to_string(),
                    watcher: watcher_name.clone(),
                    file: file_name.clone(),
                    status: "error".to_string(),
                    output: error_summary(&msg),
                })
                .await;
            crate::utils::path::handle_input_file(&file_path, &input_file_action, false);
        }
    }

    info!("[Processor] Job finished: {}", file_name);
    let _ = deps
        .health_server
        .clear_processing(&watcher_name, &file_name);
}

fn error_summary(msg: &str) -> String {
    const MAX: usize = 1000;
    const KEEP_LINES: usize = 8;

    // FFmpeg prints a long banner before the real error. Keep the tail so the
    // dashboard history shows the actionable message, not the build info.
    let mut lines: Vec<&str> = msg
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("lib")
                && !line.starts_with("configuration:")
                && !line.starts_with("ffmpeg version")
                && !line.starts_with("built with")
        })
        .collect();
    if lines.len() > KEEP_LINES {
        lines = lines.split_off(lines.len() - KEEP_LINES);
    }
    let mut summary = lines.join(" | ").replace('\n', " ");
    if summary.chars().count() > MAX {
        summary = summary.chars().take(MAX).collect();
        summary.push('…');
    }
    summary
}

fn cleanup_partial_output(path: &Path) {
    if path.exists() {
        if let Err(e) = std::fs::remove_file(path) {
            warn!("Failed to clean up partial output {:?}: {}", path, e);
        } else {
            info!("Cleaned up partial output: {:?}", path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_summary_keeps_actionable_tail() {
        let msg = "ffmpeg version 8.1\nconfiguration: --enable-gpl\n  libavutil 60\n  libavcodec 62\n  libavformat 62\n  libavdevice 62\n  libavfilter 11\n  libswscale 9\n[in#0] invalid data\nError opening input file /x.mp4";
        let summary = error_summary(msg);
        assert!(summary.contains("Error opening input file"));
        assert!(!summary.contains("ffmpeg version 8.1"));
        assert!(!summary.contains("libavutil"));
        assert!(!summary.contains('\n'));
    }
}
