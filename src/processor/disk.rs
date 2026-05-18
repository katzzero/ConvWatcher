use anyhow::{bail, Result};

use crate::config::global::{DiskSpaceConfig, DiskSpaceThreshold};

pub async fn check_disk_space(
    output_folder: &str,
    watch_folder: &str,
    config: &DiskSpaceConfig,
) -> Result<()> {
    #[cfg(unix)]
    {
        use std::fs;

        if config.check_output {
            if let Some(parent) = get_mount_point(output_folder) {
                check_available(&parent, &config.threshold, "output")?;
            }
        }

        if config.check_watch {
            if let Some(parent) = get_mount_point(watch_folder) {
                check_available(&parent, &config.threshold, "watch")?;
            }
        }

        let _ = fs::metadata(output_folder);
    }

    Ok(())
}

#[cfg(unix)]
fn get_mount_point(path: &str) -> Option<std::path::PathBuf> {
    use std::path::Path;

    let p = Path::new(path);
    let canonical = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());

    let mut current = canonical.as_path();
    loop {
        if let Some(parent) = current.parent() {
            if parent.as_os_str().is_empty() || parent == current {
                return Some(current.to_path_buf());
            }
            current = parent;
        } else {
            return Some(current.to_path_buf());
        }
    }
}

#[cfg(unix)]
fn check_available(
    mount: &std::path::Path,
    threshold: &DiskSpaceThreshold,
    label: &str,
) -> Result<()> {
    use std::process::Command;

    let output = Command::new("df")
        .arg(mount.as_os_str())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() < 2 {
        return Ok(());
    }

    let columns: Vec<&str> = lines[1].split_whitespace().collect();
    let total_kb: u64 = columns.get(1).unwrap_or(&"0").parse().unwrap_or(0);
    let available_kb: u64 = columns.get(3).unwrap_or(&"0").parse().unwrap_or(0);
    let available_bytes = available_kb * 1024;

    let required_bytes = match threshold {
        DiskSpaceThreshold::Mb(mb) => mb * 1024 * 1024,
        DiskSpaceThreshold::Gb(gb) => (*gb * 1024.0 * 1024.0 * 1024.0) as u64,
        DiskSpaceThreshold::Percent(pct) => {
            if total_kb == 0 {
                1
            } else {
                let min_free = (*pct / 100.0) * total_kb as f64;
                (min_free * 1024.0) as u64
            }
        }
    };

    if available_bytes < required_bytes {
        bail!(
            "Low disk space on {}: {} MB available, need {} MB",
            label,
            available_bytes / (1024 * 1024),
            required_bytes / (1024 * 1024)
        );
    }

    Ok(())
}

pub async fn disk_space_monitor(
    config: DiskSpaceConfig,
    output_folders: Vec<String>,
    watch_folders: Vec<String>,
) {
    let interval = tokio::time::Duration::from_secs(config.check_interval_s);
    let mut ticker = tokio::time::interval(interval);

    loop {
        ticker.tick().await;

        for folder in &output_folders {
            if let Err(e) = check_disk_space(folder, folder, &config).await {
                log::warn!("Disk space warning (output): {}", e);
            }
        }

        for folder in &watch_folders {
            if let Err(e) = check_disk_space(folder, folder, &config).await {
                log::warn!("Disk space warning (watch): {}", e);
            }
        }
    }
}
