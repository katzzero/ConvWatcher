use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::warn;
use tokio::process::Command;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::AudioRule;
use crate::health::server::HealthServer;
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;

pub async fn process_audio(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    rule: &AudioRule,
    output_folder: &str,
    watch_folder: &str,
    error_logger: Arc<ErrorLogger>,
    health_server: Arc<HealthServer>,
    disk_config: &DiskSpaceConfig,
    ffmpeg_path: &str,
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
    let ext = rule.output_ext.as_deref().unwrap_or(".mp3").trim_start_matches('.');
    let output_path = match OutputNamer::generate_path(
        &output_folder_path,
        &base_name,
        rule.output_name.as_deref().unwrap_or("{base}_{codec}_{num}.{ext}"),
        rule.audio_codec.as_deref().unwrap_or("libmp3lame"),
        ext,
    ) {
        Ok(p) => p,
        Err(_) => OutputNamer::generate_with_counter(&output_folder_path, &base_name, "audio", ext),
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
        "audio",
        || async move {
            convert_audio(&input_path, &output_path, rule, ffmpeg_path).await?;
            Ok(output_path.to_string_lossy().to_string())
        },
    )
    .await;
}

async fn convert_audio(input: &Path, output: &Path, rule: &AudioRule, ffmpeg_path: &str) -> Result<()> {
    let args = build_audio_args(rule);

    let mut cmd = Command::new(ffmpeg_path);
    cmd.arg("-y")
        .arg("-i")
        .arg(input.as_os_str());

    for arg in &args {
        cmd.arg(arg);
    }

    cmd.arg(output.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output_result = cmd.output().await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("FFmpeg failed: {}", stderr);
    }

    Ok(())
}

fn build_audio_args(rule: &AudioRule) -> Vec<String> {
    let mut args = Vec::new();

    args.push("-vn".to_string());

    let audio_codec = rule.audio_codec.as_deref().unwrap_or("libmp3lame");
    args.push("-c:a".to_string());
    args.push(audio_codec.to_string());

    if audio_codec != "copy" {
        if let Some(ref bitrate) = rule.audio_bitrate {
            args.push("-b:a".to_string());
            args.push(bitrate.clone());
        }
    }

    if let Some(sr) = rule.sample_rate {
        args.push("-ar".to_string());
        args.push(sr.to_string());
    }

    if let Some(ch) = rule.channels {
        args.push("-ac".to_string());
        args.push(ch.to_string());
    }

    args
}
