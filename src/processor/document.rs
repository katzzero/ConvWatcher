use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::{error, info, warn};
use tokio::process::Command;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::DocumentRule;
use crate::health::server::{ConversionRecord, HealthServer};
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;

pub async fn process_document(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    rule: &DocumentRule,
    output_folder: &str,
    watch_folder: &str,
    error_logger: Arc<ErrorLogger>,
    health_server: Arc<HealthServer>,
    disk_config: &DiskSpaceConfig,
    input_file_action: crate::config::global::InputFileAction,
) {
    check_disk_space(output_folder, watch_folder, disk_config).await;

    let _ = health_server.set_processing(watcher_name.clone(), file_name.clone());
    let _ = health_server.dequeue(&file_name);
    info!("[Processor] Processing started: {}", file_name);

    let output_folder_path = PathBuf::from(output_folder);
    let base_name = get_base_name(&file_name);
    let ext = rule.output_ext.as_deref().unwrap_or(".pdf").trim_start_matches('.');
    let output_path = match OutputNamer::generate_path(
        &output_folder_path,
        &base_name,
        rule.output_name.as_deref().unwrap_or("{base}_converted.{ext}"),
        "document",
        ext,
    ) {
        Ok(p) => p,
        Err(_) => {
            OutputNamer::generate_with_counter(&output_folder_path, &base_name, "document", ext)
        }
    };

    match convert_document(&file_path, &output_path, rule).await {
        Ok(()) => {
            info!("Document conversion succeeded: {}", file_name);
            let _ = health_server.increment_processed(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "done".to_string(),
                output: output_path.to_string_lossy().to_string(),
            });
            super::super::utils::path::handle_input_file(&file_path, &input_file_action, true);
        }
        Err(e) => {
            let msg = format!("Document conversion failed: {}", e);
            error!("{}", msg);
            warn!("[Processor] Error discarded, continuing: {}", file_name);
            error_logger.log(&msg, &file_name, "document::process");
            let _ = health_server.increment_error(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "error".to_string(),
                output: String::new(),
            });
            super::super::utils::path::handle_input_file(&file_path, &input_file_action, false);
        }
    }

    info!("[Processor] Job finished: {}", file_name);
    let _ = health_server.clear_processing(&watcher_name);
}

async fn convert_document(input: &Path, output: &Path, rule: &DocumentRule) -> Result<()> {
    let mut cmd = Command::new("pandoc");

    cmd.arg(input.as_os_str());
    cmd.arg("-o");
    cmd.arg(output.as_os_str());

    if rule.toc.unwrap_or(false) {
        cmd.arg("--toc");
    }

    if let Some(depth) = rule.toc_depth {
        cmd.arg(format!("--toc-depth={}", depth));
    }

    if let Some(ref css) = rule.css {
        cmd.arg(format!("--css={}", css));
    }

    if let Some(ref tmpl) = rule.template {
        cmd.arg(format!("--template={}", tmpl));
    }

    if rule.standalone.unwrap_or(false) {
        cmd.arg("-s");
    }

    if let Some(ref meta) = rule.metadata {
        for m in meta {
            cmd.arg(format!("-M{}", m));
        }
    }

    if let Some(ref engine) = rule.pdf_engine {
        cmd.arg(format!("--pdf-engine={}", engine));
    }

    if let Some(ref extra) = rule.options {
        for opt in extra {
            cmd.arg(opt);
        }
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let output_result = cmd.output().await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("Pandoc failed: {}", stderr);
    }

    Ok(())
}
