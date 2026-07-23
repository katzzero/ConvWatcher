use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat};
use log::warn;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::ImageRule;
use crate::health::server::HealthServer;
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;

/// Maximum input file size (in bytes) for image conversions.
///
/// Images above this threshold are rejected synchronously before
/// entering `spawn_blocking`, preventing blocking-pool exhaustion
/// from very large files (e.g. 1 GB TIFFs).
///
/// **Cancellation caveat:** Unlike every other processor (which uses
/// `tokio::process::Command` with `.kill_on_drop(true)`), the image
/// processor uses `tokio::task::spawn_blocking` because the `image`
/// crate's open/save are synchronous. A `tokio::time::timeout` in
/// `run_conversion` drops the outer future, but **does not abort** the
/// blocking task — it runs to completion. If the timeout fires, the
/// still-running task may write output after `cleanup_partial_output`
/// has already cleared it, leaving a stale file on disk. This is an
/// accepted limitation for image conversions given their typical speed;
/// see `docs/review/01-processor.md §H2` for the discussion.
const MAX_IMAGE_INPUT_BYTES: u64 = 100 * 1024 * 1024; // 100 MB

pub async fn process_image(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    rule: &ImageRule,
    output_folder: &str,
    watch_folder: &str,
    error_logger: Arc<ErrorLogger>,
    health_server: Arc<HealthServer>,
    disk_config: &DiskSpaceConfig,
    input_file_action: crate::config::global::InputFileAction,
) {
    if check_disk_space(output_folder, watch_folder, disk_config).await {
        warn!(
            "Disk space low — pausing conversion of {} until space is freed",
            file_name
        );
        return;
    }

    // Reject inputs that are too large for the blocking-pool path.
    if let Ok(meta) = std::fs::metadata(&file_path) {
        if meta.len() > MAX_IMAGE_INPUT_BYTES {
            let msg = format!(
                "Image input exceeds {} byte limit ({} bytes): {}",
                MAX_IMAGE_INPUT_BYTES,
                meta.len(),
                file_name
            );
            warn!("{}", msg);
            error_logger.log(&msg, &file_name, "image");
            return;
        }
    }

    let output_folder_path = PathBuf::from(output_folder);
    let base_name = get_base_name(&file_name);
    let ext = rule
        .output_ext
        .as_deref()
        .unwrap_or(".png")
        .trim_start_matches('.');
    let output_path = match OutputNamer::generate_path(
        &output_folder_path,
        &base_name,
        rule.output_name.as_deref().unwrap_or("{base}_conv.{ext}"),
        "image",
        ext,
    ) {
        Ok(p) => p,
        Err(_) => OutputNamer::generate_with_counter(&output_folder_path, &base_name, "image", ext),
    };

    let input_path = file_path.clone();
    let output_path_clone = output_path.clone();
    let rule_clone = rule.clone();
    super::runner::run_conversion(
        watcher_name,
        file_name,
        file_path,
        &output_path,
        error_logger,
        health_server,
        input_file_action,
        "image",
        || async move {
            tokio::task::spawn_blocking(move || {
                convert_image(&input_path, &output_path_clone, &rule_clone)?;
                Ok(output_path_clone.to_string_lossy().to_string())
            })
            .await
            .map_err(|e| anyhow::anyhow!("Image conversion task panicked: {}", e))?
        },
    )
    .await;
}

fn convert_image(input: &Path, output: &Path, rule: &ImageRule) -> Result<()> {
    let mut img = image::open(input)?;

    if rule.transparent.unwrap_or(false) {
        img = DynamicImage::ImageRgba8(img.to_rgba8());
    } else {
        img = DynamicImage::ImageRgb8(img.to_rgb8());
    }

    let format = detect_format(&rule.output_ext.as_deref().unwrap_or(".png"));
    save_image(&img, output, format, rule.quality.unwrap_or(90))?;

    Ok(())
}

fn detect_format(ext: &str) -> ImageFormat {
    match ext.trim_start_matches('.').to_lowercase().as_str() {
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "png" => ImageFormat::Png,
        "gif" => ImageFormat::Gif,
        "bmp" => ImageFormat::Bmp,
        "tiff" | "tif" => ImageFormat::Tiff,
        "webp" => ImageFormat::WebP,
        "qoi" => ImageFormat::Qoi,
        _ => ImageFormat::Png,
    }
}

fn save_image(img: &DynamicImage, path: &Path, format: ImageFormat, quality: u32) -> Result<()> {
    match format {
        ImageFormat::Jpeg => {
            let file = std::fs::File::create(path)?;
            let mut buf = BufWriter::new(file);
            let encoder = JpegEncoder::new_with_quality(&mut buf, quality.min(100).max(1) as u8);
            img.write_with_encoder(encoder)?;
        }
        ImageFormat::Png => {
            img.save_with_format(path, ImageFormat::Png)?;
        }
        _ => {
            img.save(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::server::HealthServer;
    use crate::logs::error_logger::ErrorLogger;

    #[tokio::test]
    async fn test_image_size_precheck_rejects_oversize() {
        // Verify the MAX_IMAGE_INPUT_BYTES pre-check added for 01 §H2.
        let tmp = std::env::temp_dir().join(format!("cw-img-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let oversized = tmp.join("huge.tiff");

        // Create a sparse file larger than MAX_IMAGE_INPUT_BYTES.
        {
            let f = std::fs::File::create(&oversized).unwrap();
            f.set_len(101 * 1024 * 1024).unwrap();
        }

        let out_dir = tmp.join("out");
        let _ = std::fs::create_dir_all(&out_dir);

        let health = Arc::new(HealthServer::new(0, "127.0.0.1".to_string(), 100));
        let err_log_path = tmp.join("errors.log");
        let global_cfg = crate::config::global::GlobalConfig {
            log: crate::config::global::LogConfig {
                errors_file: err_log_path.to_string_lossy().to_string(),
                max_error_log_size_mb: 50,
                ..Default::default()
            },
            ..Default::default()
        };
        let error_logger = Arc::new(ErrorLogger::new(&global_cfg).unwrap());

        let rule = crate::config::watch::ImageRule {
            preset: "jpeg_80".to_string(),
            subfolder: None,
            input_extensions: vec![".tiff".into()],
            output_ext: Some(".jpg".to_string()),
            quality: None,
            transparent: None,
            output_name: None,
        };

        process_image(
            "test".to_string(),
            "huge.tiff".to_string(),
            oversized.clone(),
            &rule,
            &out_dir.to_string_lossy(),
            &tmp.to_string_lossy(),
            error_logger,
            health,
            &crate::config::global::DiskSpaceConfig::default(),
            crate::config::global::InputFileAction::Mark,
        )
        .await;

        // The pre-check should have bailed before creating any output.
        let has_output = std::fs::read_dir(&out_dir)
            .ok()
            .map(|e| e.filter_map(|e| e.ok()).count())
            .unwrap_or(0);
        assert_eq!(
            has_output, 0,
            "oversized input should not produce output (01 §H2 pre-check)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_image_size_precheck_allows_small() {
        // Verify a small input passes the pre-check.
        let tmp = std::env::temp_dir().join(format!("cw-img-small-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);

        // Generate a valid 1x1 white PNG using the image crate.
        let input = tmp.join("input.png");
        let img = DynamicImage::new_rgb8(1, 1);
        img.save(&input).unwrap();

        let out_dir = tmp.join("out");
        let _ = std::fs::create_dir_all(&out_dir);

        let health = Arc::new(HealthServer::new(0, "127.0.0.1".to_string(), 100));
        let err_log_path = tmp.join("errors.log");
        let global_cfg = crate::config::global::GlobalConfig {
            log: crate::config::global::LogConfig {
                errors_file: err_log_path.to_string_lossy().to_string(),
                max_error_log_size_mb: 50,
                ..Default::default()
            },
            ..Default::default()
        };
        let error_logger = Arc::new(ErrorLogger::new(&global_cfg).unwrap());

        let rule = crate::config::watch::ImageRule {
            preset: "png".to_string(),
            subfolder: None,
            input_extensions: vec![".png".into()],
            output_ext: Some(".png".to_string()),
            quality: None,
            transparent: None,
            output_name: None,
        };

        process_image(
            "test".to_string(),
            "input.png".to_string(),
            input.clone(),
            &rule,
            &out_dir.to_string_lossy(),
            &tmp.to_string_lossy(),
            error_logger,
            health,
            &crate::config::global::DiskSpaceConfig::default(),
            crate::config::global::InputFileAction::Mark,
        )
        .await;

        // A small file should be processed and produce output.
        let has_output = std::fs::read_dir(&out_dir)
            .ok()
            .map(|e| e.filter_map(|e| e.ok()).any(|e| e.path().extension().is_some_and(|ext| ext == "png")))
            .unwrap_or(false);
        assert!(has_output, "small PNG should be processed successfully");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
