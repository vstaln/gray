//! shell/exit.rs — honest exit reports (brief 1A, Phase 1).
//!
//! Pure function `ExitStatus` + command string → `ExitReport`. No I/O.
//! NOT wired into the crate yet (`mod exit` + `pub mod shell` land with
//! brief 1D); until then this file is a compile target, not part of the
//! build. Assumes wiring as `shell::exit` with `super::contract::ExitReport`
//! (same layout as `shell::guard`).
//!
//! Unix signals (`ExitStatusExt::signal`) where available; on Windows the
//! signal is always `None`, so signal-derived labels/notes don't apply
//! (plain exit codes still report honestly).

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

use super::contract::ExitReport;
// Shared quote-aware `|` splitter (4B dedupes the NOTE(1A) local copy:
// `||` stays literal, quotes/backslashes/$(…)/heredocs respected).
use super::guard::normalize_guard_head;
use super::split::split_pipeline;

/// Build an honest [`ExitReport`] from a wait status and the command that
/// produced it. Never lies about 128+N; annotates what a model would
/// otherwise misread (OOM, benign grep/diff, masked pipelines).
pub fn exit_report(status: std::process::ExitStatus, command: &str) -> ExitReport {
    let code = status.code();
    #[cfg(unix)]
    let signal: Option<i32> = status.signal();
    #[cfg(not(unix))]
    let signal: Option<i32> = None;
    let (effective, label) = match (code, signal) {
        (Some(n), _) => (n, format!("exit {n}")),
        (None, Some(s)) => {
            let eff = 128 + s;
            // `mut` is dead on Windows (no core-dump push below); keep the
            // single shape and silence the platform-specific warn.
            #[allow(unused_mut)]
            let mut label = format!("exit {eff} ({})", signal_name(s));
            #[cfg(unix)]
            if status.core_dumped() {
                label.push_str(" (core dumped)");
            }
            (eff, label)
        }
        (None, None) => (1, "exit unknown".to_string()),
    };
    let normalized = normalize_guard_head(command);
    let head = command_head(&normalized, command);
    // Signal notes key off the effective value, so a plain `exit 137`
    // (the shell's own 128+N spelling of SIGKILL) annotates the same way.
    let mut note: Option<String> = signal_note(effective).map(str::to_string);
    let mut benign = false;
    if let Some(b) = benign_note(&head, effective) {
        benign = true;
        note = Some(b.to_string());
    } else if effective == 0 && !command.contains("pipefail") {
        note = masked_note(command);
    }
    // `ls` exit 2 ("No such file") is deliberately NOT benign: a missing
    // file is a real error, left unannotated.
    ExitReport {
        code,
        signal,
        effective,
        label,
        note,
        benign,
    }
}

/// `exit {128+s} (SIGNAME)` names; unknown signals stay numeric.
fn signal_name(sig: i32) -> String {
    let name = match sig {
        1 => "HUP",
        2 => "INT",
        3 => "QUIT",
        6 => "ABRT",
        9 => "KILL",
        11 => "SEGV",
        13 => "PIPE",
        14 => "ALRM",
        15 => "TERM",
        24 => "XCPU",
        25 => "XFSZ",
        _ => return format!("SIG{sig}"),
    };
    format!("SIG{name}")
}

/// Notes for the effective values a model would otherwise misread.
fn signal_note(effective: i32) -> Option<&'static str> {
    match effective {
        137 => Some("likely OOM-killed; check `dmesg | tail` or reduce parallelism"),
        143 => Some("terminated (SIGTERM)"),
        139 => Some("segmentation fault"),
        130 => Some("interrupted (SIGINT)"),
        _ => None,
    }
}

/// Benign-exit table: nonzero codes that are data, not errors.
fn benign_note(head: &str, effective: i32) -> Option<&'static str> {
    match (head, effective) {
        ("grep" | "egrep" | "fgrep" | "rg" | "ag", 1) => Some("no matches — not an error"),
        ("diff" | "cmp", 1) => Some("files differ — not an error"),
        ("test" | "[", 1) => Some("condition false — not an error"),
        ("which" | "command", 1) => Some("not found on PATH"),
        ("pgrep" | "pkill", 1) => Some("no processes matched"),
        _ => None,
    }
}

/// Head binary: first whitespace token, basename after `/`.
/// `command -v foo` normalizes to `-v foo` (wrapper-strip), so the
/// `command` head is recovered when the raw first token says so.
fn command_head(normalized: &str, raw: &str) -> String {
    let head = normalized.split_whitespace().next().unwrap_or("");
    if head == "-v" && base_head(raw) == "command" {
        return "command".to_string();
    }
    base_head(normalized).to_string()
}

fn base_head(cmd: &str) -> &str {
    let head = cmd.split_whitespace().next().unwrap_or("");
    head.rsplit('/').next().unwrap_or(head)
}

/// Pipeline note when the tail command's status masks the real one.
fn masked_note(command: &str) -> Option<String> {
    const TAIL_CMDS: [&str; 8] = ["tail", "head", "grep", "tee", "sort", "wc", "less", "cat"];
    let segs = split_pipeline(command);
    if segs.len() < 2 {
        return None;
    }
    let last = base_head(&segs[segs.len() - 1]);
    if !TAIL_CMDS.contains(&last) {
        return None;
    }
    let first = base_head(&segs[0]);
    Some(format!(
        "`{}` reports {last}'s exit, not {first}'s; rerun with `set -o pipefail;` to see the real status",
        command.trim()
    ))
}

#[cfg(all(test, unix))] // ExitStatusExt::from_raw is unix-only (T3 windows gate)
mod exit_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn code(n: i32) -> std::process::ExitStatus {
        std::process::ExitStatus::from_raw(n << 8)
    }

    fn sig(s: i32) -> std::process::ExitStatus {
        std::process::ExitStatus::from_raw(s)
    }

    #[test]
    fn plain_exit_code_is_honest() {
        let r = exit_report(code(3), "sh -c 'exit 3'");
        assert_eq!(r.effective, 3);
        assert_eq!(r.label, "exit 3");
        assert!(!r.benign);
        assert!(r.note.is_none());
    }

    #[test]
    fn sigkill_maps_to_137_with_oom_note() {
        let r = exit_report(sig(9), "sh -c 'kill -9 $$'");
        assert_eq!(r.effective, 137);
        assert!(r.label.contains("SIGKILL"), "{}", r.label);
        assert!(r.note.as_deref().unwrap_or("").contains("OOM"));
        assert!(!r.benign);
    }

    #[test]
    fn sigterm_maps_to_143() {
        let r = exit_report(sig(15), "sh -c 'kill -15 $$'");
        assert_eq!(r.effective, 143);
        assert!(r.label.contains("SIGTERM"), "{}", r.label);
        assert!(r.note.as_deref().unwrap_or("").contains("terminated"));
    }

    #[test]
    fn grep_no_match_is_benign() {
        let r = exit_report(code(1), "grep zzz /dev/null");
        assert_eq!(r.effective, 1);
        assert!(r.benign);
        assert!(r.note.as_deref().unwrap_or("").contains("no matches"));
    }

    #[test]
    fn diff_success_is_plain() {
        let r = exit_report(code(0), "diff a a");
        assert!(!r.benign);
        assert!(r.note.is_none());
    }

    #[test]
    fn diff_difference_is_benign() {
        let r = exit_report(code(1), "diff a b");
        assert!(r.benign);
        assert!(r.note.as_deref().unwrap_or("").contains("files differ"));
    }

    #[test]
    fn masked_pipeline_gets_pipefail_note() {
        let r = exit_report(code(0), "false | tail -1");
        assert!(!r.benign);
        assert!(r.note.as_deref().unwrap_or("").contains("pipefail"));
    }

    #[test]
    fn pipefail_suppresses_masked_note() {
        let r = exit_report(code(1), "set -o pipefail; false | tail -1");
        assert!(r.note.is_none());
    }

    #[test]
    fn sudo_env_wrapper_still_matches_benign_table() {
        let r = exit_report(code(1), "sudo env X=1 grep z f");
        assert!(r.benign);
        assert!(r.note.as_deref().unwrap_or("").contains("no matches"));
    }

    #[test]
    fn command_dash_v_is_benign() {
        let r = exit_report(code(1), "command -v foo");
        assert!(r.benign);
        assert!(r.note.as_deref().unwrap_or("").contains("PATH"));
    }

    #[test]
    fn or_or_is_not_a_pipeline() {
        let r = exit_report(code(0), "false || true");
        assert!(r.note.is_none());
    }

    #[test]
    fn quoted_pipe_is_not_a_pipeline() {
        let r = exit_report(code(0), "echo \"a|b\"");
        assert!(r.note.is_none());
    }

    #[test]
    fn ls_missing_file_stays_an_error() {
        let r = exit_report(code(2), "ls /nonexistent");
        assert!(!r.benign);
        assert!(r.note.is_none());
    }

    #[test]
    fn interrupt_notes() {
        let r = exit_report(code(130), "sleep 10");
        assert!(r.note.as_deref().unwrap_or("").contains("interrupted"));
        let r = exit_report(sig(2), "sh -c 'kill -INT $$'");
        assert_eq!(r.effective, 130);
        assert!(r.label.contains("SIGINT"), "{}", r.label);
    }

    #[test]
    fn unknown_signal_stays_numeric() {
        let r = exit_report(sig(10), "sh -c 'kill -USR1 $$'");
        assert_eq!(r.effective, 138);
        assert!(r.label.contains("SIG10"), "{}", r.label);
    }
}
