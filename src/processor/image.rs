//! Image processor — uses ffmpeg subprocess for format conversion.
//!
//! Migrated from the `image` crate (`spawn_blocking`) to a consistent
//! `tokio::process::Command` approach, matching all other processors.
//! This makes the 3600s timeout in `run_conversion` correctly cancel
//! an orphaned ffmpeg process via `.kill_on_drop(true)`.
//!
//! Conversion strategy:
//! - Reads input image with `-i <input>`
//! - Applies optional pixel-format filter (`format=rgba` or `format=rgb24`)
//!   for transparency control
//! - Applies optional JPEG quality (`-qscale:v`) mapped from user's 1-100
//!   scale to ffmpeg's inverse 1-31 scale
//! - Writes output (ffmpeg auto-detects format from extension)

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::{error, warn};
use tokio::process::Command;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::ImageRule;
use crate::health::server::HealthServer;
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;

/// Map user quality (1-100, higher=better) to ffmpeg inverse JPEG scale (1-31, lower=better).
fn jpeg_quality_scale(quality: u32) -> u32 {
    let q = ((100u32.saturating_sub(quality)) * 31).saturating_div(100);
    q.clamp(1, 31)
}

#[allow(clippy::too_many_arguments)]
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
    ffmpeg_path: &str,
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
        Err(e) => {
            warn!(
                "generate_path failed for {}: {}; falling back",
                file_name, e
            );
            match OutputNamer::generate_with_counter(&output_folder_path, &base_name, "image", ext)
            {
                Ok(p) => p,
                Err(e2) => {
                    error!(
                        "Final output path generation failed for {}: {}; aborting",
                        file_name, e2
                    );
                    return;
                }
            }
        }
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
            convert_image(&input_path, &output_path_clone, &rule_clone, ffmpeg_path).await?;
            Ok(output_path_clone.to_string_lossy().to_string())
        },
    )
    .await;
}

async fn convert_image(
    input: &Path,
    output: &Path,
    rule: &ImageRule,
    ffmpeg_path: &str,
) -> Result<()> {
    let args = build_image_args(input, output, rule);

    let mut cmd = Command::new(ffmpeg_path);
    cmd.kill_on_drop(true);

    for arg in &args {
        cmd.arg(arg);
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let output_result = cmd.output().await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("FFmpeg image conversion failed: {}", stderr.trim());
    }

    Ok(())
}

/// Build the ffmpeg argument list for an image-to-image conversion.
///
/// Command structure:
///   ffmpeg -y [pixel-filter] -i <input> [-qscale:v N] <output>
fn build_image_args(input: &Path, output: &Path, rule: &ImageRule) -> Vec<String> {
    let mut args = Vec::new();

    // Overwrite if exists (the file was already created by runner's create_new or
    // doesn't exist — -y handles both cases safely).
    args.push("-y".to_string());

    // Transparency control via pixel format filter.
    match rule.transparent {
        Some(true) => {
            args.push("-vf".to_string());
            args.push("format=rgba".to_string());
        }
        Some(false) => {
            args.push("-vf".to_string());
            args.push("format=rgb24".to_string());
        }
        None => {} // let ffmpeg auto-negotiate pixel format
    }

    args.push("-i".to_string());
    args.push(input.to_string_lossy().to_string());

    // JPEG quality mapping (only meaningful for JPEG output).
    let ext = rule.output_ext.as_deref().unwrap_or(".png");
    let normalized_ext = ext.trim_start_matches('.').to_lowercase();
    match normalized_ext.as_str() {
        "jpg" | "jpeg" => {
            let q = jpeg_quality_scale(rule.quality.unwrap_or(90));
            args.push("-qscale:v".to_string());
            args.push(q.to_string());
        }
        // PNG, GIF, BMP, TIFF, WebP: quality is not meaningfully controllable
        // via a single qscale for still images — ffmpeg uses encoder defaults.
        _ => {}
    }

    args.push(output.to_string_lossy().to_string());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jpeg_quality_scale() {
        assert_eq!(jpeg_quality_scale(100), 1); // best quality → lowest qscale
        assert_eq!(jpeg_quality_scale(90), 3);
        assert_eq!(jpeg_quality_scale(50), 15);
        assert_eq!(jpeg_quality_scale(10), 27);
        assert_eq!(jpeg_quality_scale(0), 31); // worst quality → highest qscale
    }

    #[test]
    fn test_build_image_args_default() {
        let input = Path::new("/tmp/input.png");
        let output = Path::new("/tmp/output_conv.png");
        let rule = ImageRule {
            preset: "png".to_string(),
            subfolder: None,
            input_extensions: vec![".png".into()],
            output_ext: Some(".png".to_string()),
            quality: None,
            transparent: None,
            output_name: None,
        };
        let args = build_image_args(input, output, &rule);
        assert_eq!(
            args,
            vec!["-y", "-i", "/tmp/input.png", "/tmp/output_conv.png"]
        );
    }

    #[test]
    fn test_build_image_args_jpeg_with_quality() {
        let input = Path::new("/tmp/input.tiff");
        let output = Path::new("/tmp/output.jpg");
        let rule = ImageRule {
            preset: "jpeg_80".to_string(),
            subfolder: None,
            input_extensions: vec![".tiff".into()],
            output_ext: Some(".jpg".to_string()),
            quality: Some(80),
            transparent: None,
            output_name: None,
        };
        let args = build_image_args(input, output, &rule);
        // qscale for quality=80→ (100-80)*31/100 = 6
        assert_eq!(
            args,
            vec![
                "-y",
                "-i",
                "/tmp/input.tiff",
                "-qscale:v",
                "6",
                "/tmp/output.jpg"
            ]
        );
    }

    #[test]
    fn test_build_image_args_transparent_rgba() {
        let input = Path::new("/tmp/input.png");
        let output = Path::new("/tmp/output.png");
        let rule = ImageRule {
            preset: "png".to_string(),
            subfolder: None,
            input_extensions: vec![".png".into()],
            output_ext: Some(".png".to_string()),
            quality: None,
            transparent: Some(true),
            output_name: None,
        };
        let args = build_image_args(input, output, &rule);
        assert_eq!(
            args,
            vec![
                "-y",
                "-vf",
                "format=rgba",
                "-i",
                "/tmp/input.png",
                "/tmp/output.png"
            ]
        );
    }

    #[test]
    fn test_build_image_args_non_transparent_rgb() {
        let input = Path::new("/tmp/input.webp");
        let output = Path::new("/tmp/output.png");
        let rule = ImageRule {
            preset: "png".to_string(),
            subfolder: None,
            input_extensions: vec![".webp".into()],
            output_ext: Some(".png".to_string()),
            quality: None,
            transparent: Some(false),
            output_name: None,
        };
        let args = build_image_args(input, output, &rule);
        assert_eq!(
            args,
            vec![
                "-y",
                "-vf",
                "format=rgb24",
                "-i",
                "/tmp/input.webp",
                "/tmp/output.png"
            ]
        );
    }

    #[test]
    fn test_build_image_args_jpeg_with_transparency_and_quality() {
        let input = Path::new("/tmp/input.bmp");
        let output = Path::new("/tmp/output.jpeg");
        let rule = ImageRule {
            preset: "jpeg_75".to_string(),
            subfolder: None,
            input_extensions: vec![".bmp".into()],
            output_ext: Some(".jpeg".to_string()),
            quality: Some(75),
            transparent: Some(false),
            output_name: None,
        };
        let args = build_image_args(input, output, &rule);
        // qscale for quality=75 → (100-75)*31/100 = 7
        assert_eq!(
            args,
            vec![
                "-y",
                "-vf",
                "format=rgb24",
                "-i",
                "/tmp/input.bmp",
                "-qscale:v",
                "7",
                "/tmp/output.jpeg"
            ]
        );
    }

    #[test]
    fn test_build_image_args_default_output_ext() {
        let input = Path::new("/tmp/input.gif");
        let output = Path::new("/tmp/output_conv.png");
        let rule = ImageRule {
            preset: "png".to_string(),
            subfolder: None,
            input_extensions: vec![".gif".into()],
            output_ext: None, // defaults to .png
            quality: None,
            transparent: None,
            output_name: None,
        };
        let args = build_image_args(input, output, &rule);
        assert_eq!(
            args,
            vec!["-y", "-i", "/tmp/input.gif", "/tmp/output_conv.png"]
        );
    }

    /// Integration test: if ffmpeg is available on the system, perform a real
    /// image conversion (1x1 PNG → JPEG). Skips gracefully if no ffmpeg found.
    #[tokio::test]
    async fn test_convert_image_integration() {
        // Locate ffmpeg binary.
        let ffmpeg_path = match std::env::var("FFMPEG_PATH") {
            Ok(p) if std::path::Path::new(&p).exists() => p,
            _ => {
                let candidates = ["/opt/homebrew/bin/ffmpeg", "/usr/bin/ffmpeg"];
                match candidates.iter().find(|p| std::path::Path::new(p).exists()) {
                    Some(p) => p.to_string(),
                    None => {
                        eprintln!("ffmpeg not found — skipping image conversion integration test");
                        return;
                    }
                }
            }
        };

        let tmp = std::env::temp_dir().join(format!("cw-img-int-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);

        // Create a 1x1 white PNG using ffmpeg itself.
        let input_png = tmp.join("input.png");
        let create_input = Command::new(&ffmpeg_path)
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=white:s=1x1",
                "-frames:v",
                "1",
                &input_png.to_string_lossy(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;

        match create_input {
            Ok(ref o) if o.status.success() => {}
            _ => {
                eprintln!("ffmpeg unavailable — skipping image conversion integration test");
                let _ = std::fs::remove_dir_all(&tmp);
                return;
            }
        }

        // Now convert the PNG to JPEG using our processor function.
        let rule = ImageRule {
            preset: "jpeg_85".to_string(),
            subfolder: None,
            input_extensions: vec![".png".into()],
            output_ext: Some(".jpg".to_string()),
            quality: Some(85),
            transparent: None,
            output_name: None,
        };

        let output_jpg = tmp.join("output.jpg");
        convert_image(&input_png, &output_jpg, &rule, &ffmpeg_path)
            .await
            .expect("conversion should succeed");

        assert!(output_jpg.exists(), "JPEG output file should exist");
        let meta = std::fs::metadata(&output_jpg).expect("should have metadata");
        assert!(meta.len() > 0, "JPEG output should have non-zero size");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
