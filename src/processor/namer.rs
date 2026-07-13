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
