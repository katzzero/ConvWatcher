use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{bail, Result};
use log::{error, info};
use tokio::process::Command;

use crate::config::global::DiskSpaceConfig;
use crate::config::watch::{PdfMode, PdfQuality, PdfRule};
use crate::health::server::{ConversionRecord, HealthServer};
use crate::logs::error_logger::ErrorLogger;
use crate::utils::path::get_base_name;

use super::disk::check_disk_space;
use super::namer::OutputNamer;

pub async fn process_pdf(
    watcher_name: String,
    file_name: String,
    file_path: PathBuf,
    rule: &PdfRule,
    output_folder: &str,
    watch_folder: &str,
    error_logger: Arc<ErrorLogger>,
    health_server: Arc<HealthServer>,
    disk_config: &DiskSpaceConfig,
) {
    if let Err(e) = check_disk_space(output_folder, watch_folder, disk_config).await {
        error!("Disk space check failed: {}", e);
        let _ = health_server.increment_error(&watcher_name);
        return;
    }

    let _ = health_server.set_processing(watcher_name.clone(), file_name.clone());

    let output_folder_path = PathBuf::from(output_folder);
    let base_name = get_base_name(&file_name);
    let ext = rule.output_ext.trim_start_matches('.');
    let output_path = match OutputNamer::generate_path(
        &output_folder_path,
        &base_name,
        &rule.output_name_template,
        "pdf",
        ext,
    ) {
        Ok(p) => p,
        Err(_) => OutputNamer::generate_with_counter(&output_folder_path, &base_name, "pdf", ext),
    };

    match dispatch_pdf_mode(&file_path, &output_path, rule).await {
        Ok(()) => {
            info!("PDF conversion succeeded: {}", file_name);
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
            let msg = format!("PDF conversion failed: {}", e);
            error!("{}", msg);
            error_logger.log(&msg, &file_name, "pdf::process");
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

    let _ = health_server.clear_processing(&watcher_name);
}

async fn dispatch_pdf_mode(input: &Path, output: &Path, rule: &PdfRule) -> Result<()> {
    match rule.mode {
        PdfMode::Compress => compress_pdf(input, output, rule.quality.as_ref()).await,
        PdfMode::PdfA => convert_to_pdfa(input, output, &rule.pdfa_version).await,
        PdfMode::ExtractText => extract_text(input, output).await,
        PdfMode::ExtractImages => extract_images(input, output, rule.resolution).await,
        PdfMode::ImageToPdf => images_to_pdf(input, output).await,
        PdfMode::Merge => merge_pdfs(input, output).await,
        PdfMode::Linearize => linearize_pdf(input, output).await,
        PdfMode::Encrypt => encrypt_pdf(input, output, &rule.password).await,
        PdfMode::Decrypt => decrypt_pdf(input, output, &rule.password).await,
        PdfMode::Analyze => analyze_pdf(input, output).await,
    }
}

async fn compress_pdf(input: &Path, output: &Path, quality: Option<&PdfQuality>) -> Result<()> {
    let setting = match quality {
        Some(PdfQuality::Screen) => "/screen",
        Some(PdfQuality::Ebook) => "/ebook",
        Some(PdfQuality::Printer) => "/printer",
        Some(PdfQuality::Prepress) => "/prepress",
        _ => "/default",
    };

    let output_result = Command::new("gs")
        .args(["-sDEVICE=pdfwrite", "-dCompatibilityLevel=1.4"])
        .arg(format!("-dPDFSETTINGS={}", setting))
        .args(["-dNOPAUSE", "-dQUIET", "-dBATCH"])
        .arg(format!("-sOutputFile={}", output.display()))
        .arg(input.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("Ghostscript failed: {}", stderr);
    }
    Ok(())
}

async fn convert_to_pdfa(input: &Path, output: &Path, version: &Option<String>) -> Result<()> {
    let pdfa = match version.as_deref() {
        Some("2b") | None => "2",
        Some("3b") => "3",
        Some("4") => "4",
        _ => "2",
    };

    let output_result = Command::new("gs")
        .args(["-sDEVICE=pdfwrite", &format!("-dPDFA={}", pdfa)])
        .args([
            "-dPDFACompatibilityPolicy=1",
            "-dNOPAUSE",
            "-dQUIET",
            "-dBATCH",
        ])
        .arg(format!("-sOutputFile={}", output.display()))
        .arg(input.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("Ghostscript PDF/A failed: {}", stderr);
    }
    Ok(())
}

async fn extract_text(input: &Path, output: &Path) -> Result<()> {
    let output_result = Command::new("pdftotext")
        .arg(input.as_os_str())
        .arg(output.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("pdftotext failed: {}", stderr);
    }
    Ok(())
}

async fn extract_images(input: &Path, output: &Path, resolution: Option<u32>) -> Result<()> {
    let dpi = resolution.unwrap_or(300);
    let prefix = output.with_extension("");

    let output_result = Command::new("pdfimages")
        .arg("-png")
        .arg("-r")
        .arg(dpi.to_string())
        .arg(input.as_os_str())
        .arg(prefix.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("pdfimages failed: {}", stderr);
    }
    Ok(())
}

async fn images_to_pdf(input: &Path, output: &Path) -> Result<()> {
    let output_result = Command::new("img2pdf")
        .arg(input.as_os_str())
        .arg("-o")
        .arg(output.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("img2pdf failed: {}", stderr);
    }
    Ok(())
}

async fn linearize_pdf(input: &Path, output: &Path) -> Result<()> {
    let output_result = Command::new("qpdf")
        .arg("--linearize")
        .arg(input.as_os_str())
        .arg(output.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("qpdf linearize failed: {}", stderr);
    }
    Ok(())
}

async fn encrypt_pdf(input: &Path, output: &Path, password: &Option<String>) -> Result<()> {
    let pw = password.as_deref().unwrap_or("default");

    let output_result = Command::new("qpdf")
        .arg("--encrypt")
        .arg(pw)
        .arg(pw)
        .arg("256")
        .arg("--")
        .arg(input.as_os_str())
        .arg(output.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("qpdf encrypt failed: {}", stderr);
    }
    Ok(())
}

async fn decrypt_pdf(input: &Path, output: &Path, password: &Option<String>) -> Result<()> {
    let mut cmd = Command::new("qpdf");
    cmd.arg("--decrypt");
    if let Some(pw) = password {
        cmd.arg(format!("--password={}", pw));
    }
    cmd.arg(input.as_os_str())
        .arg(output.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output_result = cmd.output().await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("qpdf decrypt failed: {}", stderr);
    }
    Ok(())
}

async fn merge_pdfs(_input_dir: &Path, _output: &Path) -> Result<()> {
    bail!("PDF merge mode: place multiple PDFs in input folder")
}

async fn analyze_pdf(input: &Path, _output: &Path) -> Result<()> {
    let output_result = Command::new("pdfinfo")
        .arg(input.as_os_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        bail!("pdfinfo failed: {}", stderr);
    }
    Ok(())
}
