use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct HardwareAccelInfo {
    pub ffmpeg_vaapi_support: bool,
    pub vaapi_available: bool,
    pub vaapi_encoders: Vec<String>,
    pub vaapi_devices: Vec<String>,
    pub nvenc_available: bool,
    pub nvenc_encoders: Vec<String>,
    pub qsv_available: bool,
    pub qsv_encoders: Vec<String>,
    pub rkmpp_available: bool,
    pub rkmpp_encoders: Vec<String>,
    pub all_encoders: Vec<String>,
}

impl Default for HardwareAccelInfo {
    fn default() -> Self {
        Self {
            ffmpeg_vaapi_support: false,
            vaapi_available: false,
            vaapi_encoders: Vec::new(),
            vaapi_devices: Vec::new(),
            nvenc_available: false,
            nvenc_encoders: Vec::new(),
            qsv_available: false,
            qsv_encoders: Vec::new(),
            rkmpp_available: false,
            rkmpp_encoders: Vec::new(),
            all_encoders: Vec::new(),
        }
    }
}

pub async fn check_hardware_accel(ffmpeg_path: &str) -> HardwareAccelInfo {
    let mut info = HardwareAccelInfo::default();

    let all_encoders = match get_encoders(ffmpeg_path).await {
        Ok(encoders) => encoders,
        Err(_) => return info,
    };

    info.all_encoders = all_encoders.clone();

    let encoder_names: Vec<String> = all_encoders
        .iter()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                Some(parts[1].to_string())
            } else {
                None
            }
        })
        .collect();

    info.vaapi_encoders = encoder_names
        .iter()
        .filter(|n| n.ends_with("_vaapi"))
        .cloned()
        .collect();
    info.ffmpeg_vaapi_support = !info.vaapi_encoders.is_empty();

    if std::path::Path::new("/dev/dri").exists() {
        if let Ok(entries) = std::fs::read_dir("/dev/dri") {
            info.vaapi_devices = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().map(|s| format!("/dev/dri/{}", s)))
                .collect();
            info.vaapi_available = !info.vaapi_devices.is_empty();
        }
    }

    info.nvenc_encoders = encoder_names
        .iter()
        .filter(|n| n.ends_with("_nvenc"))
        .cloned()
        .collect();
    info.nvenc_available = !info.nvenc_encoders.is_empty();

    info.qsv_encoders = encoder_names
        .iter()
        .filter(|n| n.ends_with("_qsv"))
        .cloned()
        .collect();
    info.qsv_available = !info.qsv_encoders.is_empty();

    info.rkmpp_encoders = encoder_names
        .iter()
        .filter(|n| n.ends_with("_rkmpp"))
        .cloned()
        .collect();
    info.rkmpp_available = !info.rkmpp_encoders.is_empty();

    info
}

async fn get_encoders(ffmpeg_path: &str) -> Result<Vec<String>, String> {
    let output = Command::new(ffmpeg_path)
        .args(["-hide_banner", "-encoders"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("Failed to run ffmpeg: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().map(|s| s.to_string()).collect())
}
