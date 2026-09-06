//! shell/view.rs — middle-out view, header, resume hint (brief 1B).
//!
//! NOT wired into the crate yet (`pub mod shell` + `bash.rs` shrink land
//! with brief 1D, which unifies these mirrors with `contract.rs`). Until
//! then this file is standalone (std only) so it compiles without touching
//! `lib.rs` / `mod.rs` / `contract.rs`.
//!
//! Types come from `super::contract` (mirrors deleted by 1D wiring;
//! signatures unchanged).
//!
//! Marker protocol: `middle_out` leaves a literal `{{MARKER}}` line where
//! `resume_hint(task, view)` output goes. The caller (1D) replaces it, so
//! `middle_out` stays pure over `(log, budgets, base_offset)` with no task
//! id in its signature.

use std::path::Path;
use std::time::Duration;

use super::contract::{ExitReport, TaskId, TaskInfo, VIEW_HEAD_FRACTION, View};

// ── formatting helpers ──────────────────────────────────────────────

/// Thousands separators: 402113 -> "402,113". Models misread bare digits.
pub fn fmt_num(mut n: usize) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let mut groups: Vec<String> = Vec::new();
    while n > 0 {
        groups.push(format!("{:03}", n % 1000));
        n /= 1000;
    }
    let mut out = groups.pop().unwrap().trim_start_matches('0').to_string();
    if out.is_empty() {
        out = "0".to_string();
    }
    while let Some(g) = groups.pop() {
        out.push(',');
        out.push_str(&g);
    }
    out
}

fn fmt_num_u64(n: u64) -> String {
    fmt_num(n as usize)
}

/// Elapsed table: 100ms->0.1s, 1200ms->1.2s, 41s->41s, 123s->2m03s, 3720s->1h02m.
pub fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 10 {
        format!("{:.1}s", d.as_secs_f64())
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Home-relative log path: $GRAY_HOME/... -> ~/... else $HOME/... -> ~/...
pub fn home_relative(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Ok(gray) = std::env::var("GRAY_HOME")
        && !gray.trim().is_empty()
        && (s.as_ref() == gray || s.starts_with(&format!("{gray}/")))
    {
        return format!("~{}", &s[gray.len()..]);
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && (s.as_ref() == home || s.starts_with(&format!("{home}/")))
    {
        return format!("~{}", &s[home.len()..]);
    }
    s.into_owned()
}

// ── sanitize (moved here from bash.rs sanitize_binary_output) ───────

/// Drop C0 controls except \t \n \r, lossy UTF-8, CRLF -> LF.
/// Byte-level filter first: bytes < 0x20 are never part of a multi-byte
/// UTF-8 sequence, so filtering cannot tear a codepoint.
fn sanitize(log: &[u8]) -> String {
    let filtered: Vec<u8> = log
        .iter()
        .filter(|&&b| b == 0x09 || b == 0x0A || b == 0x0D || b >= 0x20)
        .copied()
        .collect();
    String::from_utf8_lossy(&filtered).replace("\r\n", "\n")
}

/// Lines = newline count, plus a trailing partial line. Operates on bytes
/// so a trailing newline is not miscounted (`wc -l` agreement).
fn count_lines(s: &str) -> usize {
    if s.is_empty() {
        return 0;
    }
    let nl = s.bytes().filter(|&b| b == b'\n').count();
    if s.ends_with('\n') { nl } else { nl + 1 }
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

/// First `byte_budget` bytes, cut back to a line boundary (drop the partial
/// last line); no newline in window -> byte-cut at the codepoint boundary
/// (single-huge-line case). Then cap to the first `line_budget` lines.
fn take_head(s: &str, byte_budget: usize, line_budget: usize) -> String {
    if s.is_empty() || byte_budget == 0 || line_budget == 0 {
        return String::new();
    }
    let mut end = floor_char_boundary(s, byte_budget.min(s.len()));
    if end < s.len()
        && let Some(nl) = s[..end].rfind('\n')
    {
        end = nl;
    }
    // Exclude the boundary newline itself; the marker join adds newlines.
    let mut head = s[..end].trim_end_matches('\n').to_string();
    if count_lines(&head) > line_budget {
        let keep: Vec<&str> = head.lines().take(line_budget).collect();
        head = keep.join("\n");
    }
    head
}

/// Last `byte_budget` bytes, cut forward past the partial first line; a cut
/// that would empty the tail keeps the byte-cut at the codepoint boundary.
/// Then keep only the last `line_budget` lines.
fn take_tail(s: &str, byte_budget: usize, line_budget: usize) -> String {
    if s.is_empty() || byte_budget == 0 || line_budget == 0 {
        return String::new();
    }
    let mut start = ceil_char_boundary(s, s.len().saturating_sub(byte_budget));
    if start > 0 && start < s.len() {
        if let Some(nl) = s[start..].find('\n') {
            let after = start + nl + 1;
            if after < s.len() {
                start = after;
            }
            // else: dropping would empty the tail -> keep the byte-cut.
        }
        // else: no newline in window (huge line) -> keep the byte-cut.
    } else if start == 0 {
        // Whole string fits the byte window; line cap below still applies.
    }
    let mut tail = s[start..].trim_start_matches('\n').to_string();
    // Strip one trailing newline for clean joins; counts are unaffected.
    tail = tail.trim_end_matches('\n').to_string();
    if count_lines(&tail) > line_budget {
        let lines: Vec<&str> = tail.lines().collect();
        let n = lines.len();
        tail = lines[n - line_budget..].join("\n");
    }
    tail
}

// ── contract fns ────────────────────────────────────────────────────

/// Bounded middle-out view. Offsets/lengths are in sanitized space; 1D
/// re-sanitizes identically when paging, so `omitted_range` lines up.
pub fn middle_out(log: &[u8], budget_bytes: usize, budget_lines: usize, base_offset: u64) -> View {
    let s = sanitize(log);
    let total_bytes = s.len() as u64;
    let total_lines = count_lines(&s);
    if s.len() <= budget_bytes && total_lines <= budget_lines {
        return View {
            body: s,
            shown_lines: (total_lines, 0),
            omitted_lines: 0,
            omitted_bytes: 0,
            omitted_range: None,
            total_lines,
            total_bytes,
        };
    }
    let head_byte_budget = (budget_bytes as f32 * VIEW_HEAD_FRACTION).floor() as usize;
    let tail_byte_budget = budget_bytes.saturating_sub(head_byte_budget);
    let head_line_budget = (budget_lines as f32 * VIEW_HEAD_FRACTION).floor() as usize;
    let tail_line_budget = budget_lines - head_line_budget;
    let head = take_head(&s, head_byte_budget, head_line_budget);
    let tail = take_tail(&s, tail_byte_budget, tail_line_budget);
    let (head_lines, tail_lines) = (count_lines(&head), count_lines(&tail));
    let omitted_lines = total_lines.saturating_sub(head_lines + tail_lines);
    let omitted_bytes = s.len().saturating_sub(head.len() + tail.len());
    let omitted_range = Some((
        base_offset + head.len() as u64,
        base_offset + total_bytes.saturating_sub(tail.len() as u64),
    ));
    View {
        body: format!("{}\n{{{{MARKER}}}}\n{}", head, tail),
        shown_lines: (head_lines, tail_lines),
        omitted_lines,
        omitted_bytes,
        omitted_range,
        total_lines,
        total_bytes,
    }
}

/// Marker line for the `{{MARKER}}` slot. Empty string when nothing omitted.
pub fn resume_hint(task: TaskId, view: &View) -> String {
    let Some((a, b)) = view.omitted_range else {
        return String::new();
    };
    if view.omitted_lines == 0 && view.omitted_bytes == 0 {
        return String::new();
    }
    format!(
        "[\u{2026} {} lines / {} chars omitted (bytes {}\u{2013}{}). grep the log path above, or shell_output(task_id=\"{}\", from_offset={}) to page \u{2026}]",
        fmt_num(view.omitted_lines),
        fmt_num(view.omitted_bytes),
        fmt_num_u64(a),
        fmt_num_u64(b),
        task,
        a,
    )
}

/// First result line: "{label}{(note)} · {elapsed} · {total} lines[ · showing
/// first h + last t · omitted] · log {path}[ · no output]".
/// Running tasks (report None) render as "task tN running · pid P".
pub fn header(
    task: &TaskInfo,
    report: Option<&ExitReport>,
    view: Option<&View>,
    elapsed: Duration,
) -> String {
    let label = report
        .map(|r| r.label.clone())
        .unwrap_or_else(|| format!("task {} running · pid {}", task.id, task.pid));
    let note = report
        .and_then(|r| r.note.as_ref())
        .map(|n| format!(" ({n})"))
        .unwrap_or_default();
    let total = view.map(|v| v.total_lines).unwrap_or(0);
    let showing = match view {
        Some(v) if v.omitted_lines > 0 || v.omitted_bytes > 0 => format!(
            " · showing first {} + last {} · {} lines / {} chars omitted",
            fmt_num(v.shown_lines.0),
            fmt_num(v.shown_lines.1),
            fmt_num(v.omitted_lines),
            fmt_num(v.omitted_bytes),
        ),
        _ => String::new(),
    };
    let mut out = format!(
        "{label}{note} · {} · {} lines{showing} · log {}",
        format_elapsed(elapsed),
        fmt_num(total),
        home_relative(&task.log_path),
    );
    if total == 0 {
        out.push_str(" · no output");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    use super::super::contract::TaskState;

    fn task() -> TaskInfo {
        TaskInfo {
            id: TaskId(4),
            pid: 123,
            pgid: 123,
            command: "test".into(),
            started: Instant::now(),
            log_path: PathBuf::from("/home/u/.gray/shell/s/t4.log"),
            bytes: 0,
            state: TaskState::Running,
        }
    }

    fn report(label: &str, note: Option<&str>) -> ExitReport {
        ExitReport {
            code: Some(0),
            signal: None,
            effective: 0,
            label: label.into(),
            note: note.map(|s| s.into()),
            benign: false,
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
        let m = resume_hint(TaskId(4), &v);
        let (a, _) = v.omitted_range.unwrap();
        assert!(m.contains("shell_output(task_id=\"t4\""), "{m}");
        assert!(m.contains(&format!("from_offset={a}")), "{m}");
        assert!(m.contains("lines / "), "{m}");
        assert_eq!(
            resume_hint(TaskId(4), &middle_out(b"hi\n", 50 * 1024, 2000, 0)),
            ""
        );
    }

    #[test]
    fn home_relative_respects_gray_home() {
        // Bug 1: isolated GRAY_HOME must shorten to ~, not leak real HOME.
        let dir = tempfile::tempdir().expect("tempdir");
        let gray = dir.path().to_string_lossy().into_owned();
        let prev = std::env::var("GRAY_HOME").ok();
        unsafe { std::env::set_var("GRAY_HOME", &gray) };
        let p = std::path::PathBuf::from(&gray).join("shell/s/t1.log");
        let shown = home_relative(&p);
        match prev {
            Some(v) => unsafe { std::env::set_var("GRAY_HOME", v) },
            None => unsafe { std::env::remove_var("GRAY_HOME") },
        }
        assert!(
            shown.starts_with("~/"),
            "GRAY_HOME path must shorten to ~, got {shown}"
        );
        assert!(shown.contains("shell/s/t1.log"), "{shown}");
    }

    #[test]
    fn header_goldens() {
        // Need HOME for the ~ path; skip path assertion if HOME differs.
        let t = task();
        let r = report("exit 0", None);
        let v = middle_out(b"hi\n", 50 * 1024, 2000, 0);
        let h = header(&t, Some(&r), Some(&v), Duration::from_millis(1200));
        assert!(h.starts_with("exit 0 · 1.2s · 1 lines · log "), "{h}");

        let log = numbered_lines(3000);
        let v2 = middle_out(&log, 50 * 1024, 2000, 0);
        let h2 = header(&t, Some(&r), Some(&v2), Duration::from_secs(41));
        assert!(h2.contains("showing first"), "{h2}");
        assert!(h2.contains("omitted"), "{h2}");

        let kill = report(
            "exit 137 (SIGKILL)",
            Some("likely OOM-killed; check `dmesg | tail`"),
        );
        let h3 = header(&t, Some(&kill), Some(&v), Duration::from_secs(2));
        assert!(h3.contains("exit 137 (SIGKILL) (likely OOM-killed"), "{h3}");

        let benign = ExitReport {
            code: Some(1),
            signal: None,
            effective: 1,
            label: "exit 1".into(),
            note: Some("no matches — not an error".into()),
            benign: true,
        };
        let h4 = header(&t, Some(&benign), Some(&v), Duration::from_millis(100));
        assert!(h4.contains("(no matches — not an error)"), "{h4}");

        let h5 = header(&t, Some(&r), None, Duration::from_millis(100));
        assert!(h5.contains("no output"), "{h5}");
    }
}
