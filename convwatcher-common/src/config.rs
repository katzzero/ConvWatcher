//! Shared configuration types for the worker system.

use serde::{Deserialize, Serialize};

/// Default UDP port that agents broadcast to and the server listens on for
/// discovery beacons.
pub const DEFAULT_DISCOVERY_PORT: u16 = 8687;

/// Default TCP port the coordinator listens on for locked worker connections.
pub const DEFAULT_COORDINATOR_PORT: u16 = 8688;

/// How the worker performs input/output relative to ffmpeg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerIoMode {
    /// Write the streamed input to a temp file, run ffmpeg file-in/file-out,
    /// then stream the temp output file back. Works for any format. Point
    /// `temp_dir` at a tmpfs/ramdisk to avoid SD-card wear.
    #[default]
    Temp,
    /// Feed the input into ffmpeg `stdin` (`-i pipe:0`) and stream ffmpeg
    /// `stdout` (`-f <fmt> -`) back. Zero temp files. Single-output formats
    /// only (matroska/webm).
    Pipe,
}

impl std::str::FromStr for WorkerIoMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "temp" | "file" => Ok(WorkerIoMode::Temp),
            "pipe" | "stream" => Ok(WorkerIoMode::Pipe),
            other => Err(format!(
                "invalid worker io mode: '{other}' (expected 'temp' or 'pipe')"
            )),
        }
    }
}

impl std::fmt::Display for WorkerIoMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerIoMode::Temp => f.write_str("temp"),
            WorkerIoMode::Pipe => f.write_str("pipe"),
        }
    }
}

/// The container format ffmpeg must mux to when writing to stdout in pipe mode.
/// Not every extension supports non-seekable output; we map the desired output
/// extension to a pipe-friendly muxer, defaulting to Matroska which accepts
/// most codecs.
pub fn pipe_output_format(output_ext: &str) -> &'static str {
    match output_ext.trim_start_matches('.').to_lowercase().as_str() {
        "webm" => "webm",
        "mka" | "mkv" | "mp4" | "mov" | "m4a" | "aac" => "matroska",
        "mp3" => "mp3",
        "ogg" | "oga" => "ogg",
        "flac" => "flac",
        "wav" => "wav",
        _ => "matroska",
    }
}
