//! Executes a single streamed conversion job on the agent.

use std::process::Stdio;

use anyhow::{Context, Result};
use log::{error, info};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::process::Command;

use convwatcher_common::config::WorkerIoMode;
use convwatcher_common::ffmpeg::{build_audio_args, build_video_args, INPUT_TOKEN, OUTPUT_TOKEN};
use convwatcher_common::protocol::{JobKind, Message, WireAudioRule, WireVideoRule};
use convwatcher_common::transport::{read_stream, write_message, write_stream};

use crate::AgentConfig;

#[allow(clippy::too_many_arguments)]
pub async fn handle_job(
    config: &AgentConfig,
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    job_id: u64,
    kind: JobKind,
    video_rule: Option<WireVideoRule>,
    audio_rule: Option<WireAudioRule>,
    output_ext: &str,
    io_mode: WorkerIoMode,
    input_len: u64,
) -> Result<()> {
    info!(
        "job {job_id}: {:?} conversion, {} bytes, mode={io_mode}",
        kind, input_len
    );
    write_message(
        writer,
        &Message::JobStatus {
            job_id,
            state: "running".into(),
        },
    )
    .await
    .ok();

    let args = build_args(kind, &video_rule, &audio_rule, output_ext, io_mode);

    let result = match io_mode {
        WorkerIoMode::Temp => {
            run_temp(config, reader, writer, job_id, input_len, output_ext, args).await
        }
        WorkerIoMode::Pipe => run_pipe(config, reader, writer, job_id, input_len, args).await,
    };

    match result {
        Ok(()) => {
            write_message(
                writer,
                &Message::JobResult {
                    job_id,
                    ok: true,
                    error: None,
                },
            )
            .await
            .ok();
            info!("job {job_id}: completed");
        }
        Err(e) => {
            error!("job {job_id}: failed: {e:#}");
            write_message(
                writer,
                &Message::JobResult {
                    job_id,
                    ok: false,
                    error: Some(format!("{e:#}")),
                },
            )
            .await
            .ok();
        }
    }
    Ok(())
}

fn build_args(
    kind: JobKind,
    video_rule: &Option<WireVideoRule>,
    audio_rule: &Option<WireAudioRule>,
    output_ext: &str,
    io_mode: WorkerIoMode,
) -> Vec<String> {
    match kind {
        JobKind::Video => {
            build_video_args(&video_rule.clone().unwrap_or_default(), output_ext, io_mode)
        }
        JobKind::Audio => {
            build_audio_args(&audio_rule.clone().unwrap_or_default(), output_ext, io_mode)
        }
    }
}

/// Temp mode: receive input to a temp file, run ffmpeg file-in/file-out, stream
/// the temp output file back. Temp files are removed on completion (and on drop
/// of the guard on error paths).
async fn run_temp(
    config: &AgentConfig,
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    job_id: u64,
    input_len: u64,
    output_ext: &str,
    args: Vec<String>,
) -> Result<()> {
    let base = std::path::Path::new(&config.temp_dir);
    tokio::fs::create_dir_all(base).await.ok();
    let safe_ext = sanitize_ext(output_ext);
    let in_path = base.join(format!("cw-{job_id}-in.tmp"));
    let out_path = base.join(format!("cw-{job_id}-out.{safe_ext}"));

    let _guard = TempGuard(vec![in_path.clone(), out_path.clone()]);

    // Receive input bytes into the temp file.
    {
        let mut f = File::create(&in_path)
            .await
            .with_context(|| format!("create temp input {}", in_path.display()))?;
        read_stream(reader, &mut f, input_len)
            .await
            .context("receive input stream")?;
    }

    // Substitute placeholders with real paths.
    let real_args: Vec<String> = args
        .into_iter()
        .map(|a| match a.as_str() {
            INPUT_TOKEN => in_path.to_string_lossy().to_string(),
            OUTPUT_TOKEN => out_path.to_string_lossy().to_string(),
            _ => a,
        })
        .collect();

    let output = Command::new(&config.ffmpeg_path)
        .kill_on_drop(true)
        .args(&real_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawn ffmpeg")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffmpeg failed: {}", tail(&stderr, 800));
    }

    // Stream the output file back.
    let meta = tokio::fs::metadata(&out_path)
        .await
        .with_context(|| format!("stat output {}", out_path.display()))?;
    let out_len = meta.len();
    write_message(
        writer,
        &Message::JobOutputStart {
            job_id,
            output_len: out_len,
        },
    )
    .await
    .context("send output start")?;
    let mut f = File::open(&out_path)
        .await
        .context("open output for streaming")?;
    write_stream(writer, &mut f, out_len)
        .await
        .context("stream output back")?;
    writer.flush().await.ok();

    Ok(())
}

/// Pipe mode: feed input into ffmpeg stdin, capture stdout, stream it back.
/// No temp files. stdout is fully buffered into memory before framing — for
/// very large outputs prefer temp mode.
async fn run_pipe(
    config: &AgentConfig,
    reader: &mut OwnedReadHalf,
    writer: &mut OwnedWriteHalf,
    job_id: u64,
    input_len: u64,
    args: Vec<String>,
) -> Result<()> {
    let mut child = Command::new(&config.ffmpeg_path)
        .kill_on_drop(true)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn ffmpeg (pipe)")?;

    let mut stdin = child.stdin.take().context("ffmpeg stdin unavailable")?;
    let mut stdout = child.stdout.take().context("ffmpeg stdout unavailable")?;
    let mut stderr = child.stderr.take().context("ffmpeg stderr unavailable")?;

    // Receive input from the socket and feed it to ffmpeg stdin concurrently
    // with draining stdout and stderr, otherwise ffmpeg can deadlock on a full pipe.
    let mut remaining = input_len;
    let feed = async {
        while remaining > 0 {
            let chunk = convwatcher_common::transport::read_frame(reader).await?;
            if chunk.is_empty() {
                anyhow::bail!("empty input chunk with {remaining} bytes remaining");
            }
            if (chunk.len() as u64) > remaining {
                anyhow::bail!(
                    "received {} bytes but only {remaining} bytes remaining",
                    chunk.len()
                );
            }
            stdin
                .write_all(&chunk)
                .await
                .context("write ffmpeg stdin")?;
            remaining -= chunk.len() as u64;
        }
        stdin.shutdown().await.ok();
        drop(stdin);
        Ok::<(), anyhow::Error>(())
    };

    // Drain stdout into memory while feeding stdin.
    let mut out_buf: Vec<u8> = Vec::new();
    let drain = async {
        stdout
            .read_to_end(&mut out_buf)
            .await
            .context("read ffmpeg stdout")?;
        Ok::<Vec<u8>, anyhow::Error>(out_buf)
    };

    // Drain stderr concurrently to prevent pipe-buffer deadlock.
    let drain_stderr = async {
        let mut buf = Vec::new();
        stderr
            .read_to_end(&mut buf)
            .await
            .context("read ffmpeg stderr")?;
        Ok::<Vec<u8>, anyhow::Error>(buf)
    };

    let (feed_res, drain_res, drain_err_res) = tokio::join!(feed, drain, drain_stderr);
    feed_res?;
    let out_bytes = drain_res?;
    let err_bytes = drain_err_res?;

    let status = child.wait().await.context("await ffmpeg")?;
    if !status.success() {
        let err_str = String::from_utf8_lossy(&err_bytes);
        anyhow::bail!("ffmpeg failed: {}", tail(&err_str, 800));
    }

    write_message(
        writer,
        &Message::JobOutputStart {
            job_id,
            output_len: out_bytes.len() as u64,
        },
    )
    .await
    .context("send output start")?;
    let mut cursor = std::io::Cursor::new(out_bytes.as_slice());
    write_stream(writer, &mut cursor, out_bytes.len() as u64)
        .await
        .context("stream output back")?;
    writer.flush().await.ok();

    Ok(())
}

/// Restrict `output_ext` to a safe charset to prevent path traversal via
/// the temp-file name. Returns `"bin"` when the extension is invalid.
fn sanitize_ext(ext: &str) -> &str {
    let ext = ext.trim_start_matches('.');
    if ext.len() > 16
        || ext.is_empty()
        || !ext.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+' || c == '-')
    {
        return "bin";
    }
    ext
}

fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s[s.len() - max..].to_string()
    }
}

/// Best-effort cleanup of temp files.
struct TempGuard(Vec<std::path::PathBuf>);

impl Drop for TempGuard {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}
