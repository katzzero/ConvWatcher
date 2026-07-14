use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use image::{DynamicImage, ImageFormat};
use image::codecs::jpeg::JpegEncoder;
use log::warn;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::ImageRule;
use crate::health::server::HealthServer;
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;

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

    let output_folder_path = PathBuf::from(output_folder);
    let base_name = get_base_name(&file_name);
    let ext = rule.output_ext.as_deref().unwrap_or(".png").trim_start_matches('.');
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
