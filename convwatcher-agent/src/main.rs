//! ConvWatcher worker agent.
//!
//! A lightweight, headless process (RPi/NanoPi friendly) that only needs ffmpeg
//! and ffprobe installed. It discovers the coordinator over UDP, opens a locked
//! TCP connection, and executes streamed video/audio conversions.

mod runner;

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use log::{error, info, warn};
use serde::Deserialize;
use tokio::net::TcpStream;

use convwatcher_common::config::{WorkerIoMode, DEFAULT_DISCOVERY_PORT};
use convwatcher_common::discovery::discover_coordinator;
use convwatcher_common::protocol::{Capabilities, Message};
use convwatcher_common::transport::{read_message, write_message};

#[derive(Parser, Debug)]
#[command(name = "convwatcher-agent")]
#[command(about = "ConvWatcher remote worker agent (video/audio via ffmpeg)", long_about = None)]
struct Cli {
    /// Optional YAML config file (fields mirror the CLI flags).
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,

    /// Coordinator address (host:port). If omitted, discover via UDP beacon.
    #[arg(long)]
    coordinator_addr: Option<String>,

    /// UDP discovery port (must match the coordinator).
    #[arg(long)]
    discovery_port: Option<u16>,

    /// Shared secret; must match the coordinator's embedded_secret.
    #[arg(long)]
    secret: Option<String>,

    /// Path to the ffmpeg binary.
    #[arg(long)]
    ffmpeg_path: Option<String>,

    /// Path to the ffprobe binary (defaults to ffmpeg dir).
    #[arg(long)]
    ffprobe_path: Option<String>,

    /// Directory for temp files in `temp` io mode (point at a tmpfs/ramdisk).
    #[arg(long)]
    temp_dir: Option<String>,

    /// I/O mode: `temp` (default) or `pipe`.
    #[arg(long)]
    io_mode: Option<String>,

    /// Stable agent id (defaults to hostname).
    #[arg(long)]
    agent_id: Option<String>,

    #[arg(long, default_value = "info")]
    level: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FileConfig {
    coordinator_addr: Option<String>,
    discovery_port: Option<u16>,
    secret: Option<String>,
    ffmpeg_path: Option<String>,
    ffprobe_path: Option<String>,
    temp_dir: Option<String>,
    io_mode: Option<String>,
    agent_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub coordinator_addr: Option<String>,
    pub discovery_port: u16,
    pub secret: String,
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
    pub temp_dir: String,
    pub io_mode: WorkerIoMode,
    pub agent_id: String,
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "convwatcher-agent".to_string())
}

fn resolve_config(cli: &Cli) -> Result<AgentConfig> {
    let file: FileConfig = match &cli.config {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("read config file {}", path.display()))?;
            serde_yaml::from_str(&text).context("parse agent config YAML")?
        }
        None => FileConfig::default(),
    };

    let ffmpeg_path = cli
        .ffmpeg_path
        .clone()
        .or(file.ffmpeg_path)
        .unwrap_or_else(|| "/usr/bin/ffmpeg".to_string());

    let ffprobe_path = cli
        .ffprobe_path
        .clone()
        .or(file.ffprobe_path)
        .unwrap_or_else(|| {
            // Default: sibling "ffprobe" of the ffmpeg binary.
            std::path::Path::new(&ffmpeg_path)
                .parent()
                .map(|p| p.join("ffprobe").to_string_lossy().to_string())
                .unwrap_or_else(|| "/usr/bin/ffprobe".to_string())
        });

    let io_mode = cli
        .io_mode
        .clone()
        .or(file.io_mode)
        .map(|s| s.parse())
        .transpose()
        .map_err(|e: String| anyhow::anyhow!(e))?
        .unwrap_or_default();

    Ok(AgentConfig {
        coordinator_addr: cli.coordinator_addr.clone().or(file.coordinator_addr),
        discovery_port: cli
            .discovery_port
            .or(file.discovery_port)
            .unwrap_or(DEFAULT_DISCOVERY_PORT),
        secret: cli.secret.clone().or(file.secret).unwrap_or_default(),
        ffmpeg_path,
        ffprobe_path,
        temp_dir: cli
            .temp_dir
            .clone()
            .or(file.temp_dir)
            .unwrap_or_else(|| "/tmp".to_string()),
        io_mode,
        agent_id: cli
            .agent_id
            .clone()
            .or(file.agent_id)
            .unwrap_or_else(hostname),
    })
}

fn setup_logging(level: &str) {
    let level = match level.to_lowercase().as_str() {
        "debug" => log::LevelFilter::Debug,
        "warn" => log::LevelFilter::Warn,
        "error" => log::LevelFilter::Error,
        _ => log::LevelFilter::Info,
    };
    let _ = fern::Dispatch::new()
        .level(level)
        .format(|out, message, record| out.finish(format_args!("[{}] {}", record.level(), message)))
        .chain(std::io::stdout())
        .apply();
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    setup_logging(&cli.level);

    let config = match resolve_config(&cli) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("agent config error: {e:#}");
            std::process::exit(1);
        }
    };

    info!(
        "convwatcher-agent starting (id={}, io_mode={}, ffmpeg={})",
        config.agent_id, config.io_mode, config.ffmpeg_path
    );

    loop {
        if let Err(e) = connect_and_serve(&config).await {
            warn!("connection ended: {e:#}; retrying in 3s");
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn connect_and_serve(config: &AgentConfig) -> Result<()> {
    let addr = match &config.coordinator_addr {
        Some(a) => a
            .parse()
            .with_context(|| format!("invalid coordinator_addr '{a}'"))?,
        None => {
            info!("discovering coordinator via UDP beacon...");
            discover_coordinator(
                config.discovery_port,
                &config.agent_id,
                Duration::from_secs(3),
            )
            .await?
        }
    };

    info!("connecting to coordinator at {addr}");
    let stream = TcpStream::connect(addr).await.context("tcp connect")?;
    stream.set_nodelay(true).ok();
    let (mut reader, mut writer) = stream.into_split();

    write_message(
        &mut writer,
        &Message::Register {
            agent_id: config.agent_id.clone(),
            secret: config.secret.clone(),
            caps: Capabilities {
                ffmpeg: true,
                io_mode: config.io_mode,
            },
        },
    )
    .await
    .context("send register")?;

    match read_message(&mut reader)
        .await
        .context("await register ack")?
    {
        Message::RegisterAck { server_id } => {
            info!("registered with coordinator {server_id}");
        }
        Message::RegisterReject { reason } => {
            error!("coordinator rejected registration: {reason}");
            return Err(anyhow::anyhow!("registration rejected: {reason}"));
        }
        other => {
            return Err(anyhow::anyhow!("unexpected reply to register: {other:?}"));
        }
    }

    // Main job loop.
    loop {
        let msg = read_message(&mut reader).await.context("read message")?;
        match msg {
            Message::Job {
                job_id,
                kind,
                video_rule,
                audio_rule,
                output_ext,
                io_mode,
                input_len,
            } => {
                runner::handle_job(
                    config,
                    &mut reader,
                    &mut writer,
                    job_id,
                    kind,
                    video_rule,
                    audio_rule,
                    &output_ext,
                    io_mode,
                    input_len,
                )
                .await?;
            }
            Message::Heartbeat => {
                write_message(&mut writer, &Message::Heartbeat).await.ok();
            }
            Message::JobAbort { job_id } => {
                // Jobs run synchronously in this loop; an abort for a finished
                // job is a no-op. (Timeout enforcement is server-side.)
                warn!("received abort for job {job_id} (no active job)");
            }
            Message::Bye => {
                info!("coordinator closed the connection");
                return Ok(());
            }
            other => warn!("ignoring unexpected message: {other:?}"),
        }
    }
}
