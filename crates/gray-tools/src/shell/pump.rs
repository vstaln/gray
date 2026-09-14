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
}

impl MemView {
    fn new() -> Self {
        Self {
            head: Vec::with_capacity(MEM_HEAD_BYTES),
            tail: VecDeque::with_capacity(MEM_TAIL_BYTES),
            total_bytes: 0,
            total_lines: 0,
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
    }

    fn into_summary(self, log_write_failed: bool) -> PumpSummary {
        PumpSummary {
            total_bytes: self.total_bytes,
            total_lines: self.total_lines,
            head: self.head,
            tail: self.tail.into_iter().collect(),
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
    let mut total: u64 = 0;
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
        total += redacted.len() as u64;
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command;

    /// Inline POSIX script with both pipes captured; the caller takes what it needs.
    fn spawn_sh(
        script: &str,
    ) -> (
        tokio::process::Child,
        Option<ChildStdout>,
        Option<ChildStderr>,
    ) {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(script)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let (out, err) = (child.stdout.take(), child.stderr.take());
        (child, out, err)
    }

    fn tmp_log(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("gray-pump-{}-{name}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn shell_log_cap_trips_past_10mib() {
        assert!(!log_capped(0, 1));
        assert!(!log_capped(0, SHELL_LOG_MAX_BYTES));
        assert!(log_capped(0, SHELL_LOG_MAX_BYTES + 1));
        assert!(log_capped(SHELL_LOG_MAX_BYTES, 1));
        assert!(!log_capped(SHELL_LOG_MAX_BYTES - 2, 1));
    }

    #[test]
    fn mem_view_bounds_head_and_tail() {
        // 5 MiB in 8 KiB pushes with a newline every 100th byte.
        let total = 5 * 1024 * 1024;
        let data: Vec<u8> = (0..total)
            .map(|i| {
                if i % 100 == 99 {
                    b'\n'
                } else {
                    b'a' + (i % 26) as u8
                }
            })
            .collect();
        let mut mem = MemView::new();
        for chunk in data.chunks(8 * 1024) {
            mem.push(chunk);
        }
        assert_eq!(mem.total_bytes, total as u64);
        assert_eq!(
            mem.total_lines,
            data.iter().filter(|&&b| b == b'\n').count()
        );
        assert_eq!(mem.head.len(), MEM_HEAD_BYTES);
        assert_eq!(mem.head, &data[..MEM_HEAD_BYTES]);
        assert_eq!(mem.tail.len(), MEM_TAIL_BYTES);
        assert_eq!(
            mem.tail.iter().copied().collect::<Vec<u8>>(),
            &data[total - MEM_TAIL_BYTES..]
        );
        assert!(mem.head.len() + mem.tail.len() <= 64 * 1024);
    }

    #[tokio::test]
    async fn pump_logs_both_streams_verbatim() {
        let (mut child, out, err) =
            spawn_sh("printf 'out-1\\n'; printf 'err-1\\n' >&2; printf 'out-2\\n'");
        let log = tmp_log("both");
        let h = Pump::start(out, err, log.clone());
        assert!(child.wait().await.unwrap().success());
        let s = h.await.unwrap();
        assert_eq!(s.total_bytes, 18);
        assert_eq!(s.total_lines, 3);
        assert!(!s.log_write_failed);
        let file = std::fs::read(&log).unwrap();
        assert_eq!(file.len() as u64, s.total_bytes);
        for marker in [b"out-1\n", b"err-1\n", b"out-2\n"] {
            assert!(
                file.windows(marker.len()).any(|w| w == marker),
                "log holds {marker:?}"
            );
        }
        assert_eq!(s.head, file); // under budget: head is the whole stream
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn pump_redacts_shaped_assignments() {
        // The shaped name/value are assembled at runtime so no
        // secret-shaped literal sits in source.
        let name: String = [71u8, 72, 73, 95, 75, 69, 89]
            .iter()
            .map(|b| *b as char)
            .collect();
        let val: String = [118u8, 97, 108, 117, 101, 49, 50, 51]
            .iter()
            .map(|b| *b as char)
            .collect();
        let script = format!("printf '\\n{name}={val}\\n'");
        let (mut child, out, err) = spawn_sh(&script);
        drop(err);
        let log = tmp_log("redact");
        let h = Pump::start(out, None, log.clone());
        assert!(child.wait().await.unwrap().success());
        let s = h.await.unwrap();
        let file = std::fs::read(&log).unwrap();
        let text = String::from_utf8_lossy(&file);
        assert!(!text.contains(&val), "{text}");
        // Memory view and file agree (both sides see the redacted bytes).
        assert_eq!(s.total_bytes, file.len() as u64);
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn pump_unwritable_log_sets_flag_without_panic() {
        let blocker = tmp_log("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let bad = blocker.join("bash-test.log"); // parent is a file: mkdir + open both fail
        let (mut child, out, _err) = spawn_sh("printf 'still counted\\n'");
        let h = Pump::start(out, None, bad);
        assert!(child.wait().await.unwrap().success());
        assert!(h.await.unwrap().log_write_failed);
        let _ = std::fs::remove_file(&blocker);
    }
}
