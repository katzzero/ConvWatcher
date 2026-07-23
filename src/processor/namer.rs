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

            validate_filename(&filename)?;
            let path = output_folder.join(&filename);
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
            validate_filename(&filename).ok();
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

/// Validate that a filename (before joining) does not contain path separators,
/// absolute paths, or `..` components that could escape the output folder.
fn validate_filename(filename: &str) -> Result<()> {
    let p = Path::new(filename);
    if p.has_root() {
        anyhow::bail!(
            "Output filename is absolute: '{filename}' (would escape output folder)"
        );
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

    // Use canonicalized output_folder + parent to verify containment
    // without requiring the output file to exist.
    if let Ok(canonical_folder) = output_folder.canonicalize() {
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
        let path = folder.join("video_h264_0.mp4");
        assert!(validate_output_path(&path, folder).is_ok());
    }

    #[test]
    fn test_validate_output_path_rejects_absolute_filename() {
        assert!(validate_filename("/etc/owned.mp4").is_err());
        assert!(validate_filename("subdir/../outside.txt").is_err());
        assert!(validate_filename("normal.mp4").is_ok());
    }

    #[test]
    fn test_generate_path_rejects_absolute_template() {
        let folder = Path::new("/tmp/outputs");
        let result = OutputNamer::generate_path(folder, "base", "/etc/owned.mp4", "h264", "mp4");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("absolute") || err.contains("traversal"));
    }
}
