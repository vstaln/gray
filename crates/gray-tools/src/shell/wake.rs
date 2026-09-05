//! shell/wake.rs — wake-up message formatting (brief 3A, Phase 3).
//!
//! Pure formatting over a [`WakeEvent`] + [`TaskInfo`] snapshot. The event
//! drain itself (subscribe → steer-or-inject → coalesce) lives in the REPL
//! crate and is a later phase — this module is the formatting + filter seam
//! it calls. See the seam notes below.
//!
//! Seam for the drain owner (gray/src, later phase):
//! - subscribe `registry().wake_tx().subscribe()` once per session; the
//!   registry broadcasts globally, so filter events to this session's ids.
//! - call [`should_inject`] first: `UserInput` only wakes `sleep` (3B).
//! - turn running → `agent.steer(format_wake(&ev, &info))`; idle at the
//!   prompt with config `shell.wake_on_exit` (default true; false in `-p`
//!   and gateway unless configured) → synthetic user message + turn, shown
//!   as a system notice, not as if the user typed it.
//! - coalesce events within 500 ms into one multi-line message;
//!   `RecvError::Lagged(n)` → one line `"[shell] {n} task events were
//!   dropped; shell_output() to list"`; never inject during compaction
//!   (queue and flush after).
//!
//! Contract deltas vs brief 3A (contract frozen — no silent edits):
//! - `PatternMatched` carries no pattern expr and no match offset, so the
//!   message names no pattern and uses `info.bytes` (log end at snapshot)
//!   as the offset, `from_offset = bytes.saturating_sub(1024)`. 3C
//!   follow-up: put the expr + match offset in the event (orchestrator
//!   approves the contract change).
//! - non-zero exits read the last 1 KiB from `info.log_path` best-effort;
//!   a missing/unreadable/empty log falls back to the exit-0 shape (header
//!   + log path) so the message is never empty.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::contract::{TaskInfo, TaskState, WakeEvent};
use super::fence::fence;
use super::view::{format_elapsed, home_relative, middle_out};

/// Tail bytes included after a non-zero exit.
const WAKE_TAIL_BYTES: usize = 1024;
/// Pattern lines are capped (the pump already sends the first 200 chars).
const WAKE_LINE_CAP: usize = 200;

/// Filter seam for the drain: `UserInput` only wakes `sleep` (brief 3B) and
/// must never start a turn.
pub fn should_inject(ev: &WakeEvent) -> bool {
    !matches!(ev, WakeEvent::UserInput)
}

/// One wake-up line for an event. Reads fine appended to a tool result or
/// as a standalone message (the drain_steer fallback).
pub fn format_wake(ev: &WakeEvent, info: &TaskInfo) -> String {
    match ev {
        WakeEvent::Exited { id, report } => {
            let at = match &info.state {
                TaskState::Exited { at, .. } => *at,
                TaskState::Running => std::time::Instant::now(),
            };
            let when = format_elapsed(at.saturating_duration_since(info.started));
            if report.effective == 0 {
                format!(
                    "[shell] {id} (`{}`) exited 0 after {when}. shell_output(task_id=\"{id}\") for the last lines; log {}",
                    info.command,
                    home_relative(&info.log_path),
                )
            } else {
                let tail = wake_tail(&info.log_path);
                if tail.is_empty() {
                    format!(
                        "[shell] {id} (`{}`) {} after {when}. shell_output(task_id=\"{id}\") for the last lines; log {}",
                        info.command,
                        report.label,
                        home_relative(&info.log_path),
                    )
                } else {
                    format!(
                        "[shell] {id} (`{}`) {} after {when} — last lines:\n{}",
                        info.command,
                        report.label,
                        fence(*id, &tail),
                    )
                }
            }
        }
        WakeEvent::PatternMatched { id, line } => {
            let end = info.bytes;
            let from = end.saturating_sub(WAKE_TAIL_BYTES as u64);
            format!(
                "[shell] {id} matched: \"{}\" (offset {end}). shell_output(task_id=\"{id}\", from_offset={from})",
                truncate_chars(line, WAKE_LINE_CAP),
            )
        }
        WakeEvent::UserInput => "[shell] user input received".to_string(),
    }
}

/// Last 1 KiB of the log, sanitized via `view::middle_out` (the read is
/// already capped and 1 KiB never exceeds its line budget, so the body is
/// the whole sanitized tail). Empty when the log is missing, unreadable,
/// or blank.
fn wake_tail(path: &Path) -> String {
    let raw = read_last(path, WAKE_TAIL_BYTES);
    if raw.is_empty() {
        return String::new();
    }
    middle_out(&raw, WAKE_TAIL_BYTES, 2048, 0)
        .body
        .trim_end_matches(&['\n', '\r'][..])
        .to_string()
}

/// Last `max` bytes of a file, starting on a line boundary unless the cut
/// lands inside one huge line (then the byte-cut is kept). Empty on any
/// I/O failure — wake text must never fail.
fn read_last(path: &Path, max: usize) -> Vec<u8> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let len = match f.metadata() {
        Ok(m) => m.len(),
        Err(_) => return Vec::new(),
    };
    let want = len.min(max as u64);
    if want == 0 || f.seek(SeekFrom::End(-(want as i64))).is_err() {
        return Vec::new();
    }
    let mut raw = Vec::with_capacity(want as usize);
    if f.take(want).read_to_end(&mut raw).is_err() {
        return Vec::new();
    }
    if want < len
        && let Some(nl) = raw.iter().position(|&b| b == b'\n')
        && nl + 1 < raw.len()
    {
        raw.drain(..=nl);
    }
    raw
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::contract::{ExitReport, TaskId};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    static N: AtomicU64 = AtomicU64::new(0);

    fn tmp_log(body: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "gray-wake-{}-{}.log",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&p, body).expect("write tmp log");
        p
    }

    fn report(effective: i32, label: &str) -> ExitReport {
        ExitReport {
            code: Some(effective),
            signal: None,
            effective,
            label: label.into(),
            note: None,
            benign: false,
        }
    }

    /// Elapsed is exactly `elapsed` (at - started), so `when` is deterministic.
    fn exited_info(cmd: &str, elapsed: Duration, log: &Path, bytes: u64) -> TaskInfo {
        let started = Instant::now() - elapsed;
        TaskInfo {
            id: TaskId(5),
            pid: 4242,
            pgid: 4242,
            command: cmd.into(),
            started,
            log_path: log.to_path_buf(),
            bytes,
            state: TaskState::Exited {
                report: report(0, "exit 0"),
                at: started + elapsed,
            },
        }
    }

    #[test]
    fn exited_zero_golden() {
        let log = tmp_log(b"built\n");
        let info = exited_info("cargo build --release", Duration::from_secs(192), &log, 6);
        let ev = WakeEvent::Exited {
            id: TaskId(5),
            report: report(0, "exit 0"),
        };
        let m = format_wake(&ev, &info);
        assert!(
            m.starts_with("[shell] t5 (`cargo build --release`) exited 0 after 3m12s. "),
            "{m}"
        );
        assert!(m.contains("shell_output(task_id=\"t5\") for the last lines; log "), "{m}");
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn exited_nonzero_includes_tail_fence() {
        let log = tmp_log(b"line one\nline two\n");
        let info = exited_info("make", Duration::from_secs(61), &log, 17);
        let ev = WakeEvent::Exited {
            id: TaskId(5),
            report: report(101, "exit 101"),
        };
        let m = format_wake(&ev, &info);
        assert!(m.contains("[shell] t5 (`make`) exit 101 after 1m01s — last lines:\n"), "{m}");
        assert!(m.contains("<untrusted-output task=\"t5\">"), "{m}");
        assert!(m.contains("line two"), "{m}");
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn exited_nonzero_missing_log_falls_back() {
        let missing =
            std::env::temp_dir().join(format!("gray-wake-missing-{}.log", std::process::id()));
        let info = exited_info("make", Duration::from_secs(2), &missing, 0);
        let ev = WakeEvent::Exited {
            id: TaskId(5),
            report: report(1, "exit 1"),
        };
        let m = format_wake(&ev, &info);
        assert!(!m.contains("untrusted-output"), "{m}");
        assert!(m.contains("shell_output(task_id=\"t5\") for the last lines; log "), "{m}");
    }

    #[test]
    fn pattern_shape_uses_log_end_as_offset() {
        let log = tmp_log(b"x\n");
        let info = exited_info("watch", Duration::from_secs(1), &log, 2048);
        let ev = WakeEvent::PatternMatched {
            id: TaskId(4),
            line: "panic: boom".into(),
        };
        let m = format_wake(&ev, &info);
        assert_eq!(
            m,
            "[shell] t4 matched: \"panic: boom\" (offset 2048). shell_output(task_id=\"t4\", from_offset=1024)"
        );
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn pattern_line_capped_at_200_chars() {
        let log = tmp_log(b"x\n");
        let info = exited_info("watch", Duration::from_secs(1), &log, 300);
        let ev = WakeEvent::PatternMatched {
            id: TaskId(4),
            line: "y".repeat(300),
        };
        let m = format_wake(&ev, &info);
        assert!(m.contains(&format!("\"{}\"", "y".repeat(200))), "{m}");
        assert!(!m.contains(&"y".repeat(201)), "{m}");
        std::fs::remove_file(&log).ok();
    }

    #[test]
    fn user_input_never_injects_but_formats() {
        let log = tmp_log(b"x\n");
        let info = exited_info("watch", Duration::from_secs(1), &log, 2);
        assert!(!should_inject(&WakeEvent::UserInput));
        assert!(should_inject(&WakeEvent::Exited {
            id: TaskId(5),
            report: report(0, "exit 0"),
        }));
        assert!(!format_wake(&WakeEvent::UserInput, &info).is_empty());
        std::fs::remove_file(&log).ok();
    }
}
