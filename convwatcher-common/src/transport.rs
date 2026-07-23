//! Length-prefixed framing over an async byte stream.
//!
//! Two frame kinds share the same wire shape — a 4-byte big-endian length
//! followed by that many bytes:
//!   * control frames carry a JSON-encoded [`Message`];
//!   * bulk frames carry raw file bytes (input/output streams).
//!
//! The reader/writer split lets the server and agent hold a reader and writer
//! half independently (e.g. read jobs while writing heartbeats).

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::protocol::Message;

/// Maximum size of a single frame (control or bulk chunk). Guards against a
/// malformed/hostile peer trying to allocate huge buffers. Bulk file transfers
/// are split into chunks below this size.
pub const MAX_FRAME_LEN: u32 = 8 * 1024 * 1024; // 8 MiB

/// Size of each bulk data chunk when streaming file bytes.
pub const CHUNK_SIZE: usize = 256 * 1024; // 256 KiB

/// Write a single length-prefixed frame.
pub async fn write_frame<W>(w: &mut W, data: &[u8]) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    if data.len() as u64 > MAX_FRAME_LEN as u64 {
        bail!("frame too large: {} bytes", data.len());
    }
    w.write_all(&(data.len() as u32).to_be_bytes())
        .await
        .context("write frame length")?;
    w.write_all(data).await.context("write frame body")?;
    Ok(())
}

/// Read a single length-prefixed frame into a freshly allocated `Vec`.
pub async fn read_frame<R>(r: &mut R) -> Result<Vec<u8>>
where
    R: AsyncReadExt + Unpin,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)
        .await
        .context("read frame length")?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        bail!("frame length {} exceeds max {}", len, MAX_FRAME_LEN);
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await.context("read frame body")?;
    Ok(buf)
}

/// Serialize and write a control [`Message`].
pub async fn write_message<W>(w: &mut W, msg: &Message) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let bytes = serde_json::to_vec(msg).context("serialize message")?;
    write_frame(w, &bytes).await
}

/// Read and deserialize a control [`Message`].
pub async fn read_message<R>(r: &mut R) -> Result<Message>
where
    R: AsyncReadExt + Unpin,
{
    let bytes = read_frame(r).await?;
    let msg = serde_json::from_slice(&bytes).context("deserialize message")?;
    Ok(msg)
}

/// Stream `total` bytes from `src` to `w` as a sequence of bulk frames.
/// Returns an error if `src` yields fewer than `total` bytes.
pub async fn write_stream<W, R>(w: &mut W, src: &mut R, total: u64) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    R: AsyncReadExt + Unpin,
{
    let mut remaining = total;
    let mut buf = vec![0u8; CHUNK_SIZE];
    while remaining > 0 {
        let want = remaining.min(CHUNK_SIZE as u64) as usize;
        let n = src.read(&mut buf[..want]).await.context("read source")?;
        if n == 0 {
            bail!("source ended early: {} bytes still expected", remaining);
        }
        write_frame(w, &buf[..n]).await?;
        remaining -= n as u64;
    }
    Ok(())
}

/// Read exactly `total` bytes worth of bulk frames from `r` and write them to
/// `dst`.
pub async fn read_stream<R, W>(r: &mut R, dst: &mut W, total: u64) -> Result<()>
where
    R: AsyncReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    let mut remaining = total;
    while remaining > 0 {
        let chunk = read_frame(r).await?;
        if chunk.is_empty() {
            bail!("received empty chunk with {} bytes remaining", remaining);
        }
        if chunk.len() as u64 > remaining {
            bail!(
                "chunk overshoot: got {} bytes, only {} expected",
                chunk.len(),
                remaining
            );
        }
        dst.write_all(&chunk).await.context("write destination")?;
        remaining -= chunk.len() as u64;
    }
    dst.flush().await.ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn frame_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello world").await.unwrap();
        let mut cur = Cursor::new(buf);
        let got = read_frame(&mut cur).await.unwrap();
        assert_eq!(got, b"hello world");
    }

    #[tokio::test]
    async fn message_round_trip() {
        let mut buf = Vec::new();
        write_message(&mut buf, &Message::Heartbeat).await.unwrap();
        let mut cur = Cursor::new(buf);
        let got = read_message(&mut cur).await.unwrap();
        assert!(matches!(got, Message::Heartbeat));
    }

    #[tokio::test]
    async fn stream_round_trip() {
        let data: Vec<u8> = (0..CHUNK_SIZE * 2 + 123).map(|i| (i % 251) as u8).collect();
        let mut wire = Vec::new();
        {
            let mut src = Cursor::new(data.clone());
            write_stream(&mut wire, &mut src, data.len() as u64)
                .await
                .unwrap();
        }
        let mut cur = Cursor::new(wire);
        let mut out = Vec::new();
        read_stream(&mut cur, &mut out, data.len() as u64)
            .await
            .unwrap();
        assert_eq!(out, data);
    }
}
