use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use anyhow::Result;
use log::warn;

pub struct OutputNamer;

impl OutputNamer {
    /// Build an output path from the template, substituting `{base}`, `{codec}`,
    /// `{ext}` and an auto-incrementing `{num}` so that the result never
    /// collides with an existing file. Returns the first free path.
    pub fn generate_path(
        output_folder: &Path,
        base_name: &str,
        template: &str,
        codec: &str,
        ext: &str,
    ) -> Result<PathBuf> {
        for num in 0..1000 {
            let filename = template
                .replace("{base}", base_name)
                .replace("{codec}", codec)
                .replace("{num}", &num.to_string())
                .replace("{ext}", ext);

            validate_filename(&filename)?;
            let path = output_folder.join(&filename);
            validate_output_path(&path, output_folder)?;

            // Atomically claim the slot to avoid TOCTOU between check and write.
            // Creates an empty file that the processor will overwrite/truncate.
            // Caller cleanup (run_conversion) handles removal on failure.
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(path),
                Err(ref e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    warn!("Could not claim output path {:?}: {}", path, e);
                    continue;
                }
            }
        }

        // Exhausted the numbered range — let the caller fall back to a
        // timestamp-based counter name.
        Err(anyhow::anyhow!(
            "No free output path found in range for output_folder={:?} template={:?}",
            output_folder,
            template
        ))
    }

    pub fn generate_with_counter(
        output_folder: &Path,
        base_name: &str,
        codec: &str,
        ext: &str,
    ) -> Result<PathBuf> {
        for counter in 0..1000 {
            let filename = format!("{}_{}_{}.{}", base_name, codec, counter, ext);
            validate_filename(&filename)?;
            let path = output_folder.join(&filename);

            // Atomically claim the slot to avoid TOCTOU.
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Ok(path),
                Err(ref e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    warn!("Could not claim counter path {:?}: {}", path, e);
                    continue;
                }
            }
        }

        // Exhausted — fall back to timestamp-based name.
        let filename = format!(
            "{}_{}_{}.{}",
            base_name,
            codec,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            ext
        );
        let path = output_folder.join(&filename);
        validate_filename(&filename)?;
        Ok(path)
    }
}

/// Validate that a filename (before joining) does not contain path separators,
/// absolute paths, or `..` components that could escape the output folder.
fn validate_filename(filename: &str) -> Result<()> {
    let p = Path::new(filename);
    if p.has_root() {
        anyhow::bail!("Output filename is absolute: '{filename}' (would escape output folder)");
    }
    for component in p.components() {
        match component {
            std::path::Component::ParentDir => {
                anyhow::bail!("Output filename contains '..' traversal: '{filename}'");
            }
            std::path::Component::Prefix(_) => {
                anyhow::bail!("Output filename contains Windows prefix: '{filename}'");
            }
            _ => {}
        }
    }
    Ok(())
}

/// Verify that `path` resolves to a location inside `output_folder`.
/// Prevents path traversal via `..` or absolute paths in base_name/template.
fn validate_output_path(path: &Path, output_folder: &Path) -> Result<()> {
    // Check for ".." in any path component
    for component in path.components() {
        if let std::path::Component::ParentDir = component {
            anyhow::bail!("Output path contains '..' traversal component");
        }
    }

    // Canonicalize the output folder; fail fast if we can't.
    let canonical_folder = output_folder.canonicalize().map_err(|e| {
        anyhow::anyhow!(
            "Failed to canonicalize output folder {:?}: {}",
            output_folder,
            e
        )
    })?;

    // Use canonicalized output_folder + parent to verify containment
    // without requiring the output file to exist.
    if let Some(parent) = path.parent() {
        if let Ok(canonical_parent) = parent.canonicalize() {
            if !canonical_parent.starts_with(&canonical_folder) {
                anyhow::bail!(
                    "Output path escapes output folder: {:?} not inside {:?}",
                    canonical_parent,
                    canonical_folder
                );
            }
        } else {
            let resolved = canonical_folder.join(path.file_name().unwrap_or_default());
            if !resolved.starts_with(&canonical_folder) {
                anyhow::bail!(
                    "Output path escapes output folder: {:?} resolves outside {:?}",
                    path,
                    canonical_folder
                );
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_output_path_rejects_dotdot() {
        let dir = std::env::temp_dir().join("convwatcher_dotdot_test");
        std::fs::create_dir_all(&dir).unwrap();
        let folder = &dir;
        let path = Path::new("/tmp/outputs/../../etc/evil.txt");
        assert!(validate_output_path(path, folder).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_validate_output_path_accepts_normal() {
        let dir = std::env::temp_dir().join("convwatcher_namer_test_ok");
        std::fs::create_dir_all(&dir).unwrap();
        let folder = &dir;
        let path = folder.join("video_h264_0.mp4");
        assert!(validate_output_path(&path, folder).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_generate_path_rejects_absolute_template() {
        let dir = std::env::temp_dir().join("convwatcher_namer_test_abs");
        std::fs::create_dir_all(&dir).unwrap();
        let result = OutputNamer::generate_path(&dir, "base", "/etc/owned.mp4", "h264", "mp4");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("absolute") || err.contains("traversal"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_generate_with_counter_returns_result() {
        let dir = std::env::temp_dir().join("convwatcher_namer_test_wc");
        std::fs::create_dir_all(&dir).unwrap();
        let result = OutputNamer::generate_with_counter(&dir, "base", "video", "mp4");
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.starts_with(&dir));
        // The atomically-created file should exist.
        assert!(path.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_generate_path_exhausted_error_is_contextual() {
        // Pre-seed the exact names that generate_path will try (0..1000).
        let dir = std::env::temp_dir().join("convwatcher_namer_test_exh");
        std::fs::create_dir_all(&dir).unwrap();
        for num in 0..1000 {
            let filename = format!("video_{}.mp4", num);
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join(&filename))
                .ok();
        }
        let result = OutputNamer::generate_path(&dir, "base", "video_{num}.{ext}", "video", "mp4");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // The error should mention the output_folder and template.
        assert!(err.contains("output_folder"), "error msg: {}", err);
        assert!(err.contains("template"), "error msg: {}", err);
        std::fs::remove_dir_all(&dir).ok();
    }
}
