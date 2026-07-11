use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use image::{DynamicImage, ImageFormat};
use image::codecs::jpeg::JpegEncoder;
use log::{error, info, warn};

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::ImageRule;
use crate::health::server::{ConversionRecord, HealthServer};
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
) {
    check_disk_space(output_folder, watch_folder, disk_config).await;

    let _ = health_server.set_processing(watcher_name.clone(), file_name.clone());
    let _ = health_server.dequeue(&file_name);
    info!("[Processor] Processing started: {}", file_name);

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

    match convert_image(&file_path, &output_path, rule) {
        Ok(()) => {
            info!("Image conversion succeeded: {}", file_name);
            let _ = health_server.increment_processed(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "done".to_string(),
                output: output_path.to_string_lossy().to_string(),
            });
        }
        Err(e) => {
            let msg = format!("Image conversion failed: {}", e);
            error!("{}", msg);
            warn!("[Processor] Error discarded, continuing: {}", file_name);
            error_logger.log(&msg, &file_name, "image::process");
            let _ = health_server.increment_error(&watcher_name);
            let _ = health_server.add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "error".to_string(),
                output: String::new(),
            });
        }
    }

    info!("[Processor] Job finished: {}", file_name);
    let _ = health_server.clear_processing(&watcher_name);
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
