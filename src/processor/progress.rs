//! Progresso de conversão e captura de saída FFmpeg para o health dashboard.
//!
//! Fornece:
//! - `ProgressReporter`: publica percentual, ETA e speed de um job (vídeo/áudio)
//!   no `HealthServer`, com throttle de 1s para não lotar o servidor.
//! - `run_ffmpeg_with_progress`: roda ffmpeg lendo stdout em streaming (progresso)
//!   e drenando stderr em paralelo (evita deadlock), gravando logs com prefixo
//!   `[stdout]`/`[stderr]` no health server.
//! - `probe_duration`: obtém a duração do input via ffprobe antes do ffmpeg.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;

use crate::health::server::{HealthServer, ProgressInfo};

/// Throttle mínimo (ms) entre publicações de progresso no health server.
const PUBLISH_THROTTLE_MS: u64 = 1_000;

/// Estado mutável do progresso de um job (protegido por Mutex).
#[derive(Debug)]
struct ProgressState {
    duration_s: Option<f64>,
    out_time_s: Option<f64>,
    speed: Option<f64>,
    last_publish: Instant,
}

impl Default for ProgressState {
    fn default() -> Self {
        Self {
            duration_s: None,
            out_time_s: None,
            speed: None,
            last_publish: Instant::now(),
        }
    }
}

/// Resultado do parse de uma linha de progresso do ffmpeg.
#[derive(Default)]
struct ProgressUpdate {
    out_time_us: Option<u64>,
    out_time_ms: Option<u64>,
    out_time: Option<String>,
    speed: Option<f64>,
    progress_end: bool,
}

/// Reporter de progresso para um job de conversão (watcher, file).
#[derive(Clone)]
pub struct ProgressReporter {
    pub health_server: Arc<HealthServer>,
    pub watcher: String,
    pub file: String,
    state: Arc<tokio::sync::Mutex<ProgressState>>,
}

impl ProgressReporter {
    pub fn new(health_server: Arc<HealthServer>, watcher: String, file: String) -> Self {
        Self {
            health_server,
            watcher,
            file,
            state: Arc::new(tokio::sync::Mutex::new(ProgressState::default())),
        }
    }

    /// Define a duração total do input (segundos), obtida via ffprobe.
    pub fn set_duration(&self, duration_s: f64) {
        // Não bloqueia: seta de forma assíncrona (fire-and-forget) pois é
        // chamado uma vez no início da conversão.
        let hs = self.health_server.clone();
        let watcher = self.watcher.clone();
        let file = self.file.clone();
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut s = state.lock().await;
            s.duration_s = Some(duration_s);
            let _ = hs.set_progress(
                &watcher,
                &file,
                ProgressInfo {
                    percent: None,
                    out_time: None,
                    speed: None,
                    eta: None,
                },
            );
        });
    }

    /// Parseia uma linha de progresso do ffmpeg (`-progress pipe:1`) e publica
    /// no health server se o throttle permitir. `progress=end` sempre publica.
    pub fn parse_and_publish(&self, line: &str) {
        let update = parse_progress_line(line);
        let hs = self.health_server.clone();
        let watcher = self.watcher.clone();
        let file = self.file.clone();
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut publish_now = false;
            {
                let mut s = state.lock().await;
                if let Some(us) = update.out_time_us {
                    s.out_time_s = Some(us as f64 / 1_000_000.0);
                    publish_now = true;
                }
                if let Some(ms) = update.out_time_ms {
                    // ffmpeg's legacy out_time_ms is actually microseconds.
                    s.out_time_s = Some(ms as f64 / 1_000_000.0);
                    publish_now = true;
                }
                if let Some(ref t) = update.out_time {
                    if let Some(secs) = parse_hhmmss(t) {
                        s.out_time_s = Some(secs);
                        publish_now = true;
                    }
                }
                if let Some(sp) = update.speed {
                    s.speed = Some(sp);
                    publish_now = true;
                }
                if update.progress_end {
                    publish_now = true;
                }
                if publish_now
                    && s.last_publish.elapsed().as_millis() < PUBLISH_THROTTLE_MS as u128
                    && !update.progress_end
                {
                    publish_now = false;
                } else if publish_now {
                    s.last_publish = Instant::now();
                }
            }
            if publish_now {
                let s = state.lock().await;
                let percent = s
                    .out_time_s
                    .and_then(|ot| s.duration_s.map(|d| (ot / d * 100.0).clamp(0.0, 100.0)));
                let eta = s.out_time_s.and_then(|ot| {
                    s.duration_s
                        .and_then(|d| s.speed.filter(|&sp| sp > 0.0).map(|sp| (d - ot) / sp))
                });
                let info = ProgressInfo {
                    percent,
                    out_time: s.out_time_s,
                    speed: s.speed,
                    eta,
                };
                drop(s);
                let _ = hs.set_progress(&watcher, &file, info);
            }
            // progress=end garante publicação do estado final (fora do throttle).
            if update.progress_end {
                let s = state.lock().await;
                let percent = s
                    .out_time_s
                    .and_then(|ot| s.duration_s.map(|d| (ot / d * 100.0).clamp(0.0, 100.0)));
                let eta = s.out_time_s.and_then(|ot| {
                    s.duration_s
                        .and_then(|d| s.speed.filter(|&sp| sp > 0.0).map(|sp| (d - ot) / sp))
                });
                let info = ProgressInfo {
                    percent,
                    out_time: s.out_time_s,
                    speed: s.speed,
                    eta,
                };
                drop(s);
                let _ = hs.set_progress(&watcher, &file, info);
            }
        });
    }

    /// Limpa progresso e logs FFmpeg deste job (chamado ao terminar: sucesso,
    /// erro, timeout ou abort).
    pub fn clear(&self) {
        let hs = self.health_server.clone();
        let watcher = self.watcher.clone();
        let file = self.file.clone();
        tokio::spawn(async move {
            let _ = hs.clear_progress(&watcher, &file);
            let _ = hs.clear_ffmpeg_logs(&watcher, &file);
        });
    }

    /// Acumula uma linha de log FFmpeg com prefixo `[stdout]`/`[stderr]`.
    pub fn push_line(&self, stream: &str, line: &str) {
        let prefixed = format!("[{}] {}", stream, line);
        let hs = self.health_server.clone();
        let watcher = self.watcher.clone();
        let file = self.file.clone();
        tokio::spawn(async move {
            let _ = hs.push_ffmpeg_log(&watcher, &file, &prefixed);
        });
    }
}

/// Executa um `Command` (ffmpeg) lendo stdout em streaming para progresso e
/// drenando stderr em paralelo (evita deadlock). Retorna o `Output` com
/// stdout/stderr acumulados para que o chamador ainda possa inspecionar o
/// erro em caso de falha. O comando deve já ter `.kill_on_drop(true)` e
/// `.stdout/stderr(Stdio::piped())`.
pub async fn run_ffmpeg_with_progress(
    cmd: &mut Command,
    reporter: &ProgressReporter,
) -> Result<std::process::Output> {
    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn ffmpeg: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("ffmpeg stdout not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("ffmpeg stderr not piped"))?;

    let reporter_err = reporter.clone();
    let stderr_bytes: Arc<tokio::sync::Mutex<Vec<u8>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let stderr_bytes_clone = stderr_bytes.clone();

    let stderr_handle = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            reporter_err.push_line("stderr", &line);
            let mut b = stderr_bytes_clone.lock().await;
            b.extend_from_slice(line.as_bytes());
            b.push(b'\n');
        }
    });

    let mut stdout_buf = tokio::io::BufReader::new(stdout);
    let mut line = String::new();
    let mut stdout_bytes: Vec<u8> = Vec::new();

    loop {
        line.clear();
        let n = stdout_buf.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        reporter.push_line("stdout", trimmed);
        reporter.parse_and_publish(trimmed);
        stdout_bytes.extend_from_slice(line.as_bytes());
    }

    let _ = stderr_handle.await;
    let status = child.wait().await?;

    let stderr_final = Arc::try_unwrap(stderr_bytes)
        .map(|m| m.into_inner())
        .unwrap_or_else(|_| Vec::new());

    Ok(std::process::Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_final,
    })
}

/// Obtém a duração do input (segundos) via ffprobe. Retorna `None` se não for
/// possível determinar.
pub async fn probe_duration(ffprobe_path: &str, input: &Path) -> Result<Option<f64>> {
    use std::process::Stdio;
    let output = Command::new(ffprobe_path)
        .kill_on_drop(true)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(input)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let duration: f64 = stdout.trim().parse().unwrap_or(0.0);
    if duration > 0.0 {
        Ok(Some(duration))
    } else {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

fn parse_progress_line(line: &str) -> ProgressUpdate {
    let mut u = ProgressUpdate::default();
    if line.is_empty() {
        return u;
    }
    let (key, value) = match line.split_once('=') {
        Some(kv) => kv,
        None => return u,
    };
    let key = key.trim();
    let value = value.trim();
    match key {
        "out_time_us" => {
            u.out_time_us = value.parse().ok();
        }
        "out_time_ms" => {
            u.out_time_ms = value.parse().ok();
        }
        "out_time" => {
            u.out_time = Some(value.to_string());
        }
        "speed" => {
            let s = value.strip_suffix('x').unwrap_or(value);
            u.speed = s.parse().ok();
        }
        "progress" if value == "end" => {
            u.progress_end = true;
        }
        "progress" => {}
        _ => {}
    }
    u
}

/// Converte `HH:MM:SS.mmm` em segundos.
fn parse_hhmmss(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].parse().ok()?;
    let m: f64 = parts[1].parse().ok()?;
    let sec: f64 = parts[2].parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Limite do buffer de logs FFmpeg por job (watcher, file).
    const FFMPEG_LOG_MAX_LINES: usize = 200;
    const FFMPEG_LOG_MAX_BYTES: usize = 64 * 1024;

    /// Aplica os limites (200 linhas / 64 KiB) descartando as linhas mais antigas.
    fn trim_ffmpeg_log(lines: &mut Vec<String>) {
        while lines.len() > FFMPEG_LOG_MAX_LINES {
            lines.remove(0);
        }
        let mut total_bytes: usize = lines.iter().map(|l| l.len()).sum();
        while total_bytes > FFMPEG_LOG_MAX_BYTES && !lines.is_empty() {
            let removed = lines.remove(0);
            total_bytes -= removed.len();
        }
    }

    #[test]
    fn parse_out_time_us() {
        let u = parse_progress_line("out_time_us=1234567");
        assert_eq!(u.out_time_us, Some(1_234_567));
    }

    #[test]
    fn parse_out_time_ms() {
        let u = parse_progress_line("out_time_ms=1234");
        assert_eq!(u.out_time_ms, Some(1234));
    }

    #[test]
    fn parse_out_time_hhmmss() {
        let u = parse_progress_line("out_time=00:00:05.500");
        assert_eq!(u.out_time, Some("00:00:05.500".to_string()));
        assert_eq!(parse_hhmmss("00:00:05.5"), Some(5.5));
        assert_eq!(parse_hhmmss("00:01:00.0"), Some(60.0));
        assert_eq!(parse_hhmmss("1:00:00.0"), Some(3600.0));
        assert!(parse_hhmmss("not-a-time").is_none());
    }

    #[test]
    fn parse_speed() {
        let u = parse_progress_line("speed=0.5x");
        assert_eq!(u.speed, Some(0.5));
        let u = parse_progress_line("speed=2.00x");
        assert_eq!(u.speed, Some(2.0));
        let u = parse_progress_line("speed=1x");
        assert_eq!(u.speed, Some(1.0));
    }

    #[test]
    fn parse_progress_end() {
        let u = parse_progress_line("progress=end");
        assert!(u.progress_end);
    }

    #[test]
    fn percent_clamp() {
        let pct = |ot: f64, d: f64| (ot / d * 100.0).clamp(0.0, 100.0);
        assert!((pct(5.0, 10.0) - 50.0).abs() < 0.001);
        assert!((pct(10.0, 10.0) - 100.0).abs() < 0.001);
        assert!((pct(12.0, 10.0) - 100.0).abs() < 0.001); // clamp acima
        assert!((pct(0.0, 10.0) - 0.0).abs() < 0.001);
    }

    #[test]
    fn eta_calculation() {
        let eta = |ot: f64, d: f64, sp: f64| -> Option<f64> { Some((d - ot) / sp) };
        assert!((eta(0.0, 10.0, 1.0).unwrap() - 10.0).abs() < 0.001);
        assert!((eta(5.0, 10.0, 2.0).unwrap() - 2.5).abs() < 0.001);
    }

    #[test]
    fn trim_ffmpeg_log_discards_oldest() {
        let mut lines: Vec<String> = (0..250).map(|i| format!("line {i}")).collect();
        trim_ffmpeg_log(&mut lines);
        assert_eq!(lines.len(), FFMPEG_LOG_MAX_LINES);
        assert_eq!(lines[0], "line 50");
        assert_eq!(lines.last(), Some(&"line 249".to_string()));
    }

    #[test]
    fn trim_ffmpeg_log_respects_byte_limit() {
        let mut lines: Vec<String> = (0..100).map(|_| "x".repeat(1000)).collect(); // 100 KiB total
        trim_ffmpeg_log(&mut lines);
        let total: usize = lines.iter().map(|l| l.len()).sum();
        assert!(total <= FFMPEG_LOG_MAX_BYTES);
        assert!(!lines.is_empty());
    }
}
