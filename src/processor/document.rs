use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::warn;
use tokio::process::Command;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::DocumentRule;
use crate::health::server::HealthServer;
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
    if check_disk_space(output_folder, watch_folder, disk_config).await {
        warn!(
            "Disk space low — pausing conversion of {} until space is freed",
            file_name
        );
        return;
    }

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

    let input_path = file_path.clone();
    let output_path_for_cleanup = output_path.clone();
    super::runner::run_conversion(
        watcher_name,
        file_name,
        file_path,
        &output_path_for_cleanup,
        error_logger,
        health_server,
        input_file_action,
        "document",
        || async move {
            convert_document(&input_path, &output_path, rule).await?;
            Ok(output_path.to_string_lossy().to_string())
        },
    )
    .await;
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
