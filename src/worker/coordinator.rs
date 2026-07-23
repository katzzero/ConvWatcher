//! The coordinator's pool of connected worker agents.
//!
//! Responsibilities:
//!   * run the UDP discovery responder;
//!   * accept locked TCP connections from agents and authenticate them;
//!   * track each agent's in-flight load;
//!   * dispatch a video/audio job to the least-loaded idle agent, streaming the
//!     input file over and writing the returned output to a local path.
//!
//! A job dispatch is a synchronous request/response on a single agent's
//! connection (one job per connection at a time), guarded by a per-agent async
//! mutex. If no agent is available, [`WorkerPool::dispatch`] returns
//! `Ok(None)` so the caller can fall back to local processing.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use log::{error, info, warn};
use socket2::SockRef;
use subtle::ConstantTimeEq;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use convwatcher_common::config::{WorkerIoMode, DEFAULT_COORDINATOR_PORT, DEFAULT_DISCOVERY_PORT};
use convwatcher_common::discovery::serve_discovery;
use convwatcher_common::protocol::{JobKind, Message, WireAudioRule, WireVideoRule};
use convwatcher_common::transport::{read_message, read_stream, write_message, write_stream};

/// A single connected agent.
struct Agent {
    id: String,
    io_mode: WorkerIoMode,
    /// Serializes access to the connection (one job at a time).
    conn: Mutex<AgentConn>,
    inflight: AtomicU64,
    alive: AtomicBool,
}

struct AgentConn {
    reader: OwnedReadHalf,
    writer: OwnedWriteHalf,
    alive: bool,
}

/// Describes a single conversion to run remotely.
pub struct RemoteJob<'a> {
    pub kind: JobKind,
    pub video_rule: Option<WireVideoRule>,
    pub audio_rule: Option<WireAudioRule>,
    pub output_ext: String,
    /// Local input file to stream to the agent.
    pub input_path: &'a Path,
    /// Local path to write the returned output to.
    pub output_path: &'a Path,
}

impl<'a> RemoteJob<'a> {
    pub fn new(
        kind: JobKind,
        video_rule: Option<WireVideoRule>,
        audio_rule: Option<WireAudioRule>,
        output_ext: impl Into<String>,
        input_path: &'a Path,
        output_path: &'a Path,
    ) -> Self {
        Self {
            kind,
            video_rule,
            audio_rule,
            output_ext: output_ext.into(),
            input_path,
            output_path,
        }
    }
}

/// The pool shared across the coordinator.
pub struct WorkerPool {
    secret: String,
    server_id: String,
    agents: Mutex<HashMap<String, Arc<Agent>>>,
    job_seq: AtomicU64,
}

impl WorkerPool {
    pub fn new(secret: String) -> Arc<Self> {
        Arc::new(Self {
            secret,
            server_id: format!("convwatcher-server-{}", std::process::id()),
            agents: Mutex::new(HashMap::new()),
            job_seq: AtomicU64::new(1),
        })
    }

    /// Number of currently connected agents.
    pub async fn agent_count(&self) -> usize {
        self.agents.lock().await.len()
    }

    /// Spawn the discovery responder and TCP accept loop.
    pub fn spawn(
        self: &Arc<Self>,
        bind_addr: String,
        advertise_addr: String,
        discovery_port: Option<u16>,
        coordinator_port: Option<u16>,
    ) {
        let discovery_port = discovery_port.unwrap_or(DEFAULT_DISCOVERY_PORT);
        let coordinator_port = coordinator_port.unwrap_or(DEFAULT_COORDINATOR_PORT);

        // Discovery responder.
        {
            let advertise = advertise_addr.clone();
            tokio::spawn(async move {
                if let Err(e) = serve_discovery(discovery_port, advertise, coordinator_port).await {
                    error!("discovery responder stopped: {e:#}");
                }
            });
        }

        // TCP accept loop.
        {
            let pool = self.clone();
            let bind = format!("{bind_addr}:{coordinator_port}");
            tokio::spawn(async move {
                match TcpListener::bind(&bind).await {
                    Ok(listener) => {
                        info!("Coordinator accepting agents on tcp://{bind}");
                        loop {
                            match listener.accept().await {
                                Ok((stream, peer)) => {
                                    let pool = pool.clone();
                                    tokio::spawn(async move {
                                        if let Err(e) = pool.handle_agent(stream, peer).await {
                                            warn!("agent {peer} disconnected: {e:#}");
                                        }
                                    });
                                }
                                Err(e) => warn!("accept error: {e}"),
                            }
                        }
                    }
                    Err(e) => error!("failed to bind coordinator on {bind}: {e}"),
                }
            });
        }
    }

    async fn handle_agent(
        self: Arc<Self>,
        stream: TcpStream,
        peer: std::net::SocketAddr,
    ) -> Result<()> {
        stream.set_nodelay(true).ok();
        {
            let sock_ref = SockRef::from(&stream);
            let _ = sock_ref.set_keepalive(true);
        }
        let (mut reader, mut writer) = stream.into_split();

        // First message must be Register.
        let (agent_id, io_mode) = match read_message(&mut reader).await? {
            Message::Register {
                agent_id,
                secret,
                caps,
            } => {
                if !self.secret_ok(&secret) {
                    write_message(
                        &mut writer,
                        &Message::RegisterReject {
                            reason: "invalid secret".into(),
                        },
                    )
                    .await
                    .ok();
                    bail!("agent {agent_id} @ {peer} rejected: bad secret");
                }
                write_message(
                    &mut writer,
                    &Message::RegisterAck {
                        server_id: self.server_id.clone(),
                    },
                )
                .await?;
                (agent_id, caps.io_mode)
            }
            other => bail!("expected Register, got {other:?}"),
        };

        info!("Agent registered: {agent_id} @ {peer} (io_mode={io_mode})");

        let agent = Arc::new(Agent {
            id: agent_id.clone(),
            io_mode,
            conn: Mutex::new(AgentConn {
                reader,
                writer,
                alive: true,
            }),
            inflight: AtomicU64::new(0),
            alive: AtomicBool::new(true),
        });

        self.agents
            .lock()
            .await
            .insert(agent_id.clone(), agent.clone());

        // Health-monitor loop: periodically probe the connection and check the
        // alive flag.  If the agent reconnects the old entry is replaced in the
        // map — the stale handler compares Arcs to avoid evicting the live one.
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let dead = {
                let mut guard = agent.conn.lock().await;
                if !guard.alive {
                    true
                } else {
                    // Liveness probe: write a heartbeat; on failure mark dead.
                    if write_message(&mut guard.writer, &Message::Heartbeat).await.is_err() {
                        guard.alive = false;
                        agent.alive.store(false, Ordering::SeqCst);
                        true
                    } else {
                        let _ = guard.writer.flush().await;
                        false
                    }
                }
            };
            if dead {
                break;
            }
        }

        // Only remove if the map still holds our Arc (not a replacement).
        {
            let mut agents = self.agents.lock().await;
            if let Some(existing) = agents.get(&agent_id) {
                if Arc::ptr_eq(existing, &agent) {
                    agents.remove(&agent_id);
                }
            }
        }
        info!("Agent removed: {agent_id}");
        Ok(())
    }

    fn secret_ok(&self, provided: &str) -> bool {
        if self.secret.is_empty() {
            warn!("embedded_secret is empty — accepting agent without authentication");
            return true;
        }
        self.secret.as_bytes().ct_eq(provided.as_bytes()).into()
    }

    /// Pick the least-loaded agent and run the job on it. Returns:
    ///   * `Ok(true)`  — dispatched and completed successfully;
    ///   * `Ok(false)` — no agent available (caller should fall back locally);
    ///   * `Err(_)`    — an agent took the job but it failed (caller should fall
    ///                   back locally after cleanup).
    pub async fn dispatch(&self, job: RemoteJob<'_>) -> Result<bool> {
        let agent = match self.pick_agent().await {
            Some(a) => a,
            None => return Ok(false),
        };

        agent.inflight.fetch_add(1, Ordering::SeqCst);
        let result = self.run_on_agent(&agent, job).await;
        agent.inflight.fetch_sub(1, Ordering::SeqCst);

        match result {
            Ok(()) => Ok(true),
            Err(e) => {
                // Mark the connection dead so the monitor loop evicts it; a
                // failed stream usually means a broken pipe.
                agent.conn.lock().await.alive = false;
                agent.alive.store(false, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    async fn pick_agent(&self) -> Option<Arc<Agent>> {
        let agents = self.agents.lock().await;
        agents
            .values()
            .filter(|a| a.alive.load(Ordering::SeqCst))
            .min_by_key(|a| a.inflight.load(Ordering::SeqCst))
            .cloned()
    }

    /// Default timeout for remote job execution.
    const JOB_TIMEOUT: Duration = Duration::from_secs(3600);

    async fn run_on_agent(&self, agent: &Arc<Agent>, job: RemoteJob<'_>) -> Result<()> {
        self.run_on_agent_with_timeout(agent, job, Self::JOB_TIMEOUT).await
    }

    async fn run_on_agent_with_timeout(&self, agent: &Arc<Agent>, job: RemoteJob<'_>, timeout: Duration) -> Result<()> {
        let job_id = self.job_seq.fetch_add(1, Ordering::SeqCst);
        let io_mode = agent.io_mode;

        let input_len = tokio::fs::metadata(job.input_path)
            .await
            .with_context(|| format!("stat input {}", job.input_path.display()))?
            .len();

        let mut guard = agent.conn.lock().await;
        if !guard.alive {
            bail!("agent {} connection is dead", agent.id);
        }

        // Send job header + input stream.
        write_message(
            &mut guard.writer,
            &Message::Job {
                job_id,
                kind: job.kind,
                video_rule: job.video_rule.clone(),
                audio_rule: job.audio_rule.clone(),
                output_ext: job.output_ext.clone(),
                io_mode,
                input_len,
            },
        )
        .await
        .context("send job header")?;

        {
            let mut f = tokio::fs::File::open(job.input_path)
                .await
                .context("open input for streaming")?;
            write_stream(&mut guard.writer, &mut f, input_len)
                .await
                .context("stream input to agent")?;
            guard.writer.flush().await.ok();
        }

        // Read responses until JobResult, with timeout.
        let result = tokio::time::timeout(timeout, async {
            loop {
                let msg = read_message(&mut guard.reader)
                    .await
                    .context("read agent response")?;
                match msg {
                    Message::JobStatus { .. } => { /* informational */ }
                    Message::JobOutputStart {
                        job_id: jid,
                        output_len,
                    } => {
                        if jid != job_id {
                            bail!("output start for wrong job: {jid} != {job_id}");
                        }
                        if let Some(parent) = job.output_path.parent() {
                            tokio::fs::create_dir_all(parent).await.ok();
                        }
                        let mut out = tokio::fs::File::create(job.output_path)
                            .await
                            .with_context(|| {
                                format!("create output {}", job.output_path.display())
                            })?;
                        read_stream(&mut guard.reader, &mut out, output_len)
                            .await
                            .context("receive output stream")?;
                    }
                    Message::JobResult {
                        job_id: jid,
                        ok,
                        error,
                    } => {
                        if jid != job_id {
                            bail!("result for wrong job: {jid} != {job_id}");
                        }
                        if ok {
                            return Ok(());
                        }
                        // Clean up any partial output we wrote.
                        let _ = tokio::fs::remove_file(job.output_path).await;
                        bail!("agent job failed: {}", error.unwrap_or_default());
                    }
                    other => bail!("unexpected message during job: {other:?}"),
                }
            }
        })
        .await;

        match result {
            Ok(r) => r,
            Err(_elapsed) => {
                let _ = write_message(&mut guard.writer, &Message::JobAbort { job_id }).await;
                let _ = guard.writer.flush().await;
                bail!("job {job_id} timed out on agent");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_job_abort_sent_on_timeout() {
        use convwatcher_common::transport::read_frame;

        // We test the timeout path by sending a JobRequest then streaming
        // input, which the agent side consumes. After the stream, the
        // server enters the response-read loop. The agent sends a
        // JobStatus (informational) so the loop continues. Then the agent
        // stops responding, causing the inner read to block until the
        // 500ms timeout fires.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let abort_seen = Arc::new(AtomicBool::new(false));
        let abort_seen_clone = abort_seen.clone();
        let server_handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut agent_reader, mut agent_writer) = tokio::io::split(stream);

            // Consume the initial JobRequest.
            let _ = read_frame(&mut agent_reader).await;

            // Consume stream data frames (write_stream sends raw frames).
            loop {
                match read_frame(&mut agent_reader).await {
                    Ok(frame) => {
                        // Check if this frame happens to be a JSON message.
                        if let Ok(msg) = serde_json::from_slice::<Message>(&frame) {
                            match msg {
                                Message::JobAbort { .. } => {
                                    abort_seen_clone.store(true, Ordering::SeqCst);
                                    return;
                                }
                                _ => {
                                    // Acknowledge non-abort messages to keep
                                    // the server in its response loop.
                                    let _ = write_message(
                                        &mut agent_writer,
                                        &Message::JobStatus { job_id: 0, state: "ok".into() },
                                    ).await;
                                    let _ = agent_writer.flush().await;
                                }
                            }
                        }
                        // Binary data frame: continue consuming.
                    }
                    Err(_) => return,
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let client_stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (s_reader, s_writer) = client_stream.into_split();

        let pool = WorkerPool::new("test".to_string());
        let agent = Arc::new(Agent {
            id: "test-agent".to_string(),
            io_mode: WorkerIoMode::Temp,
            conn: Mutex::new(AgentConn {
                reader: s_reader,
                writer: s_writer,
                alive: true,
            }),
            inflight: AtomicU64::new(0),
            alive: AtomicBool::new(true),
        });
        pool.agents.lock().await.insert("test-agent".to_string(), agent.clone());

        let tmp = std::env::temp_dir().join(format!("cw-test-abort-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp);
        let input = tmp.join("input.bin");
        let output = tmp.join("output.bin");
        std::fs::write(&input, b"some test input data").unwrap();

        let job = RemoteJob {
            kind: convwatcher_common::protocol::JobKind::Video,
            video_rule: Some(convwatcher_common::protocol::WireVideoRule::default()),
            audio_rule: None,
            output_ext: "mp4".to_string(),
            input_path: &input,
            output_path: &output,
        };

        let result = pool.run_on_agent_with_timeout(&agent, job, Duration::from_millis(500)).await;
        assert!(result.is_err(), "expected timeout error");
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("timed out"), "error should mention 'timed out': {err}");

        server_handle.await.unwrap();
        assert!(abort_seen.load(Ordering::SeqCst), "JobAbort was not sent to the agent");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
