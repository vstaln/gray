//! shell/pump.rs: bounded streaming pump: pipes to log file + memory.
//!
//! Two reader tasks (8 KiB reads, arrival-interleaved) send chunks over
//! an mpsc channel to one writer task owning the log file and the memory
//! view. `start` returns the writer JoinHandle, which resolves when both
//! pipes hit EOF. Readers end on EOF or a real I/O error only: never on a
//! transient one (`Interrupted` retries).

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::process::{ChildStderr, ChildStdout};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::contract::{MEM_HEAD_BYTES, MEM_TAIL_BYTES, PumpSummary};

const READ_BUF_BYTES: usize = 8 * 1024;
const PUMP_CHANNEL_CHUNKS: usize = 64;
/// Shell transcript cap per log file: matches gray.log 10MiB. The 7-day
/// sweep alone let real usage reach 208MB; the pump stops file writes past
/// this (memory view continues), the sweep catches pre-cap files.
pub const SHELL_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// Pure size check (unit-testable): stop file writes once the next chunk
/// would push past the cap.
fn log_capped(file_bytes: u64, incoming: u64) -> bool {
    file_bytes.saturating_add(incoming) > SHELL_LOG_MAX_BYTES
}

// pure core (no I/O)

struct MemView {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total_bytes: u64,
    total_lines: usize,
    has_cr: bool,
}

impl MemView {
    fn new() -> Self {
        Self {
            head: Vec::with_capacity(MEM_HEAD_BYTES),
            tail: VecDeque::with_capacity(MEM_TAIL_BYTES),
            total_bytes: 0,
            total_lines: 0,
            has_cr: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        if self.head.len() < MEM_HEAD_BYTES {
            let take = (MEM_HEAD_BYTES - self.head.len()).min(chunk.len());
            self.head.extend_from_slice(&chunk[..take]);
        }
        self.tail.extend(chunk.iter().copied());
        let excess = self.tail.len().saturating_sub(MEM_TAIL_BYTES);
        if excess > 0 {
            // ponytail: VecDeque drain is O(excess); never Vec::remove(0).
            self.tail.drain(..excess);
        }
        self.total_bytes += chunk.len() as u64;
        self.total_lines += chunk.iter().filter(|&&b| b == b'\n').count();
        self.has_cr = self.has_cr || chunk.contains(&b'\r');
    }

    fn into_summary(self, log_write_failed: bool) -> PumpSummary {
        PumpSummary {
            total_bytes: self.total_bytes,
            total_lines: self.total_lines,
            head: self.head,
            tail: self.tail.into_iter().collect(),
            has_cr: self.has_cr,
            log_write_failed,
        }
    }
}

// async pump

pub struct Pump;

impl Pump {
    /// Spawn the pump. Must be called from within a Tokio runtime (readers and
    /// writer are `tokio::spawn`ed); the handle resolves when both pipes hit EOF.
    pub fn start(
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
        log_path: PathBuf,
    ) -> JoinHandle<PumpSummary> {
        tokio::spawn(pump_main(stdout, stderr, log_path))
    }
}

async fn pump_main(
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    log_path: PathBuf,
) -> PumpSummary {
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(PUMP_CHANNEL_CHUNKS);
    if let Some(pipe) = stdout {
        let tx = tx.clone();
        tokio::spawn(async move { read_pipe(pipe, tx).await });
    }
    if let Some(pipe) = stderr {
        let tx = tx.clone();
        tokio::spawn(async move { read_pipe(pipe, tx).await });
    }
    drop(tx); // writer now lives on the reader clones; ends at double EOF

    // Missing parents are created; anything that fails here (or any later
    // write) sets the flag: never panics.
    let mut log_failed = false;
    if let Some(parent) = log_path.parent()
        && !parent.as_os_str().is_empty()
        && tokio::fs::create_dir_all(parent).await.is_err()
    {
        log_failed = true;
    }
    let mut log: Option<BufWriter<tokio::fs::File>> = if log_failed {
        None
    } else {
        match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true) // O_APPEND
            .open(&log_path)
            .await
        {
            Ok(f) => Some(BufWriter::new(f)),
            Err(e) => {
                log::warn!("shell pump: cannot open log {}: {e}", log_path.display());
                log_failed = true;
                None
            }
        }
    };

    let mut mem = MemView::new();
    let mut log_bytes: u64 = tokio::fs::metadata(&log_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    let mut log_truncated = false;
    while let Some(chunk) = rx.recv().await {
        // Durable transcript: secret-shaped text is redacted before the
        // log write. One redaction serves both sides (memory + file) so
        // byte counts stay consistent; binary chunks pass through untouched.
        let redacted = gray_core::redaction::redact_bytes_for_log(&chunk);
        mem.push(&redacted);
        // Size cap: one truncation note, then file writes stop (memory
        // view continues). Not a failure: `log_write_failed` stays false.
        if !log_truncated && log.is_some() && log_capped(log_bytes, redacted.len() as u64) {
            if let Some(f) = log.as_mut() {
                let _ = f
                    .write_all(b"\n[gray: shell log truncated at 10MiB; memory view continues]\n")
                    .await;
                let _ = f.flush().await;
            }
            log = None;
            log_truncated = true;
            continue;
        }
        let flushed = match log.as_mut() {
            None => false,
            Some(f) => {
                let ok = f.write_all(&redacted).await.is_ok() && f.flush().await.is_ok();
                if ok {
                    log_bytes += redacted.len() as u64;
                }
                ok
            }
        };
        if !flushed && !log_failed && !log_truncated && log.is_some() {
            log::warn!(
                "shell pump: log write failed for {}; memory view continues",
                log_path.display()
            );
            log_failed = true;
            log = None;
        }
    }
    if let Some(f) = log.as_mut()
        && f.flush().await.is_err()
    {
        log_failed = true;
    }
    mem.into_summary(log_failed)
}

/// One reader task per pipe: forward raw bytes unchanged.
async fn read_pipe<R>(pipe: R, tx: mpsc::Sender<Vec<u8>>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut pipe = pipe;
    let mut buf = [0u8; READ_BUF_BYTES];
    loop {
        match pipe.read(&mut buf).await {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) | Ok(0) => break, // EOF or a real I/O error ends this reader only
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).await.is_err() {
                    break; // writer gone; nothing left to do
                }
            }
        }
    }
}

// ── in-file tests ────────────────────────────────────────────────────────────

#[path = "pump_tests.rs"]
#[cfg(test)]
mod tests;
