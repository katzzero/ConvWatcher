//! ffmpeg argument construction shared by the coordinator's local processing
//! and the remote agent.
//!
//! The functions here build the *argument vector* (not a `Command`) so the same
//! logic can drive a file-in/file-out invocation (`temp` mode) or a
//! stdin/stdout pipe invocation (`pipe` mode). The argument ordering mirrors the
//! original `convert_video`/`convert_audio` implementations exactly so behaviour
//! is unchanged.

use crate::config::{pipe_output_format, WorkerIoMode};
use crate::protocol::{WireAudioRule, WireVideoRule};

/// Placeholder used in the returned args where the caller must substitute the
/// real input path. In `pipe` mode the input token is `pipe:0` and needs no
/// substitution.
pub const INPUT_TOKEN: &str = "{INPUT}";
/// Placeholder for the output path in `temp` mode.
pub const OUTPUT_TOKEN: &str = "{OUTPUT}";

/// Build the full ffmpeg argument list for a video conversion.
///
/// In `Temp` mode the returned vector contains [`INPUT_TOKEN`]/[`OUTPUT_TOKEN`]
/// placeholders the caller replaces with real paths. In `Pipe` mode the input
/// is `pipe:0`, the output is `-f <fmt> pipe:1`, and there are no placeholders.
pub fn build_video_args(
    rule: &WireVideoRule,
    output_ext: &str,
    io_mode: WorkerIoMode,
) -> Vec<String> {
    let quality = rule.quality.as_deref().unwrap_or("crf 23");
    let quality_args = parse_quality_value(quality);
    let codec = rule.codec.as_deref().unwrap_or("libx264");
    let (hwaccel_pre, hwaccel_post) = build_hwaccel_args(codec);

    let mut args: Vec<String> = Vec::new();
    args.push("-y".to_string());
    args.extend(hwaccel_pre);

    match io_mode {
        WorkerIoMode::Temp => {
            args.push("-i".to_string());
            args.push(INPUT_TOKEN.to_string());
        }
        WorkerIoMode::Pipe => {
            args.push("-i".to_string());
            args.push("pipe:0".to_string());
        }
    }

    args.extend(hwaccel_post);

    args.push("-c:v".to_string());
    args.push(codec.to_string());
    args.extend(quality_args);

    let audio_codec = rule.audio_codec.as_deref().unwrap_or("aac");
    args.push("-c:a".to_string());
    args.push(audio_codec.to_string());
    if audio_codec != "copy" {
        args.push("-b:a".to_string());
        args.push(rule.audio_bitrate.as_deref().unwrap_or("128k").to_string());
    }

    push_output(&mut args, output_ext, io_mode);
    args
}

/// Build the full ffmpeg argument list for an audio conversion.
pub fn build_audio_args(
    rule: &WireAudioRule,
    output_ext: &str,
    io_mode: WorkerIoMode,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    args.push("-y".to_string());

    match io_mode {
        WorkerIoMode::Temp => {
            args.push("-i".to_string());
            args.push(INPUT_TOKEN.to_string());
        }
        WorkerIoMode::Pipe => {
            args.push("-i".to_string());
            args.push("pipe:0".to_string());
        }
    }

    args.push("-vn".to_string());

    let audio_codec = rule.audio_codec.as_deref().unwrap_or("libmp3lame");
    args.push("-c:a".to_string());
    args.push(audio_codec.to_string());

    if audio_codec != "copy" {
        if let Some(ref bitrate) = rule.audio_bitrate {
            args.push("-b:a".to_string());
            args.push(bitrate.clone());
        }
    }

    if let Some(sr) = rule.sample_rate {
        args.push("-ar".to_string());
        args.push(sr.to_string());
    }

    if let Some(ch) = rule.channels {
        args.push("-ac".to_string());
        args.push(ch.to_string());
    }

    push_output(&mut args, output_ext, io_mode);
    args
}

fn push_output(args: &mut Vec<String>, output_ext: &str, io_mode: WorkerIoMode) {
    match io_mode {
        WorkerIoMode::Temp => {
            args.push(OUTPUT_TOKEN.to_string());
        }
        WorkerIoMode::Pipe => {
            args.push("-f".to_string());
            args.push(pipe_output_format(output_ext).to_string());
            args.push("pipe:1".to_string());
        }
    }
}

/// Parse a quality string (e.g. "crf 23", "cq 20", "3000k") into ffmpeg args.
/// Ported verbatim from the original video processor.
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
                vec!["-crf".to_string(), "23".to_string()]
            }
        }
    }
}

/// Build hardware acceleration arguments for the given codec.
/// Returns (pre_input_args, post_input_args). Ported verbatim from the original
/// video processor.
pub fn build_hwaccel_args(codec: &str) -> (Vec<String>, Vec<String>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_parsing_matches_original() {
        assert_eq!(parse_quality_value("crf 23"), vec!["-crf", "23"]);
        assert_eq!(parse_quality_value("cq 18"), vec!["-cq", "18"]);
        assert_eq!(parse_quality_value("qp 25"), vec!["-qp", "25"]);
        assert_eq!(parse_quality_value("qscale 4"), vec!["-qscale:v", "4"]);
        assert_eq!(parse_quality_value("23"), vec!["-crf", "23"]);
        assert_eq!(parse_quality_value("5M"), vec!["-b:v", "5M"]);
    }

    #[test]
    fn video_temp_args_have_placeholders() {
        let rule = WireVideoRule {
            codec: Some("libx264".into()),
            quality: Some("crf 20".into()),
            audio_codec: Some("aac".into()),
            audio_bitrate: Some("128k".into()),
            ..Default::default()
        };
        let args = build_video_args(&rule, "mp4", WorkerIoMode::Temp);
        assert!(args.contains(&INPUT_TOKEN.to_string()));
        assert!(args.contains(&OUTPUT_TOKEN.to_string()));
        // Ordering sanity: -y, -i {INPUT}, -c:v libx264, -crf 20, -c:a aac, -b:a 128k, {OUTPUT}
        assert_eq!(args[0], "-y");
        assert_eq!(args[1], "-i");
        assert_eq!(args[2], INPUT_TOKEN);
        let cv = args.iter().position(|a| a == "-c:v").unwrap();
        assert_eq!(args[cv + 1], "libx264");
    }

    #[test]
    fn video_pipe_args_use_pipes() {
        let rule = WireVideoRule {
            codec: Some("libx264".into()),
            ..Default::default()
        };
        let args = build_video_args(&rule, "mkv", WorkerIoMode::Pipe);
        assert!(args.contains(&"pipe:0".to_string()));
        assert!(args.contains(&"pipe:1".to_string()));
        assert!(!args.contains(&INPUT_TOKEN.to_string()));
        let f = args.iter().position(|a| a == "-f").unwrap();
        assert_eq!(args[f + 1], "matroska");
    }

    #[test]
    fn audio_args_copy_skips_bitrate() {
        let rule = WireAudioRule {
            audio_codec: Some("copy".into()),
            audio_bitrate: Some("320k".into()),
            ..Default::default()
        };
        let args = build_audio_args(&rule, "mka", WorkerIoMode::Pipe);
        assert!(!args.contains(&"-b:a".to_string()));
    }
}
