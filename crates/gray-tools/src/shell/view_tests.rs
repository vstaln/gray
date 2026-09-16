use super::*;
use std::path::PathBuf;

use super::super::contract::{INLINE_BUDGET_BYTES, MEM_HEAD_BYTES, MEM_TAIL_BYTES};

fn log_path() -> PathBuf {
    PathBuf::from("/home/u/.gray/shell/s/bash-test.log")
}

fn report(label: &str, note: Option<&str>) -> ExitReport {
    ExitReport {
        effective: 0,
        label: label.into(),
        note: note.map(|s| s.into()),
    }
}

// 1D wave-test fix: ~22 bytes/line so 3,000 lines ≈ 66 KiB, exceeding
// both the 50 KiB and 2,000-line budgets (the brief's "60 KiB,
// 3,000-line input"). The old 11-byte lines never tripped the byte cap.
fn numbered_lines(n: usize) -> Vec<u8> {
    (1..=n)
        .map(|i| format!("line {i:05} {i:010}\n"))
        .collect::<String>()
        .into_bytes()
}

#[test]
fn within_budget_returns_whole() {
    let log = b"hi\n";
    let v = middle_out(log, 50 * 1024, 2000, 0);
    assert_eq!(v.body, "hi\n");
    assert_eq!(v.omitted_lines, 0);
    assert!(v.omitted_range.is_none());
    assert_eq!(v.total_lines, 1);
}

#[test]
fn middle_out_keeps_head_and_tail_with_exact_counts() {
    let log = numbered_lines(3000);
    assert!(log.len() > 50 * 1024);
    let v = middle_out(&log, 50 * 1024, 2000, 0);
    assert!(v.body.contains("{{MARKER}}"));
    let head_tail_bytes: usize = v.body.len().saturating_sub("{{MARKER}}".len() + 2);
    assert!(head_tail_bytes <= 50 * 1024, "{head_tail_bytes}");
    assert!(v.body.starts_with("line 00001 "));
    assert!(v.body.ends_with("0000003000"));
    assert_eq!(
        v.shown_lines.0 + v.shown_lines.1 + v.omitted_lines,
        v.total_lines
    );
    assert_eq!(v.total_lines, 3000);
    let (a, b) = v.omitted_range.unwrap();
    assert!(a < b);
    assert_eq!(
        v.omitted_bytes,
        log.len() - (v.body.len() - "{{MARKER}}".len() - 2)
    );
}

#[test]
fn single_huge_line_cuts_at_codepoint_boundary() {
    let mut log = vec![b'x'; 200 * 1024];
    log.extend_from_slice("héllo 🌍 end".as_bytes());
    let v = middle_out(&log, 50 * 1024, 2000, 100);
    assert!(v.body.contains("{{MARKER}}"));
    // Both halves must still be valid UTF-8 (no torn codepoint).
    assert!(std::str::from_utf8(v.body.as_bytes()).is_ok());
    let (a, b) = v.omitted_range.unwrap();
    let head_len = v
        .body
        .split("{{MARKER}}")
        .next()
        .unwrap()
        .trim_end_matches('\n')
        .len();
    assert_eq!(a, 100 + head_len as u64);
    assert!(b > a);
}

#[test]
fn emoji_straddling_head_cut_is_atomic() {
    // 4-byte emoji placed so a naive byte cut would split it.
    let head_budget = ((50 * 1024) as f32 * VIEW_HEAD_FRACTION).floor() as usize;
    let mut log = vec![b'A'; head_budget - 2];
    log.extend_from_slice("😀".as_bytes()); // bytes head_budget-2..head_budget+2
    log.extend_from_slice(b"\n");
    log.extend_from_slice(&vec![b'B'; 60 * 1024]);
    let v = middle_out(&log, 50 * 1024, 2000, 0);
    let rendered = v.body.replace("{{MARKER}}", "M");
    // Emoji is either wholly in head or wholly out — never replacement char from a split.
    assert!(!rendered.contains('�'), "{rendered:?}");
}

#[test]
fn sanitize_drops_controls_and_folds_crlf() {
    let log = b"a\x00b\x07c\td\re\r\nf\n".to_vec();
    let v = middle_out(&log, 50 * 1024, 2000, 0);
    assert_eq!(v.body, "abc\td\re\nf\n");
}

#[test]
fn elapsed_table() {
    assert_eq!(format_elapsed(Duration::from_millis(100)), "0.1s");
    assert_eq!(format_elapsed(Duration::from_millis(1200)), "1.2s");
    assert_eq!(format_elapsed(Duration::from_secs(41)), "41s");
    assert_eq!(format_elapsed(Duration::from_secs(123)), "2m03s");
    assert_eq!(format_elapsed(Duration::from_secs(3720)), "1h02m");
}

#[test]
fn resume_hint_marker_shape() {
    let log = numbered_lines(3000);
    let v = middle_out(&log, 50 * 1024, 2000, 0);
    let m = resume_hint(&v);
    assert!(m.contains("lines / "), "{m}");
    assert!(m.contains("grep the log path above"), "{m}");
    assert_eq!(resume_hint(&middle_out(b"hi\n", 50 * 1024, 2000, 0)), "");
}

#[test]
fn home_relative_respects_gray_home() {
    // Bug 1: isolated GRAY_HOME must shorten to ~, not leak real HOME.
    let dir = tempfile::tempdir().expect("tempdir");
    let gray = dir.path().to_string_lossy().into_owned();
    let prev = std::env::var("GRAY_HOME").ok();
    unsafe { std::env::set_var("GRAY_HOME", &gray) };
    let p = std::path::PathBuf::from(&gray).join("shell/s/bash-test.log");
    let shown = home_relative(&p);
    match prev {
        Some(v) => unsafe { std::env::set_var("GRAY_HOME", v) },
        None => unsafe { std::env::remove_var("GRAY_HOME") },
    }
    assert!(
        shown.starts_with("~/"),
        "GRAY_HOME path must shorten to ~, got {shown}"
    );
    assert!(shown.contains("shell/s/bash-test.log"), "{shown}");
}

#[test]
fn header_goldens() {
    // Need HOME for the ~ path; skip path assertion if HOME differs.
    let r = report("exit 0", None);
    let v = middle_out(b"hi\n", 50 * 1024, 2000, 0);
    let h = header(&r, Some(&v), Duration::from_millis(1200), &log_path());
    assert!(h.starts_with("exit 0 · 1.2s · 1 lines · log "), "{h}");

    let log = numbered_lines(3000);
    let v2 = middle_out(&log, 50 * 1024, 2000, 0);
    let h2 = header(&r, Some(&v2), Duration::from_secs(41), &log_path());
    assert!(h2.contains("showing first"), "{h2}");
    assert!(h2.contains("omitted"), "{h2}");
    assert!(h2.contains("grep the log for more"), "{h2}");
    // Whole views carry no hint: the log path stays the last field.
    assert!(
        !h.contains("grep the log for more"),
        "whole view must not hint: {h}"
    );
    assert!(h.contains(" · log "), "{h}");

    let kill = report(
        "exit 137 (SIGKILL)",
        Some("likely OOM-killed; check `dmesg | tail`"),
    );
    let h3 = header(&kill, Some(&v), Duration::from_secs(2), &log_path());
    assert!(h3.contains("exit 137 (SIGKILL) (likely OOM-killed"), "{h3}");

    let benign = ExitReport {
        effective: 1,
        label: "exit 1".into(),
        note: Some("no matches — not an error".into()),
    };
    let h4 = header(&benign, Some(&v), Duration::from_millis(100), &log_path());
    assert!(h4.contains("(no matches — not an error)"), "{h4}");

    let h5 = header(&r, None, Duration::from_millis(100), &log_path());
    assert!(h5.contains("no output"), "{h5}");
}

#[test]
fn crlf_sets_folded_flag_and_header_marker() {
    // A CRLF file and an LF file render identically; the header must
    // disclose the folding so the model can distrust the visible bytes.
    let v = middle_out(b"id,name\r\n1,ann\r\n", 50 * 1024, 2000, 0);
    assert!(v.has_cr);
    assert_eq!(v.body, "id,name\n1,ann\n");
    let h = header(
        &report("exit 0", None),
        Some(&v),
        Duration::from_millis(100),
        &log_path(),
    );
    assert!(h.contains("CR folded for display"), "{h}");
    assert!(h.contains(" \u{00b7} log "), "log path stays last: {h}");

    let clean = middle_out(b"id,name\n1,ann\n", 50 * 1024, 2000, 0);
    assert!(!clean.has_cr);
    let h2 = header(
        &report("exit 0", None),
        Some(&clean),
        Duration::from_millis(100),
        &log_path(),
    );
    assert!(!h2.contains("CR folded"), "{h2}");
}

#[test]
fn inline_budget_keeps_head_and_tail_with_elision() {
    // SPEC-01: the foreground inline budget is 6 KiB head + 6 KiB tail.
    assert_eq!(MEM_HEAD_BYTES, 6 * 1024);
    assert_eq!(MEM_TAIL_BYTES, 6 * 1024);
    assert_eq!(INLINE_BUDGET_BYTES, 12 * 1024);
    // 3,000 ~22-byte lines ≈ 66 KiB: over budget, so head + tail survive
    // with an elided middle and exact line accounting.
    let log = numbered_lines(3000);
    assert!(log.len() > INLINE_BUDGET_BYTES);
    let v = middle_out(&log, INLINE_BUDGET_BYTES, 2000, 0);
    assert!(v.body.contains("{{MARKER}}"));
    let shown_bytes: usize = v.body.len().saturating_sub("{{MARKER}}".len() + 2);
    assert!(shown_bytes <= INLINE_BUDGET_BYTES, "{shown_bytes}");
    assert!(v.body.starts_with("line 00001 "));
    assert!(v.body.ends_with("0000003000"));
    assert_eq!(
        v.shown_lines.0 + v.shown_lines.1 + v.omitted_lines,
        v.total_lines
    );
    assert_eq!(v.total_lines, 3000);
    // Just under the budget renders whole with no marker.
    let small = middle_out(
        &log[..INLINE_BUDGET_BYTES - 100],
        INLINE_BUDGET_BYTES,
        2000,
        0,
    );
    assert!(!small.body.contains("{{MARKER}}"));
    assert!(small.omitted_range.is_none());
}
