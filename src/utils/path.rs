use std::path::Path;

pub fn get_base_name(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string())
}

pub fn mark_done(path: &Path) {
    let done_path = path.with_file_name(format!("{}.done", path.file_name().unwrap_or_default().to_string_lossy()));
    if let Err(e) = std::fs::rename(path, &done_path) {
        log::warn!("Failed to mark file as done {:?}: {}", path, e);
    }
}

pub fn mark_error(path: &Path) {
    let error_path = path.with_file_name(format!("{}.error", path.file_name().unwrap_or_default().to_string_lossy()));
    if let Err(e) = std::fs::rename(path, &error_path) {
        log::warn!("Failed to mark file as error {:?}: {}", path, e);
    }
}
