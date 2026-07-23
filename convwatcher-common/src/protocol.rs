//! Wire protocol between the coordinator (server) and worker agents.
//!
//! Messages are serialized with `serde_json` and framed by
//! [`crate::transport`] using a 4-byte big-endian length prefix. Bulk file
//! bytes (input/output streams) are NOT sent as `Message` variants; they are
//! written directly as length-prefixed chunks by the transport layer, bracketed
//! by [`Message::Job`]/[`Message::JobOutputStart`] which announce the total
//! byte length.

use serde::{Deserialize, Serialize};

use crate::config::WorkerIoMode;

/// Whether a job is a video or audio conversion. Agents only handle these two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Video,
    Audio,
}

/// The video-conversion parameters the agent needs to rebuild the exact ffmpeg
/// command. Mirrors the ffmpeg-relevant fields of the server's `VideoRule`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WireVideoRule {
    pub codec: Option<String>,
    pub quality: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_bitrate: Option<String>,
    /// Duration verification is done by the server (it has ffprobe and both
    /// files after transfer), so the agent does not need these — kept for
    /// completeness / future use.
    pub check_duration: Option<bool>,
    pub min_duration_ratio: Option<f64>,
}

/// The audio-conversion parameters the agent needs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WireAudioRule {
    pub audio_codec: Option<String>,
    pub audio_bitrate: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u8>,
}

/// Capabilities advertised by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub ffmpeg: bool,
    pub io_mode: WorkerIoMode,
}

/// Control messages exchanged over the locked TCP connection (and, for
/// [`Message::Beacon`]/[`Message::BeaconAck`], over UDP discovery).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Message {
    /// UDP: agent -> broadcast, looking for a coordinator.
    Beacon { agent_id: String },
    /// UDP: coordinator -> agent, announcing where to connect.
    BeaconAck { tcp_addr: String, tcp_port: u16 },

    /// TCP: agent -> server, first message after connecting.
    Register {
        agent_id: String,
        secret: String,
        caps: Capabilities,
    },
    /// TCP: server -> agent, connection accepted.
    RegisterAck { server_id: String },
    /// TCP: server -> agent, connection rejected (bad secret, etc.).
    RegisterReject { reason: String },

    /// TCP: server -> agent, announces a job. Immediately followed by
    /// `input_len` bytes streamed as transport chunks.
    Job {
        job_id: u64,
        kind: JobKind,
        video_rule: Option<WireVideoRule>,
        audio_rule: Option<WireAudioRule>,
        /// Output container extension (without dot), e.g. "mp4"/"mkv"/"mp3".
        output_ext: String,
        /// I/O mode the agent should use for this job.
        io_mode: WorkerIoMode,
        /// Total number of input bytes that follow this message.
        input_len: u64,
    },

    /// TCP: agent -> server, acknowledges it started running ffmpeg.
    JobStatus { job_id: u64, state: String },

    /// TCP: agent -> server, announces the output stream. Immediately followed
    /// by `output_len` bytes streamed as transport chunks.
    JobOutputStart { job_id: u64, output_len: u64 },

    /// TCP: agent -> server, final result of a job.
    JobResult {
        job_id: u64,
        ok: bool,
        /// Tail of ffmpeg stderr on failure (for logging).
        error: Option<String>,
    },

    /// TCP: server -> agent, requests the agent abort a running job (timeout).
    JobAbort { job_id: u64 },

    /// TCP: both directions, keepalive.
    Heartbeat,

    /// TCP: agent -> server, graceful disconnect.
    Bye,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_json_round_trip() {
        let msgs = vec![
            Message::Beacon {
                agent_id: "rpi-1".into(),
            },
            Message::BeaconAck {
                tcp_addr: "192.168.1.10".into(),
                tcp_port: 8688,
            },
            Message::Register {
                agent_id: "rpi-1".into(),
                secret: "s3cr3t".into(),
                caps: Capabilities {
                    ffmpeg: true,
                    io_mode: WorkerIoMode::Pipe,
                },
            },
            Message::Job {
                job_id: 42,
                kind: JobKind::Video,
                video_rule: Some(WireVideoRule {
                    codec: Some("libx264".into()),
                    quality: Some("crf 23".into()),
                    audio_codec: Some("aac".into()),
                    audio_bitrate: Some("128k".into()),
                    check_duration: Some(true),
                    min_duration_ratio: Some(0.9),
                }),
                audio_rule: None,
                output_ext: "mp4".into(),
                io_mode: WorkerIoMode::Temp,
                input_len: 123456,
            },
            Message::JobOutputStart {
                job_id: 42,
                output_len: 654321,
            },
            Message::JobResult {
                job_id: 42,
                ok: false,
                error: Some("boom".into()),
            },
            Message::Heartbeat,
            Message::Bye,
        ];

        for m in msgs {
            let bytes = serde_json::to_vec(&m).unwrap();
            let back: Message = serde_json::from_slice(&bytes).unwrap();
            let bytes2 = serde_json::to_vec(&back).unwrap();
            assert_eq!(bytes, bytes2);
        }
    }
}
