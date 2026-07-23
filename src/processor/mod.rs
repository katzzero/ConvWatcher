pub mod audio;
pub mod disk;
pub mod document;
pub mod external;
pub mod image;
pub mod job;
pub mod namer;
pub mod pdf;
pub mod runner;
pub mod video;

use std::sync::Arc;

use crate::config::global::DiskSpaceConfig;
use crate::health::server::HealthServer;
use crate::logs::error_logger::ErrorLogger;
use job::{ConversionJob, MatchedRule};

/// Run a single conversion job locally using the appropriate processor.
///
/// This is the per-job body shared by the standalone daemon and the
/// coordinator's local-fallback path. It does not manage the concurrency
/// semaphore or the `processing_files` set — callers handle those.
pub async fn process_one(
    job: ConversionJob,
    health_server: Arc<HealthServer>,
    error_logger: Arc<ErrorLogger>,
    disk_config: DiskSpaceConfig,
    ffmpeg_path: String,
    ffprobe_path: String,
) {
    let action = job.input_file_action.clone();
    match job.matched_rule {
        MatchedRule::Video(ref rule) => {
            video::process_video(
                job.watcher_name.clone(),
                job.file_name.clone(),
                job.file_path.clone(),
                rule,
                &job.output_folder,
                &job.watch_folder,
                error_logger,
                health_server,
                &disk_config,
                &ffmpeg_path,
                &ffprobe_path,
                action,
            )
            .await;
        }
        MatchedRule::Image(ref rule) => {
            image::process_image(
                job.watcher_name.clone(),
                job.file_name.clone(),
                job.file_path.clone(),
                rule,
                &job.output_folder,
                &job.watch_folder,
                error_logger,
                health_server,
                &disk_config,
                action,
            )
            .await;
        }
        MatchedRule::Audio(ref rule) => {
            audio::process_audio(
                job.watcher_name.clone(),
                job.file_name.clone(),
                job.file_path.clone(),
                rule,
                &job.output_folder,
                &job.watch_folder,
                error_logger,
                health_server,
                &disk_config,
                &ffmpeg_path,
                action,
            )
            .await;
        }
        MatchedRule::Pdf(ref rule) => {
            pdf::process_pdf(
                job.watcher_name.clone(),
                job.file_name.clone(),
                job.file_path.clone(),
                rule,
                &job.output_folder,
                &job.watch_folder,
                error_logger,
                health_server,
                &disk_config,
                action,
            )
            .await;
        }
        MatchedRule::Document(ref rule) => {
            document::process_document(
                job.watcher_name.clone(),
                job.file_name.clone(),
                job.file_path.clone(),
                rule,
                &job.output_folder,
                &job.watch_folder,
                error_logger,
                health_server,
                &disk_config,
                action,
            )
            .await;
        }
        MatchedRule::Custom(ref rule) => {
            external::process_external(
                job.watcher_name.clone(),
                job.file_name.clone(),
                job.file_path.clone(),
                rule,
                &job.output_folder,
                &job.watch_folder,
                error_logger,
                health_server,
                &disk_config,
                action,
            )
            .await;
        }
    }
}
