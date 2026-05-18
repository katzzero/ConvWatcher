use std::path::PathBuf;

use crate::config::watch::WatchType;

pub struct ConversionJob {
    pub watcher_name: String,
    pub file_name: String,
    pub file_path: PathBuf,
    pub watch_type: WatchType,
    pub output_folder: String,
    pub watch_folder: String,
}
