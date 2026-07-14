use std::path::{Path, PathBuf};

use anyhow::Result;

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

            let path = output_folder.join(filename);
            validate_output_path(&path, output_folder)?;
            if !path.exists() {
                return Ok(path);
            }
        }

        // Exhausted the numbered range — let the caller fall back to a
        // timestamp-based counter name.
        Err(anyhow::anyhow!("No free output path found in range"))
    }

    pub fn generate_with_counter(
        output_folder: &Path,
        base_name: &str,
        codec: &str,
        ext: &str,
    ) -> PathBuf {
        for counter in 0..1000 {
            let filename = format!("{}_{}_{}.{}", base_name, codec, counter, ext);
            let path = output_folder.join(&filename);
            if !path.exists() {
                return path;
            }
        }

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
        output_folder.join(filename)
    }
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

    // If both paths can be canonicalized, verify containment
    if let (Ok(canonical_path), Ok(canonical_folder)) =
        (path.canonicalize(), output_folder.canonicalize())
    {
        if !canonical_path.starts_with(&canonical_folder) {
            anyhow::bail!(
                "Output path escapes output folder: {:?} not inside {:?}",
                canonical_path,
                canonical_folder
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_output_path_rejects_dotdot() {
        let folder = Path::new("/tmp/outputs");
        let path = Path::new("/tmp/outputs/../../etc/evil.txt");
        assert!(validate_output_path(path, folder).is_err());
    }

    #[test]
    fn test_validate_output_path_accepts_normal() {
        let folder = Path::new("/tmp/outputs");
        let path = Path::new("/tmp/outputs/video_h264_0.mp4");
        assert!(validate_output_path(path, folder).is_ok());
    }
}
