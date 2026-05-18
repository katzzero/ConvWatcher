use std::path::{Path, PathBuf};

use anyhow::Result;

pub struct OutputNamer;

impl OutputNamer {
    pub fn generate_path(
        output_folder: &Path,
        base_name: &str,
        template: &str,
        codec: &str,
        ext: &str,
    ) -> Result<PathBuf> {
        let filename = template
            .replace("{base}", base_name)
            .replace("{codec}", codec)
            .replace("{num}", "0")
            .replace("{ext}", ext);

        Ok(output_folder.join(filename))
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
