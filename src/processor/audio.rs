use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Result};
use log::{error, warn};
use tokio::process::Command;

use crate::config::watch::AudioRule;
use crate::health::server::ConversionRecord;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;
use super::progress::{probe_duration, run_ffmpeg_with_progress, ProgressReporter};
#[allow(clippy::too_many_arguments)]
pub async fn process_audio(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    rule: &AudioRule,
    output_folder: &str,
    watch_folder: &str,
    deps: &crate::processor::context::ProcessorDeps,
    ffmpeg_path: &str,
    input_file_action: crate::config::global::InputFileAction,
) {
    if check_disk_space(output_folder, watch_folder, &deps.disk_config).await {
        let msg = format!(
            "Disk space low — pausing conversion of {} until space is freed",
            file_name
        );
        warn!("{}", msg);
        deps.error_logger.log(&msg, &file_name, "audio");
        let _ = deps.health_server.increment_error(&watcher_name);
        let _ = deps
            .health_server
            .add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "error".to_string(),
                output: String::new(),
            })
            .await;
        return;
    }

    let output_folder_path = PathBuf::from(output_folder);
    let base_name = get_base_name(&file_name);
    let ext = rule
        .output_ext
        .as_deref()
        .unwrap_or(".mp3")
        .trim_start_matches('.');
    let output_path = match OutputNamer::generate_path(
        &output_folder_path,
        &base_name,
        rule.output_name
            .as_deref()
            .unwrap_or("{base}_{codec}_{num}.{ext}"),
        rule.audio_codec.as_deref().unwrap_or("libmp3lame"),
        ext,
    ) {
        Ok(p) => p,
        Err(e) => {
            warn!(
                "generate_path failed for {}: {}; falling back",
                file_name, e
            );
            match OutputNamer::generate_with_counter(&output_folder_path, &base_name, "audio", ext)
            {
                Ok(p) => p,
                Err(e2) => {
                    error!(
                        "Final output path generation failed for {}: {}; aborting",
                        file_name, e2
                    );
                    return;
                }
            }
        }
    };

    let input_path = file_path.clone();
    let output_path_for_cleanup = output_path.clone();
    let reporter = ProgressReporter::new(
        deps.health_server.clone(),
        watcher_name.clone(),
        file_name.clone(),
    );
    let ffmpeg_path = ffmpeg_path.to_string();
    let ffprobe_path = deps.ffprobe_path.clone();
    let rule_clone = rule.clone();
    super::runner::run_conversion(
        deps,
        watcher_name,
        file_name,
        file_path,
        &output_path_for_cleanup,
        input_file_action,
        "audio",
        move || async move {
            convert_audio(
                &input_path,
                &output_path,
                &rule_clone,
                &ffmpeg_path,
                &ffprobe_path,
                &reporter,
            )
            .await?;
            Ok(output_path.to_string_lossy().to_string())
        },
    )
    .await;
}

async fn convert_audio(
    input: &Path,
    output: &Path,
    rule: &AudioRule,
    ffmpeg_path: &str,
    ffprobe_path: &str,
    reporter: &ProgressReporter,
) -> Result<()> {
    let args = build_audio_args(rule);
    let is_copy = rule.audio_codec.as_deref().unwrap_or("libmp3lame") == "copy";

    // Duração do input via ffprobe ANTES do ffmpeg (necessária para percentual/ETA).
    let duration_s = probe_duration(ffprobe_path, input).await.unwrap_or(None);
    if let Some(d) = duration_s {
        reporter.set_duration(d);
    }

    let mut cmd = Command::new(ffmpeg_path);
    cmd.kill_on_drop(true);
    cmd.arg("-y").arg("-i").arg(input.as_os_str());

    for arg in &args {
        cmd.arg(arg);
    }

    // Apenas codecs de codificação real emitem `-progress pipe:1`. `copy` não
    // gera progresso (é rápido).
    if !is_copy {
        cmd.arg("-progress").arg("pipe:1").arg("-nostats");
    }

    cmd.arg(output.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output_result = run_ffmpeg_with_progress(&mut cmd, reporter).await?;

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
