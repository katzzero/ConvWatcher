use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::info;
use tokio::sync::{mpsc, Semaphore};

use crate::processor::context::ProcessorDeps;
use crate::processor::job::ConversionJob;
use crate::processor::process_one;

struct ProcessingGuard {
    files: Arc<std::sync::Mutex<HashSet<PathBuf>>>,
    path: PathBuf,
}

impl Drop for ProcessingGuard {
    fn drop(&mut self) {
        if let Ok(mut files) = self.files.lock() {
            files.remove(&self.path);
        }
    }
}

/// Process conversion jobs from the channel until the channel is closed, then
/// drain in-flight jobs.
pub async fn process_jobs(
    mut job_rx: mpsc::Receiver<ConversionJob>,
    deps: ProcessorDeps,
    semaphore: Arc<Semaphore>,
    processing_files: Arc<std::sync::Mutex<HashSet<PathBuf>>>,
) {
    let mut tasks: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    while let Some(job) = job_rx.recv().await {
        let d = deps.clone();
        let sem = semaphore.clone();
        let pf = processing_files.clone();
        let file_path = job.file_path.clone();

        tasks.spawn(async move {
            // Removes the path from processing_files on every exit path,
            // including panic or task abort.
            let _processing_guard = ProcessingGuard {
                files: pf,
                path: file_path,
            };
            let _permit = sem.acquire().await;
            process_one(d, job).await;
        });

        // Reap completed jobs so the set does not grow unbounded on a daemon
        // that runs for weeks.
        while tasks.try_join_next().is_some() {}
    }

    info!(
        "Job processor shutting down, draining {} in-flight jobs",
        tasks.len()
    );

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !tasks.is_empty() {
        match tokio::time::timeout_at(deadline, tasks.join_next()).await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                info!("Job drain timeout reached, aborting remaining jobs");
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
            }
        }
    }

    info!("Job processor shut down complete");
}
