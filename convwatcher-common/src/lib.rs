//! Shared code between the ConvWatcher coordinator (server) and the remote
//! worker agent. Contains the wire protocol, TCP framing/transport, UDP
//! discovery, ffmpeg argument construction and shared config types.

pub mod config;
pub mod discovery;
pub mod ffmpeg;
pub mod protocol;
pub mod transport;

pub use config::WorkerIoMode;
pub use protocol::{JobKind, Message, WireAudioRule, WireVideoRule};
