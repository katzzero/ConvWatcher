use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::{error, info};
use tokio::process::Command;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::VideoRule;
use crate::health::server::{ConversionRecord, HealthServer};
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;

pub async fn process_video(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    rule: &VideoRule,
    output_folder: &str,
    watch_folder: &str,
    error_logger: Arc<ErrorLogger>,
    health_server: Arc<HealthServer>,
    disk_config: &DiskSpaceConfig,
    ffmpeg_path: &str,
    ffprobe_path: &str,
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
        &rule.codec,
        ext,
    ) {
        Ok(p) => p,
        Err(_) => OutputNamer::generate_with_counter(&output_folder_path, &base_name, "video", ext),
    };

    match convert_video(&file_path, &output_path, rule, ffmpeg_path, ffprobe_path).await {
        Ok(()) => {
            info!("Video conversion succeeded: {}", file_name);
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
            let msg = format!("Video conversion failed: {}", e);
            error!("{}", msg);
            error_logger.log(&msg, &file_name, "video::process");
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

async fn convert_video(input: &Path, output: &Path, rule: &VideoRule, ffmpeg_path: &str, ffprobe_path: &str) -> Result<()> {
    let quality_args = parse_quality_value(&rule.quality);

    let mut cmd = Command::new(ffmpeg_path);
    cmd.arg("-y")
        .arg("-i")
        .arg(input.as_os_str())
        .arg("-c:v")
        .arg(&rule.codec);

    for arg in &quality_args {
        cmd.arg(arg);
    }

    cmd.arg("-c:a")
        .arg(&rule.audio_codec)
        .arg("-b:a")
        .arg(&rule.audio_bitrate)
        .arg(output.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output_result = cmd.output().await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("FFmpeg failed: {}", stderr);
    }

    if rule.check_duration {
        let input_duration = get_video_duration(input, ffprobe_path).await.unwrap_or(0.0);
        let output_duration = get_video_duration(output, ffprobe_path).await.unwrap_or(0.0);

        if input_duration > 0.0 && output_duration < input_duration * rule.min_duration_ratio {
            bail!(
                "Duration mismatch: input={:.1}s output={:.1}s (min ratio: {})",
                input_duration,
                output_duration,
                rule.min_duration_ratio
            );
        }
    }

    Ok(())
}

pub fn parse_quality_value(quality_str: &str) -> Vec<String> {
    let trimmed = quality_str.trim();

    if trimmed.is_empty() {
        return vec!["-crf".to_string(), "23".to_string()];
    }

    let parts: Vec<&str> = trimmed.split_whitespace().collect();

    match parts[0].to_lowercase().as_str() {
        "crf" => {
            let value = parts.get(1).unwrap_or(&"23");
            vec!["-crf".to_string(), value.to_string()]
        }
        "vbr" => {
            let value = parts.get(1).unwrap_or(&"4");
            vec!["-q:v".to_string(), value.to_string()]
        }
        _ => {
            let first = parts[0];
            if first.ends_with('M') || first.ends_with('m') || first.ends_with('K')
                || first.ends_with('k')
            {
                vec!["-b:v".to_string(), first.to_string()]
            } else if first.parse::<u32>().is_ok() {
                vec!["-crf".to_string(), first.to_string()]
            } else {
                vec!["-crf".to_string(), "23".to_string()]
            }
        }
    }
}

pub async fn get_video_duration(path: &Path, ffprobe_path: &str) -> Result<f64> {
    let output = Command::new(ffprobe_path)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let duration: f64 = stdout.trim().parse().unwrap_or(0.0);
    Ok(duration)
}
