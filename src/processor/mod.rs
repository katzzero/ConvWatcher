pub mod audio;
pub mod context;
pub mod disk;
pub mod document;
pub mod external;
pub mod image;
pub mod job;
pub mod namer;
pub mod pdf;
pub mod runner;
pub mod video;

use crate::processor::context::ProcessorDeps;
use crate::processor::job::{ConversionJob, MatchedRule};

/// Run a single conversion job locally using the appropriate processor.
///
/// This is the per-job body used by the daemon. It does not manage the
/// concurrency semaphore or the `processing_files` set — callers handle those.
pub async fn process_one(deps: ProcessorDeps, job: ConversionJob) {
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
                &deps,
                &deps.ffmpeg_path,
                &deps.ffprobe_path,
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
                &deps,
                &deps.ffmpeg_path,
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
                &deps,
                &deps.ffmpeg_path,
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
                &deps,
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
                &deps,
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
                &deps,
                action,
            )
            .await;
        }
    }
}
