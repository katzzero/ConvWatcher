use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::warn;
use tokio::process::Command;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::VideoRule;
use crate::health::server::HealthServer;
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;

pub async fn process_video(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    rule: &VideoRule,
    output_folder: &str,
    watch_folder: &str,
    error_logger: Arc<ErrorLogger>,
    health_server: Arc<HealthServer>,
    disk_config: &DiskSpaceConfig,
    ffmpeg_path: &str,
    ffprobe_path: &str,
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
        .unwrap_or(".mp4")
        .trim_start_matches('.');
    let output_path = match OutputNamer::generate_path(
        &output_folder_path,
        &base_name,
        rule.output_name
            .as_deref()
            .unwrap_or("{base}_{codec}_{num}.{ext}"),
        rule.codec.as_deref().unwrap_or("libx264"),
        ext,
    ) {
        Ok(p) => p,
        Err(_) => OutputNamer::generate_with_counter(&output_folder_path, &base_name, "video", ext),
    };

    let input_path = file_path.clone();
    let output_path_for_cleanup = output_path.clone();
    super::runner::run_conversion(
        watcher_name,
        file_name,
        file_path,
        &output_path_for_cleanup,
        error_logger,
        health_server,
        input_file_action,
        "video",
        || async move {
            convert_video(&input_path, &output_path, rule, ffmpeg_path, ffprobe_path).await?;
            Ok(output_path.to_string_lossy().to_string())
        },
    )
    .await;
}

async fn convert_video(
    input: &Path,
    output: &Path,
    rule: &VideoRule,
    ffmpeg_path: &str,
    ffprobe_path: &str,
) -> Result<()> {
    let quality = rule.quality.as_deref().unwrap_or("crf 23");
    let quality_args = parse_quality_value(quality);
    let codec = rule.codec.as_deref().unwrap_or("libx264");

    let (hwaccel_pre, hwaccel_post) = build_hwaccel_args(codec);

    let mut cmd = Command::new(ffmpeg_path);
    cmd.kill_on_drop(true);
    cmd.arg("-y");

    for arg in &hwaccel_pre {
        cmd.arg(arg);
    }

    cmd.arg("-i").arg(input.as_os_str());

    for arg in &hwaccel_post {
        cmd.arg(arg);
    }

    cmd.arg("-c:v").arg(codec);

    for arg in &quality_args {
        cmd.arg(arg);
    }

    let audio_codec = rule.audio_codec.as_deref().unwrap_or("aac");
    cmd.arg("-c:a").arg(audio_codec);
    if audio_codec != "copy" {
        cmd.arg("-b:a")
            .arg(rule.audio_bitrate.as_deref().unwrap_or("128k"));
    }
    cmd.arg(output.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output_result = cmd.output().await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("FFmpeg failed: {}", stderr);
    }

    if rule.check_duration.unwrap_or(true) {
        let input_duration = get_video_duration(input, ffprobe_path).await.unwrap_or(0.0);
        let output_duration = get_video_duration(output, ffprobe_path)
            .await
            .unwrap_or(0.0);

        if input_duration > 0.0
            && output_duration < input_duration * rule.min_duration_ratio.unwrap_or(0.9)
        {
            bail!(
                "Duration mismatch: input={:.1}s output={:.1}s (min ratio: {:.2})",
                input_duration,
                output_duration,
                rule.min_duration_ratio.unwrap_or(0.9)
            );
        }
    }

    Ok(())
}

pub fn parse_quality_value(quality_str: &str) -> Vec<String> {
    let trimmed = quality_str.trim();

    if trimmed.is_empty() {
        return vec!["-crf".to_string(), "23".to_string()];
    }

    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let keyword = parts[0].to_lowercase();

    match keyword.as_str() {
        "crf" => {
            let value = parts.get(1).unwrap_or(&"23");
            vec!["-crf".to_string(), value.to_string()]
        }
        "cq" => {
            let value = parts.get(1).unwrap_or(&"23");
            vec!["-cq".to_string(), value.to_string()]
        }
        "qp" => {
            let value = parts.get(1).unwrap_or(&"25");
            vec!["-qp".to_string(), value.to_string()]
        }
        "qp_i" => {
            let value = parts.get(1).unwrap_or(&"25");
            vec!["-qp_i".to_string(), value.to_string()]
        }
        "qscale" => {
            let value = parts.get(1).unwrap_or(&"4");
            vec!["-qscale:v".to_string(), value.to_string()]
        }
        "constant_bit_rate" => {
            let value = parts.get(1).unwrap_or(&"3000");
            vec!["-b:v".to_string(), value.to_string()]
        }
        "vbr" => {
            let value = parts.get(1).unwrap_or(&"4");
            vec!["-q:v".to_string(), value.to_string()]
        }
        _ => {
            let first = parts[0];
            if first.ends_with('M')
                || first.ends_with('m')
                || first.ends_with('K')
                || first.ends_with('k')
            {
                vec!["-b:v".to_string(), first.to_string()]
            } else if first.parse::<u32>().is_ok() {
                vec!["-crf".to_string(), first.to_string()]
            } else {
                warn!(
                    "Unknown quality format '{}' — falling back to -crf 23",
                    quality_str
                );
                vec!["-crf".to_string(), "23".to_string()]
            }
        }
    }
}

/// Build hardware acceleration arguments for the given codec.
/// Returns (pre_input_args, post_input_args) to position around -i <input>.
fn build_hwaccel_args(codec: &str) -> (Vec<String>, Vec<String>) {
    if codec.contains("_vaapi") {
        (
            vec![
                "-vaapi_device".to_string(),
                "/dev/dri/renderD128".to_string(),
            ],
            vec!["-vf".to_string(), "format=nv12,hwupload".to_string()],
        )
    } else if codec.contains("_qsv") {
        (
            vec![
                "-init_hw_device".to_string(),
                "qsv=qsv".to_string(),
                "-hwaccel".to_string(),
                "qsv".to_string(),
                "-hwaccel_output_format".to_string(),
                "qsv".to_string(),
            ],
            vec![],
        )
    } else if codec.contains("_rkmpp") {
        (
            vec![
                "-init_hw_device".to_string(),
                "rkmpp=rkmpp_dev".to_string(),
                "-hwaccel".to_string(),
                "rkmpp".to_string(),
                "-hwaccel_output_format".to_string(),
                "drm_prime".to_string(),
                "-hwaccel_device".to_string(),
                "rkmpp_dev".to_string(),
            ],
            vec!["-vf".to_string(), "format=nv12,hwupload".to_string()],
        )
    } else {
        (vec![], vec![])
    }
}

pub async fn get_video_duration(path: &Path, ffprobe_path: &str) -> Result<f64> {
    let output = Command::new(ffprobe_path)
        .kill_on_drop(true)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let duration: f64 = stdout.trim().parse().unwrap_or(0.0);
    Ok(duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_crf() {
        assert_eq!(parse_quality_value("crf 23"), vec!["-crf", "23"]);
        assert_eq!(parse_quality_value("crf 18"), vec!["-crf", "18"]);
    }

    #[test]
    fn test_parse_cq_nvenc() {
        assert_eq!(parse_quality_value("cq 23"), vec!["-cq", "23"]);
        assert_eq!(parse_quality_value("cq 18"), vec!["-cq", "18"]);
    }

    #[test]
    fn test_parse_qp_vaapi() {
        assert_eq!(parse_quality_value("qp 25"), vec!["-qp", "25"]);
        assert_eq!(parse_quality_value("qp 28"), vec!["-qp", "28"]);
    }

    #[test]
    fn test_parse_qp_i_amf() {
        assert_eq!(parse_quality_value("qp_i 25"), vec!["-qp_i", "25"]);
        assert_eq!(parse_quality_value("qp_i 28"), vec!["-qp_i", "28"]);
    }

    #[test]
    fn test_parse_qscale() {
        assert_eq!(parse_quality_value("qscale 4"), vec!["-qscale:v", "4"]);
    }

    #[test]
    fn test_parse_constant_bit_rate() {
        assert_eq!(
            parse_quality_value("constant_bit_rate 3000"),
            vec!["-b:v", "3000"]
        );
        assert_eq!(
            parse_quality_value("constant_bit_rate 5000"),
            vec!["-b:v", "5000"]
        );
    }

    #[test]
    fn test_parse_vbr() {
        assert_eq!(parse_quality_value("vbr 4"), vec!["-q:v", "4"]);
    }

    #[test]
    fn test_parse_numeric_only() {
        assert_eq!(parse_quality_value("23"), vec!["-crf", "23"]);
        assert_eq!(parse_quality_value("28"), vec!["-crf", "28"]);
    }

    #[test]
    fn test_parse_bitrate() {
        assert_eq!(parse_quality_value("3000k"), vec!["-b:v", "3000k"]);
        assert_eq!(parse_quality_value("5M"), vec!["-b:v", "5M"]);
    }

    #[test]
    fn test_parse_empty() {
        assert_eq!(parse_quality_value(""), vec!["-crf", "23"]);
    }

    #[test]
    fn test_parse_unknown_fallback() {
        assert_eq!(parse_quality_value("foo 42"), vec!["-crf", "23"]);
    }

    #[test]
    fn test_hwaccel_vaapi() {
        let (pre, post) = build_hwaccel_args("h264_vaapi");
        assert!(pre.contains(&"-vaapi_device".to_string()));
        assert!(post.contains(&"-vf".to_string()));
    }

    #[test]
    fn test_hwaccel_qsv() {
        let (pre, post) = build_hwaccel_args("hevc_qsv");
        assert!(pre.contains(&"-init_hw_device".to_string()));
        assert!(post.is_empty());
    }

    #[test]
    fn test_hwaccel_rkmpp() {
        let (pre, post) = build_hwaccel_args("hevc_rkmpp");
        assert!(pre.contains(&"-init_hw_device".to_string()));
        assert!(pre.contains(&"-hwaccel".to_string()));
        assert!(post.contains(&"-vf".to_string()));
    }

    #[test]
    fn test_hwaccel_none() {
        let (pre, post) = build_hwaccel_args("libx264");
        assert!(pre.is_empty());
        assert!(post.is_empty());
    }

    /// Path to the local-only test asset (HEVC .mov). Returns `None` when the
    /// file is absent so the test can skip gracefully — the asset is never
    /// committed.
    fn test_mov() -> Option<std::path::PathBuf> {
        let p =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-sample/test.mov");
        if p.exists() {
            Some(p)
        } else {
            None
        }
    }

    /// Derive the sibling ffprobe path from the system ffmpeg. Mirrors the
    /// production fallback logic so the test validates the *correct* binary.
    fn sibling_ffprobe(ffmpeg: &str) -> String {
        std::path::Path::new(ffmpeg)
            .parent()
            .map(|p| p.join("ffprobe").to_string_lossy().to_string())
            .unwrap_or_else(|| "/usr/bin/ffprobe".to_string())
    }

    #[tokio::test]
    async fn test_ffprobe_duration_on_real_video() {
        // Regression guard for 01 §H1 / 02 §H5: ffprobe fallback pointing at
        // the ffmpeg binary silently returns 0.0 for every file.  This test
        // uses the *real* sibling ffprobe and asserts the duration is
        // non-zero and within tolerance.
        let mov = match test_mov() {
            Some(p) => p,
            None => {
                eprintln!("test.mov not found — skipping ffprobe_duration_on_real_video");
                return;
            }
        };

        // Locate ffmpeg (env override or default).
        let ffmpeg = match std::env::var("FFMPEG_PATH") {
            Ok(p) if std::path::Path::new(&p).exists() => p,
            _ => {
                let candidates = ["/opt/homebrew/bin/ffmpeg", "/usr/bin/ffmpeg"];
                candidates.iter().find(|p| std::path::Path::new(p).exists())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| {
                        eprintln!("ffmpeg not found — skipping ffprobe_duration_on_real_video");
                        String::new()
                    })
            }
        };
        if ffmpeg.is_empty() {
            return;
        }
        let ffprobe = sibling_ffprobe(&ffmpeg);

        // Quick probe: skip if ffprobe is unavailable.
        let probe = tokio::process::Command::new(&ffprobe)
            .arg("-version")
            .output()
            .await;
        match probe {
            Ok(ref o) if o.status.success() => {}
            _ => {
                eprintln!("ffprobe not available — skipping ffprobe_duration_on_real_video");
                return;
            }
        }

        // Probe HEVC decode support — skip gracefully if unavailable.
        let hevc_check = tokio::process::Command::new(&ffmpeg)
            .args(["-hide_banner", "-decoders"])
            .output()
            .await;
        let has_hevc = match hevc_check {
            Ok(ref o) if o.status.success() => {
                let out = String::from_utf8_lossy(&o.stdout);
                out.lines().any(|l| l.contains("hevc"))
            }
            _ => false,
        };
        if !has_hevc {
            eprintln!("ffmpeg lacks HEVC decoder — skipping ffprobe_duration_on_real_video");
            return;
        }

        let duration = get_video_duration(&mov, &ffprobe).await.unwrap_or(0.0);

        // A duration of 0.0 indicates the ffprobe-fallback-points-at-ffmpeg
        // bug from 02 §H5 — ffmpeg's stdout is empty so parse returns 0.0.
        assert!(
            (duration - 1.835).abs() < 0.05,
            "expected duration ~1.835s, got {duration}s — if 0.0, this is the 02 §H5 ffprobe bug"
        );
    }
}
