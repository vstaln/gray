//! shell/pump.rs — bounded streaming pump: pipes → log file + memory + watch (brief 1C).
//!
//! Wired by 1D (`shell::pump`); types come from `super::contract`.
//! Contract deltas ruled in P1D-report.md (kept here too): `Pump::start`
//! takes `id: TaskId` first, `pattern` is the `NotifyPattern` substring
//! stub until 3C swaps in `regex::Regex`, `PumpSummary` carries
//! `log_write_failed`.
//! Design: two reader tasks (8 KiB reads, arrival-interleaved) → `mpsc<Vec<u8>>`
//! → one writer task owning the log file and the memory view. `start` returns the
//! writer's `JoinHandle`, which resolves when both pipes hit EOF. Readers end on
//! EOF or a real I/O error only — never on a transient one (`Interrupted` retries).

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::process::{ChildStderr, ChildStdout};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use super::contract::{
    MEM_HEAD_BYTES, MEM_TAIL_BYTES, NotifyPattern, PumpSummary, TaskId, WakeEvent,
};

const READ_BUF_BYTES: usize = 8 * 1024;
const PUMP_CHANNEL_CHUNKS: usize = 64;
const MAX_PATTERN_MATCHES: u8 = 5;
/// ≥10 s between wakes per task in release; 1 s under cfg(test) so the
/// rate-limit test stays fast (brief 3C blesses the shorter test interval).
#[cfg(test)]
const PATTERN_MIN_INTERVAL: Duration = Duration::from_secs(1);
#[cfg(not(test))]
const PATTERN_MIN_INTERVAL: Duration = Duration::from_secs(10);
const MAX_WAKE_LINE_CHARS: usize = 200;

/// Log line appended when the pattern fires for the 5th time (brief 1C wording).
pub const NOTIFY_DISABLED_NOTE: &str = "[gray: notify_on disabled after 5 matches]";

// Identity/event/budget types live in `super::contract` (mirrors deleted by
// 1D wiring; `NotifyPattern` stays there as the std-only stub until 3C).

// ── pure core (no I/O) ───────────────────────────────────────────────────────

/// Split `buf` into (complete-UTF-8 prefix, incomplete-tail carry). Only a
/// *truncated* sequence at the very end becomes carry (`error_len() == None`,
/// ≤3 bytes); bytes that are invalid mid-chunk stay in the prefix for lossy
/// decoding. Raw bytes still go to the log and memory verbatim — the carry
/// only feeds the sanitize/scan path.
fn split_complete_prefix(buf: &[u8]) -> (&[u8], &[u8]) {
    match std::str::from_utf8(buf) {
        Ok(_) => (buf, &[]),
        Err(e) if e.error_len().is_none() => buf.split_at(e.valid_up_to()),
        Err(_) => (buf, &[]),
    }
}

/// Sanitize decoded text for the pattern-scan path only: drop C0 controls
/// except `\t \n \r`, fold CRLF → LF (lone `\r` is kept, per brief 1B).
fn sanitize_text(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' && chars.peek() == Some(&'\n') {
            continue; // CRLF → LF: skip \r, the \n lands next iteration
        }
        if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') {
            continue;
        }
        out.push(c);
    }
    out
}

/// Drain every complete (`\n`-terminated) line from `pending`; the trailing
/// partial line stays buffered for the next chunk (or EOF).
fn take_complete_lines(pending: &mut String) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(pos) = pending.find('\n') {
        let mut line: String = pending.drain(..=pos).collect();
        line.pop(); // strip the '\n'
        lines.push(line);
    }
    lines
}

fn truncate_wake_line(line: &str) -> String {
    if line.chars().count() > MAX_WAKE_LINE_CHARS {
        line.chars().take(MAX_WAKE_LINE_CHARS).collect()
    } else {
        line.to_string()
    }
}

/// Bounded memory: first MEM_HEAD_BYTES verbatim + ring of last MEM_TAIL_BYTES.
/// Line counts come from raw bytes (`b'\n'`), never from decoded text.
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

/// Rate-limit state shared by both readers: ≥ PATTERN_MIN_INTERVAL between
/// wakes, at most MAX_PATTERN_MATCHES, then disabled for good.
struct PatternState {
    wakes: u8,
    last_wake: Option<Instant>,
    disabled: bool,
}

impl PatternState {
    fn new() -> Self {
        Self {
            wakes: 0,
            last_wake: None,
            disabled: false,
        }
    }

    fn should_wake(&self, now: Instant) -> bool {
        if self.disabled || self.wakes >= MAX_PATTERN_MATCHES {
            return false;
        }
        match self.last_wake {
            Some(t) if now.duration_since(t) < PATTERN_MIN_INTERVAL => false,
            _ => true,
        }
    }

    fn record_wake(&mut self, now: Instant) {
        self.wakes += 1;
        self.last_wake = Some(now);
        if self.wakes >= MAX_PATTERN_MATCHES {
            self.disabled = true;
        }
    }
}

// ── async pump ───────────────────────────────────────────────────────────────

/// Pattern-scan context shared by both readers. The `Mutex` is never held
/// across `.await` (lock → decide → drop → send).
struct ScanShared {
    id: TaskId,
    pattern: NotifyPattern,
    state: Mutex<PatternState>,
    wake: Option<broadcast::Sender<WakeEvent>>,
}

pub struct Pump;

impl Pump {
    /// Spawn the pump. Must be called from within a Tokio runtime (readers and
    /// writer are `tokio::spawn`ed); the handle resolves when both pipes hit EOF.
    pub fn start(
        id: TaskId, // DELTA vs contract.rs: source of PatternMatched{id,..}
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
        log_path: PathBuf,
        bytes_tx: watch::Sender<u64>,
        pattern: Option<NotifyPattern>, // STUB for Option<regex::Regex> until 3C
        wake: Option<broadcast::Sender<WakeEvent>>,
    ) -> JoinHandle<PumpSummary> {
        tokio::spawn(pump_main(id, stdout, stderr, log_path, bytes_tx, pattern, wake))
    }
}

async fn pump_main(
    id: TaskId,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    log_path: PathBuf,
    bytes_tx: watch::Sender<u64>,
    pattern: Option<NotifyPattern>,
    wake: Option<broadcast::Sender<WakeEvent>>,
) -> PumpSummary {
    let scan = pattern.map(|p| {
        Arc::new(ScanShared {
            id,
            pattern: p,
            state: Mutex::new(PatternState::new()),
            wake,
        })
    });

    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(PUMP_CHANNEL_CHUNKS);
    if let Some(pipe) = stdout {
        let (tx, scan) = (tx.clone(), scan.clone());
        tokio::spawn(async move { read_pipe(pipe, tx, scan).await });
    }
    if let Some(pipe) = stderr {
        let (tx, scan) = (tx.clone(), scan.clone());
        tokio::spawn(async move { read_pipe(pipe, tx, scan).await });
    }
    drop(tx); // writer now lives on the reader clones; ends at double EOF

    // Missing parents are created (`~/.gray/shell/<session>/`); anything that
    // fails here (or any later write) sets the flag — never panics.
    let mut log_failed = false;
    if let Some(parent) = log_path.parent() {
        if !parent.as_os_str().is_empty() && tokio::fs::create_dir_all(parent).await.is_err() {
            log_failed = true;
        }
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
    while let Some(chunk) = rx.recv().await {
        mem.push(&chunk);
        total += chunk.len() as u64;
        let flushed = match log.as_mut() {
            None => false,
            Some(f) => f.write_all(&chunk).await.is_ok() && f.flush().await.is_ok(),
        };
        if !flushed && !log_failed {
            log::warn!(
                "shell pump: log write failed for {}; memory view continues",
                log_path.display()
            );
            log_failed = true;
            log = None;
        }
        // Sent after every batch even when the log is gone, so wait="output"
        // keeps observing stream progress off the memory count. Ignored when
        // there are zero receivers (normal).
        let _ = bytes_tx.send(total);
    }
    if let Some(f) = log.as_mut() {
        if f.flush().await.is_err() {
            log_failed = true;
        }
    }
    mem.into_summary(log_failed)
}

/// One reader task per pipe: forward raw bytes unchanged, and — only when a
/// pattern is configured — decode (with UTF-8 carry across chunks), sanitize,
/// split into complete lines, and scan.
async fn read_pipe<R>(pipe: R, tx: mpsc::Sender<Vec<u8>>, scan: Option<Arc<ScanShared>>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut pipe = pipe;
    let mut buf = [0u8; READ_BUF_BYTES];
    let mut carry: Vec<u8> = Vec::new();
    let mut pending = String::new();
    loop {
        match pipe.read(&mut buf).await {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) | Ok(0) => break, // EOF or a real I/O error ends this reader only
            Ok(n) => {
                let raw = &buf[..n];
                if tx.send(raw.to_vec()).await.is_err() {
                    break; // writer gone; nothing left to do
                }
                if let Some(shared) = scan.as_ref() {
                    feed_scan(&mut carry, &mut pending, raw, shared, &tx).await;
                }
            }
        }
    }
    // EOF: a trailing partial line still counts as a complete line.
    if let Some(shared) = scan.as_ref() {
        if !carry.is_empty() || !pending.is_empty() {
            pending.push_str(&sanitize_text(&String::from_utf8_lossy(&carry)));
            if !pending.is_empty() {
                let line = std::mem::take(&mut pending);
                scan_line(&line, shared, &tx).await;
            }
        }
    }
}

async fn feed_scan(
    carry: &mut Vec<u8>,
    pending: &mut String,
    raw: &[u8],
    shared: &Arc<ScanShared>,
    tx: &mpsc::Sender<Vec<u8>>,
) {
    let chunk: Cow<'_, [u8]> = if carry.is_empty() {
        Cow::Borrowed(raw)
    } else {
        let mut joined = std::mem::take(carry);
        joined.extend_from_slice(raw);
        Cow::Owned(joined)
    };
    let (complete, rest) = split_complete_prefix(&chunk);
    let rest_vec = rest.to_vec();
    pending.push_str(&sanitize_text(&String::from_utf8_lossy(complete)));
    *carry = rest_vec;
    for line in take_complete_lines(pending) {
        scan_line(&line, shared, tx).await;
    }
}

async fn scan_line(line: &str, shared: &Arc<ScanShared>, tx: &mpsc::Sender<Vec<u8>>) {
    if !shared.pattern.matches(line) {
        return;
    }
    let now = Instant::now();
    let disabled_by_this = {
        let mut state = shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.should_wake(now) {
            return;
        }
        state.record_wake(now);
        state.disabled
    };
    if let Some(wake) = shared.wake.as_ref() {
        // Ignored when there are zero receivers (normal outside a live session).
        let _ = wake.send(WakeEvent::PatternMatched {
            id: shared.id,
            line: truncate_wake_line(line),
        });
    }
    if disabled_by_this {
        let mut note = String::from(NOTIFY_DISABLED_NOTE);
        note.push('\n');
        // Into the log via the writer like any other bytes; loss is fine.
        let _ = tx.send(note.into_bytes()).await;
    }
}

// ── in-file tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::process::Command;

    /// Inline POSIX script with both pipes captured; the caller takes what it needs.
    fn spawn_sh(script: &str) -> (tokio::process::Child, Option<ChildStdout>, Option<ChildStderr>) {
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
    fn split_prefix_holds_truncated_tail_only() {
        assert_eq!(split_complete_prefix(b"hello"), (b"hello".as_slice(), b"".as_slice()));
        // U+1F600 is 4 bytes; any truncation at the end becomes carry.
        let emoji = "🎉".as_bytes();
        for cut in 1..4 {
            let (head, tail) = split_complete_prefix(&emoji[..cut]);
            assert_eq!(head, b"".as_slice());
            assert_eq!(tail, &emoji[..cut]);
        }
        let (head, tail) = split_complete_prefix(emoji);
        assert_eq!(head, emoji);
        assert!(tail.is_empty());
        // Invalid bytes mid-chunk are not carry (decoded lossy later).
        let (head, tail) = split_complete_prefix(b"a\xff");
        assert_eq!(head, b"a\xff".as_slice());
        assert!(tail.is_empty());
    }

    #[test]
    fn sanitize_drops_c0_except_tab_lf_cr_and_folds_crlf() {
        // 1D wave-test fix: the old expectation ate the legitimate \n after
        // "c". Brief 1C keeps \n (drops NUL, folds CRLF, keeps lone \r).
        assert_eq!(sanitize_text("a\x00b\tc\nd\re\r\nf"), "ab\tc\nd\re\nf");
        assert_eq!(sanitize_text("\x1b[31mred\x07"), "[31mred"); // ESC + BEL dropped
        assert_eq!(sanitize_text("\x7f"), "\x7f"); // DEL is out of the 0x00–0x1f brief range: kept
    }

    #[test]
    fn complete_lines_leave_partial_buffered() {
        let mut pending = String::from("a\nb\npartial");
        assert_eq!(take_complete_lines(&mut pending), vec!["a", "b"]);
        assert_eq!(pending, "partial");
        assert!(take_complete_lines(&mut pending).is_empty());
    }

    #[test]
    fn mem_view_bounds_head_and_tail() {
        // 5 MiB in 8 KiB pushes: letters + '\n' every 100th byte.
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
        assert_eq!(mem.total_lines, data.iter().filter(|&&b| b == b'\n').count());
        assert_eq!(mem.head.len(), MEM_HEAD_BYTES);
        assert_eq!(mem.head, &data[..MEM_HEAD_BYTES]);
        assert_eq!(mem.tail.len(), MEM_TAIL_BYTES);
        assert_eq!(
            mem.tail.iter().copied().collect::<Vec<u8>>(),
            &data[total - MEM_TAIL_BYTES..]
        );
        assert!(mem.head.len() + mem.tail.len() <= 64 * 1024);
    }

    #[test]
    fn pattern_state_rate_limits_and_disables_after_five() {
        let mut st = PatternState::new();
        let t0 = Instant::now();
        // 20 rapid matches → exactly one wake.
        assert!(st.should_wake(t0));
        st.record_wake(t0);
        let extra = (0..19)
            .filter(|_| {
                if st.should_wake(t0) {
                    st.record_wake(t0);
                    true
                } else {
                    false
                }
            })
            .count();
        assert_eq!(extra, 0);
        // Spaced matches → 4 more wakes, then disabled for good.
        let mut t = t0 + PATTERN_MIN_INTERVAL;
        for _ in 0..4 {
            assert!(st.should_wake(t));
            st.record_wake(t);
            t += PATTERN_MIN_INTERVAL;
        }
        assert!(st.disabled);
        assert!(!st.should_wake(t + Duration::from_secs(3600)));
    }

    #[test]
    fn notify_stub_matches_substring_and_truncates_wake_lines() {
        let p = NotifyPattern::new("ready");
        assert!(p.matches("server ready on :8080"));
        assert!(!p.matches("server started"));
        assert!(!NotifyPattern::new("").matches("ready")); // empty never matches
        let long = "x".repeat(300);
        assert_eq!(truncate_wake_line(&long).chars().count(), 200);
        assert_eq!(truncate_wake_line("short"), "short");
    }

    #[tokio::test]
    async fn pump_logs_both_streams_verbatim_and_reports_watch() {
        let (mut child, out, err) =
            spawn_sh("printf 'out-1\\n'; printf 'err-1\\n' >&2; printf 'out-2\\n'");
        let log = tmp_log("both");
        let (bytes_tx, bytes_rx) = watch::channel(0u64);
        assert_eq!(*bytes_rx.borrow(), 0);
        let h = Pump::start(TaskId(1), out, err, log.clone(), bytes_tx, None, None);
        assert!(child.wait().await.unwrap().success());
        let s = h.await.unwrap();
        assert_eq!(s.total_bytes, 18);
        assert_eq!(s.total_lines, 3);
        assert!(!s.log_write_failed);
        let file = std::fs::read(&log).unwrap();
        assert_eq!(file.len() as u64, s.total_bytes);
        assert_eq!(*bytes_rx.borrow(), file.len() as u64); // watch final == file length
        for marker in [b"out-1\n", b"err-1\n", b"out-2\n"] {
            assert!(file.windows(marker.len()).any(|w| w == marker), "log holds {marker:?}");
        }
        assert_eq!(s.head, file); // under budget: head is the whole stream
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn pump_scan_holds_emoji_carry_and_wakes_once_with_id() {
        let (tx, mut rx) = mpsc::channel(8);
        let (wake_tx, mut wake_rx) = broadcast::channel(8);
        let shared = Arc::new(ScanShared {
            id: TaskId(7),
            pattern: NotifyPattern::new("ready"),
            state: Mutex::new(PatternState::new()),
            wake: Some(wake_tx),
        });
        // Feed the 4-byte emoji split across two chunks, then the match word.
        let emoji = "🎉".as_bytes();
        let mut c1 = b"server ".to_vec();
        c1.extend_from_slice(&emoji[..2]);
        let mut c2 = emoji[2..].to_vec();
        c2.extend_from_slice(b" ready\n");
        let mut carry = Vec::new();
        let mut pending = String::new();
        feed_scan(&mut carry, &mut pending, &c1, &shared, &tx).await;
        // 1D wave-test fix: the partial line "server " is correctly buffered
        // (complete-lines-only scan) — pending holds it, not empty.
        assert_eq!(pending, "server "); // no complete line yet
        feed_scan(&mut carry, &mut pending, &c2, &shared, &tx).await;
        drop(tx);
        let mut fwd = Vec::new();
        while let Some(b) = rx.recv().await {
            fwd.extend_from_slice(&b);
        }
        // 1D wave-test fix: feed_scan never forwards raw bytes — read_pipe
        // does that (verified by the e2e verbatim-log test). Here only the
        // scan runs, and this line doesn't disable the pattern, so tx sees
        // nothing. The carry is proven by the intact emoji in the wake line.
        assert!(fwd.is_empty());
        match wake_rx.recv().await.unwrap() {
            WakeEvent::PatternMatched { id, line } => {
                assert_eq!(id, TaskId(7));
                assert_eq!(line, "server 🎉 ready"); // emoji intact, not split
            }
            ev => panic!("expected PatternMatched, got {ev:?}"),
        }
    }

    #[tokio::test]
    async fn pump_end_to_end_pattern_wake_carries_id() {
        let (mut child, out, err) = spawn_sh("printf 'server starting\\nserver ready on :8080\\n'");
        drop(err);
        let log = tmp_log("wake");
        let (bytes_tx, _rx) = watch::channel(0u64);
        let (wake_tx, mut wake_rx) = broadcast::channel(8);
        let h = Pump::start(
            TaskId(4),
            out,
            None,
            log.clone(),
            bytes_tx,
            Some(NotifyPattern::new("ready")),
            Some(wake_tx),
        );
        assert!(child.wait().await.unwrap().success());
        match tokio::time::timeout(Duration::from_secs(5), wake_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            WakeEvent::PatternMatched { id, line } => {
                assert_eq!(id, TaskId(4));
                assert_eq!(line, "server ready on :8080");
            }
            ev => panic!("expected PatternMatched, got {ev:?}"),
        }
        let s = h.await.unwrap();
        assert_eq!(s.total_lines, 2);
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn pump_pattern_disables_after_five_and_notes_the_log() {
        // Spacing comes from the script itself (test rate limit is 1 s; release is 10 s).
        let (mut child, out, err) =
            spawn_sh("for i in 1 2 3 4 5 6; do echo tick; sleep 1.2; done");
        drop(err);
        let log = tmp_log("disable");
        let (bytes_tx, _rx) = watch::channel(0u64);
        let (wake_tx, mut wake_rx) = broadcast::channel(16);
        let h = Pump::start(
            TaskId(3),
            out,
            None,
            log.clone(),
            bytes_tx,
            Some(NotifyPattern::new("tick")),
            Some(wake_tx),
        );
        for _ in 0..5 {
            let ev = tokio::time::timeout(Duration::from_secs(5), wake_rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(ev, WakeEvent::PatternMatched { .. }));
        }
        assert!(child.wait().await.unwrap().success()); // 6th tick ran, then EOF
        let s = h.await.unwrap();
        assert!(wake_rx.try_recv().is_err()); // 6th match suppressed: still exactly 5 wakes
        let file = std::fs::read(&log).unwrap();
        assert!(String::from_utf8_lossy(&file).contains(NOTIFY_DISABLED_NOTE));
        assert_eq!(s.total_bytes, file.len() as u64);
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn pump_unwritable_log_sets_flag_without_panic() {
        let blocker = tmp_log("blocker");
        std::fs::write(&blocker, b"x").unwrap();
        let bad = blocker.join("t1.log"); // parent is a file: mkdir + open both fail
        let (mut child, out, _err) = spawn_sh("printf 'still counted\\n'");
        let (bytes_tx, _rx) = watch::channel(0u64);
        let h = Pump::start(TaskId(9), out, None, bad, bytes_tx, None, None);
        assert!(child.wait().await.unwrap().success());
        let s = h.await.unwrap();
        assert!(s.log_write_failed);
        assert_eq!(s.total_bytes, 14); // memory view stays alive
        assert_eq!(s.head, b"still counted\n");
        let _ = std::fs::remove_file(&blocker);
    }
}
