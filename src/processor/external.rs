use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::{error, info};
use tokio::process::Command;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::CustomRule;
use crate::health::server::{ConversionRecord, HealthServer};
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;

const DANGEROUS_CHARS: &[&str] = &[";", "&&", "||", "|", "`", "$(", "\n", "\r"];

pub async fn process_external(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    rule: &CustomRule,
    output_folder: &str,
    watch_folder: &str,
    error_logger: Arc<ErrorLogger>,
    health_server: Arc<HealthServer>,
    disk_config: &DiskSpaceConfig,
) {
    if let Err(e) = check_disk_space(output_folder, watch_folder, disk_config).await {
        error!("Disk space check failed: {}", e);
        let _ = health_server.increment_error(&watcher_name);
        return;
    }

    let _ = health_server.set_processing(watcher_name.clone(), file_name.clone());

    let output_folder_path = PathBuf::from(output_folder);
    let base_name = get_base_name(&file_name);
    let ext = rule.output_ext.trim_start_matches('.');
    let output_path = match OutputNamer::generate_path(
        &output_folder_path,
        &base_name,
        &rule.output_name_template,
        "custom",
        ext,
    ) {
        Ok(p) => p,
        Err(_) => {
            OutputNamer::generate_with_counter(&output_folder_path, &base_name, "custom", ext)
        }
    };

    match execute_custom(&file_path, &output_path, output_folder, &file_name, rule).await {
        Ok(()) => {
            info!("External conversion succeeded: {}", file_name);
            let _ = health_server.increment_processed(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "done".to_string(),
                output: output_path.to_string_lossy().to_string(),
            });
        }
        Err(e) => {
            let msg = format!("External conversion failed: {}", e);
            error!("{}", msg);
            error_logger.log(&msg, &file_name, "external::process");
            let _ = health_server.increment_error(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "error".to_string(),
                output: String::new(),
            });
        }
    }

    let _ = health_server.clear_processing(&watcher_name);
}

async fn execute_custom(
    input: &Path,
    output: &Path,
    output_folder: &str,
    file_name: &str,
    rule: &CustomRule,
) -> Result<()> {
    validate_command_template(&rule.command)?;

    let basename = get_base_name(file_name);
    let ext = rule.output_ext.trim_start_matches('.');

    let expanded = rule
        .command
        .replace("{input}", &input.to_string_lossy())
        .replace("{output}", &output.to_string_lossy())
        .replace("{basename}", &basename)
        .replace("{ext}", ext)
        .replace("{output_folder}", output_folder);

    validate_placeholder_values(&expanded)?;

    let parts: Vec<&str> = expanded.split_whitespace().collect();
    if parts.is_empty() {
        bail!("Empty command");
    }

    let program = parts[0];
    let args: Vec<&str> = parts[1..].to_vec();

    let output_result = Command::new(program)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("External command failed: {}", stderr);
    }

    Ok(())
}

fn validate_command_template(template: &str) -> Result<()> {
    if template.contains("..") {
        bail!("Template contains '..' path traversal");
    }
    Ok(())
}

fn validate_placeholder_values(value: &str) -> Result<()> {
    if value.contains("..") {
        bail!("Expanded value contains '..' path traversal");
    }
    for &dangerous in DANGEROUS_CHARS {
        if value.contains(dangerous) {
            bail!("Expanded value contains dangerous character: {}", dangerous);
        }
    }
    Ok(())
}
