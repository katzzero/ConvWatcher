use std::path::Path;

use crate::config::global::InputFileAction;

pub fn get_base_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string())
}

pub fn handle_input_file(path: &Path, action: &InputFileAction, success: bool) {
    match action {
        InputFileAction::Mark => {
            if success {
                mark_done(path);
            } else {
                mark_error(path);
            }
        }
        InputFileAction::Delete => {
            if success {
                if let Err(e) = std::fs::remove_file(path) {
                    log::warn!("Failed to delete input file {:?}: {}", path, e);
                }
            } else {
                mark_error(path);
            }
        }
        InputFileAction::None => {}
    }
}

fn mark_done(path: &Path) {
    let done_path = path.with_file_name(format!(
        "{}.done",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    if let Err(e) = std::fs::rename(path, &done_path) {
        log::warn!("Failed to mark file as done {:?}: {}", path, e);
    }
}

fn mark_error(path: &Path) {
    let error_path = path.with_file_name(format!(
        "{}.error",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    if let Err(e) = std::fs::rename(path, &error_path) {
        log::warn!("Failed to mark file as error {:?}: {}", path, e);
    }
}
