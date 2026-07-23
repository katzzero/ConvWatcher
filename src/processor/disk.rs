use crate::config::global::{DiskSpaceConfig, DiskSpaceThreshold};
use log::warn;

/// Returns `true` if disk space is critically low and conversions should be
/// paused (the caller should skip the current file and retry it later).
pub async fn check_disk_space(
    output_folder: &str,
    watch_folder: &str,
    config: &DiskSpaceConfig,
) -> bool {
    if !config.check_output && !config.check_watch {
        return false;
    }

    #[cfg(unix)]
    {
        let mut low = false;

        if config.check_output {
            if let Some(parent) = canonicalize_or_self(output_folder) {
                if let Err(e) = check_available(&parent, &config.threshold, "output").await {
                    warn!("Disk space low (output): {}", e);
                    low = true;
                }
            }
        }

        if config.check_watch {
            if let Some(parent) = canonicalize_or_self(watch_folder) {
                if let Err(e) = check_available(&parent, &config.threshold, "watch").await {
                    warn!("Disk space low (watch): {}", e);
                    low = true;
                }
            }
        }

        low
    }

    #[cfg(not(unix))]
    {
        let _ = (output_folder, watch_folder, config);
        false
    }
}

#[cfg(unix)]
fn canonicalize_or_self(path: &str) -> Option<std::path::PathBuf> {
    use std::path::Path;

    let p = Path::new(path);
    let current = if p.exists() {
        p.canonicalize().ok()?
    } else {
        p.to_path_buf()
    };

    Some(current)
}

#[cfg(unix)]
async fn check_available(
    mount: &std::path::Path,
    threshold: &DiskSpaceThreshold,
    label: &str,
) -> anyhow::Result<()> {
    let output = match tokio::process::Command::new("df")
        .arg("-kP")
        .arg(mount.as_os_str())
        .kill_on_drop(true)
        .output()
        .await
    {
        Ok(o) => o,
        Err(_) => return Ok(()),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() < 2 {
        return Ok(());
    }

    let columns: Vec<&str> = lines[1].split_whitespace().collect();
    if columns.len() < 4 {
        return Ok(());
    }

    let available_kb: u64 = columns.get(3).unwrap_or(&"0").parse().unwrap_or(0);
    let available_bytes = available_kb * 1024;

    let required_bytes = match threshold {
        DiskSpaceThreshold::Mb(mb) => mb * 1024 * 1024,
        DiskSpaceThreshold::Gb(gb) => (*gb * 1024.0 * 1024.0 * 1024.0) as u64,
        DiskSpaceThreshold::Percent(pct) => {
            let total_kb: u64 = columns.get(1).unwrap_or(&"0").parse().unwrap_or(0);
            if total_kb == 0 {
                1
            } else {
                let min_free = (*pct / 100.0) * total_kb as f64;
                (min_free * 1024.0) as u64
            }
        }
    };

    if available_bytes < required_bytes {
        anyhow::bail!(
            "{}: {} MB available, need {} MB",
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
            check_disk_space(folder, folder, &config).await;
        }

        for folder in &watch_folders {
            check_disk_space(folder, folder, &config).await;
        }
    }
}
