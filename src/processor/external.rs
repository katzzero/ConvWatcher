use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::warn;
use tokio::process::Command;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::CustomRule;
use crate::health::server::HealthServer;
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;

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
        rule.output_name.as_deref().unwrap_or("{base}_custom.{ext}"),
        "custom",
        ext,
    ) {
        Ok(p) => p,
        Err(_) => {
            OutputNamer::generate_with_counter(&output_folder_path, &base_name, "custom", ext)
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

/// Split template into argv tokens, then replace placeholders in each token
/// individually. This preserves paths containing whitespace as single argv
/// elements.
fn build_argv(
    template: &str,
    input: &str,
    output: &str,
    basename: &str,
    ext: &str,
    output_folder: &str,
) -> Result<Vec<String>> {
    let tokens: Vec<&str> = template.split_whitespace().collect();
    let mut argv = Vec::with_capacity(tokens.len());

    for token in tokens {
        let replaced = token
            .replace("{input}", input)
            .replace("{output}", output)
            .replace("{basename}", basename)
            .replace("{ext}", ext)
            .replace("{output_folder}", output_folder);

        if replaced.contains("..") {
            bail!("Expanded token '{}' contains '..' path traversal", replaced);
        }

        argv.push(replaced);
    }

    Ok(argv)
}

fn validate_command_template(template: &str) -> Result<()> {
    if template.contains("..") {
        bail!("Template contains '..' path traversal");
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
    fn test_build_argv_rejects_traversal() {
        let result = build_argv(
            "ffmpeg -i {input} -filter {basename}",
            "/in/video.mp4",
            "",
            "../../etc",
            "",
            "/out/",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_template_rejects_dotdot() {
        assert!(validate_command_template("ffmpeg -i ../secret").is_err());
        assert!(validate_command_template("ffmpeg -i {input}").is_ok());
    }
}
