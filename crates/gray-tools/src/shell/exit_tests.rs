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
    assert!(r.note.is_none());
}

#[test]
fn sigkill_maps_to_137_with_oom_note() {
    let r = exit_report(sig(9), "sh -c 'kill -9 $$'");
    assert_eq!(r.effective, 137);
    assert!(r.label.contains("SIGKILL"), "{}", r.label);
    assert!(r.note.as_deref().unwrap_or("").contains("OOM"));
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
    assert!(r.note.as_deref().unwrap_or("").contains("no matches"));
}

#[test]
fn diff_success_is_plain() {
    let r = exit_report(code(0), "diff a a");
    assert!(r.note.is_none());
}

#[test]
fn diff_difference_is_benign() {
    let r = exit_report(code(1), "diff a b");
    assert!(r.note.as_deref().unwrap_or("").contains("files differ"));
}

#[test]
fn masked_pipeline_note_is_posix_honest() {
    // The executor is `sh -c` (dash on some systems): the note must
    // never recommend `set -o pipefail` (a bashism dash rejects with
    // exit 2). POSIX advice — rerun the first stage without the pipe —
    // works in every shell.
    let r = exit_report(code(0), "false | tail -1");
    let note = r.note.as_deref().unwrap_or("");
    assert!(!note.contains("pipefail"), "{note}");
    assert!(note.contains("tail"), "{note}");
    assert!(note.contains("false"), "{note}");
    // Wire-budget: the note must name the two stages, never echo the
    // full pipeline (long commands would ride every header twice).
    assert!(
        !note.contains('|'),
        "header must not echo the pipeline: {note}"
    );
}

#[test]
fn pipefail_mention_does_not_suppress_masked_note() {
    // Leftover-word false negative: the note keys off the pipeline
    // shape, not the command text. A command merely mentioning
    // "pipefail" under `sh -c` still reports the tail's exit.
    let r = exit_report(code(0), "echo pipefail | tail -1");
    assert!(r.note.is_some());
}

#[test]
fn wrapped_heads_are_taken_literally_now() {
    // The destructive-command guard owned wrapper stripping (`sudo`/`env`/
    // `nice`/`timeout`). With the guard gone, exit reporting reads the
    // literal head, so a wrapped benign command gets no benign note.
    let r = exit_report(code(1), "sudo env X=1 grep z f");
    assert!(r.note.is_none());
}

#[test]
fn command_dash_v_is_benign() {
    let r = exit_report(code(1), "command -v foo");
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
