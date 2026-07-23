//! Routing of conversion jobs between remote agents and local processors.
//!
//! For video/audio jobs, if a worker agent is connected we run the conversion
//! remotely: the coordinator streams the input to the agent and writes the
//! returned bytes to the locally-computed output path. All the success/error
//! bookkeeping (history, processing state, input-file handling, timeout,
//! partial-output cleanup) is done here via the shared [`run_conversion`]
//! helper — exactly as local processing does — so remote and local jobs are
//! recorded identically. If no agent is available or the remote job fails, we
//! fall back to the local processors.

use std::path::PathBuf;
use std::sync::Arc;

use log::warn;

use convwatcher_common::protocol::{JobKind, WireAudioRule, WireVideoRule};

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::{AudioRule, VideoRule};
use crate::health::server::HealthServer;
use crate::logs::error_logger::ErrorLogger;
use crate::processor::disk::check_disk_space;
use crate::processor::job::{ConversionJob, MatchedRule};
use crate::processor::namer::OutputNamer;
use crate::processor::runner::run_conversion;
use crate::utils::path::get_base_name;

use super::coordinator::{RemoteJob, WorkerPool};

/// Entry point used by the server's job loop. Decides whether to run the job
/// remotely (video/audio + agent available) or locally.
#[allow(clippy::too_many_arguments)]
pub async fn route_job(
    job: ConversionJob,
    pool: Arc<WorkerPool>,
    health: Arc<HealthServer>,
    error_logger: Arc<ErrorLogger>,
    disk_config: DiskSpaceConfig,
    ffmpeg_path: String,
    ffprobe_path: String,
) {
    let can_remote = pool.agent_count().await > 0
        && matches!(
            job.matched_rule,
            MatchedRule::Video(_) | MatchedRule::Audio(_)
        );

    if can_remote {
        match &job.matched_rule {
            MatchedRule::Video(rule) => {
                remote_video(
                    job.clone(),
                    &pool,
                    &health,
                    &error_logger,
                    &disk_config,
                    rule.clone(),
                    ffmpeg_path,
                    ffprobe_path,
                )
                .await;
                return;
            }
            MatchedRule::Audio(rule) => {
                remote_audio(
                    job.clone(),
                    &pool,
                    &health,
                    &error_logger,
                    &disk_config,
                    rule.clone(),
                    ffmpeg_path,
                    ffprobe_path,
                )
                .await;
                return;
            }
            _ => {}
        }
    }

    // Local processing (image/pdf/document/custom, or no agent available).
    crate::processor::process_one(
        job,
        health,
        error_logger,
        disk_config,
        ffmpeg_path,
        ffprobe_path,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn remote_video(
    job: ConversionJob,
    pool: &Arc<WorkerPool>,
    health: &Arc<HealthServer>,
    error_logger: &Arc<ErrorLogger>,
    disk_config: &DiskSpaceConfig,
    rule: VideoRule,
    ffmpeg_path: String,
    ffprobe_path: String,
) {
    if check_disk_space(&job.output_folder, &job.watch_folder, disk_config).await {
        warn!("Disk space low — pausing conversion of {}", job.file_name);
        return;
    }

    let ext = rule
        .output_ext
        .as_deref()
        .unwrap_or(".mp4")
        .trim_start_matches('.')
        .to_string();
    let output_path = compute_output_path(
        &job.output_folder,
        &job.file_name,
        rule.output_name.as_deref(),
        rule.codec.as_deref().unwrap_or("libx264"),
        &ext,
        "video",
    );

    let wire = WireVideoRule {
        codec: rule.codec.clone(),
        quality: rule.quality.clone(),
        audio_codec: rule.audio_codec.clone(),
        audio_bitrate: rule.audio_bitrate.clone(),
        check_duration: rule.check_duration,
        min_duration_ratio: rule.min_duration_ratio,
    };

    run_remote(
        job,
        pool,
        health,
        error_logger,
        disk_config,
        output_path,
        ext,
        "video",
        JobKind::Video,
        Some(wire),
        None,
        ffmpeg_path,
        ffprobe_path,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn remote_audio(
    job: ConversionJob,
    pool: &Arc<WorkerPool>,
    health: &Arc<HealthServer>,
    error_logger: &Arc<ErrorLogger>,
    disk_config: &DiskSpaceConfig,
    rule: AudioRule,
    ffmpeg_path: String,
    ffprobe_path: String,
) {
    if check_disk_space(&job.output_folder, &job.watch_folder, disk_config).await {
        warn!("Disk space low — pausing conversion of {}", job.file_name);
        return;
    }

    let ext = rule
        .output_ext
        .as_deref()
        .unwrap_or(".mp3")
        .trim_start_matches('.')
        .to_string();
    let output_path = compute_output_path(
        &job.output_folder,
        &job.file_name,
        rule.output_name.as_deref(),
        rule.audio_codec.as_deref().unwrap_or("libmp3lame"),
        &ext,
        "audio",
    );

    let wire = WireAudioRule {
        audio_codec: rule.audio_codec.clone(),
        audio_bitrate: rule.audio_bitrate.clone(),
        sample_rate: rule.sample_rate,
        channels: rule.channels,
    };

    run_remote(
        job,
        pool,
        health,
        error_logger,
        disk_config,
        output_path,
        ext,
        "audio",
        JobKind::Audio,
        None,
        Some(wire),
        ffmpeg_path,
        ffprobe_path,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn run_remote(
    job: ConversionJob,
    pool: &Arc<WorkerPool>,
    health: &Arc<HealthServer>,
    error_logger: &Arc<ErrorLogger>,
    disk_config: &DiskSpaceConfig,
    output_path: PathBuf,
    output_ext: String,
    op_label: &'static str,
    kind: JobKind,
    video_rule: Option<WireVideoRule>,
    audio_rule: Option<WireAudioRule>,
    ffmpeg_path: String,
    ffprobe_path: String,
) {
    let input_path = job.file_path.clone();
    let output_for_convert = output_path.clone();

    let remote = RemoteJob {
        kind,
        video_rule,
        audio_rule,
        output_ext,
        input_path: &input_path,
        output_path: &output_for_convert,
    };

    match pool.dispatch(remote).await {
        Ok(true) => {
            run_conversion(
                job.watcher_name.clone(),
                job.file_name.clone(),
                job.file_path.clone(),
                &output_path,
                error_logger.clone(),
                health.clone(),
                job.input_file_action.clone(),
                op_label,
                move || async move {
                    Ok(output_for_convert.to_string_lossy().to_string())
                },
            )
            .await;
        }
        Ok(false) | Err(_) => {
            crate::processor::process_one(
                job,
                health.clone(),
                error_logger.clone(),
                disk_config.clone(),
                ffmpeg_path,
                ffprobe_path,
            )
            .await;
        }
    }
}

fn compute_output_path(
    output_folder: &str,
    file_name: &str,
    output_name: Option<&str>,
    codec: &str,
    ext: &str,
    fallback_label: &str,
) -> PathBuf {
    let folder = PathBuf::from(output_folder);
    let base = get_base_name(file_name);
    match OutputNamer::generate_path(
        &folder,
        &base,
        output_name.unwrap_or("{base}_{codec}_{num}.{ext}"),
        codec,
        ext,
    ) {
        Ok(p) => p,
        Err(_) => OutputNamer::generate_with_counter(&folder, &base, fallback_label, ext),
    }
}
