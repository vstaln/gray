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
//! `resume_hint(view)` output goes. The caller replaces it, so
//! `middle_out` stays pure over `(log, budgets, base_offset)`.

use std::path::Path;
use std::time::Duration;

use super::contract::{ExitReport, VIEW_HEAD_FRACTION, View};

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

pub fn fmt_num_u64(n: u64) -> String {
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
    // Prefix match must accept both separators: Windows paths arrive with
    // backslashes while GRAY_HOME/HOME may hold either spelling.
    let under = |root: &str| -> bool {
        !root.is_empty()
            && (s.as_ref() == root
                || s.starts_with(&format!("{root}/"))
                || s.starts_with(&format!("{root}\\"))
                || s.eq_ignore_ascii_case(root))
    };
    if let Ok(gray) = std::env::var("GRAY_HOME")
        && !gray.trim().is_empty()
        && under(gray.trim())
    {
        // Keep the documented "~/..." shape: normalize the remainder's
        // first separator so consumers expanding "~/", including the
        // shell_contract log-path parser, keep working on native paths.
        return format!(
            "~/{}",
            s[gray.trim().len()..].trim_start_matches(['/', '\\'])
        );
    }
    if let Ok(home) = std::env::var("HOME")
        && under(&home)
    {
        return format!("~/{}", s[home.len()..].trim_start_matches(['/', '\\']));
    }
    s.into_owned()
}

// ── sanitize (moved here from bash.rs sanitize_binary_output) ───────

/// Drop C0 controls except \t \n \r, lossy UTF-8, CRLF and bare CR -> LF.
/// Progress updates must not carry cursor-reset controls into the terminal UI.
/// Byte-level filter first: bytes < 0x20 are never part of a multi-byte
/// UTF-8 sequence, so filtering cannot tear a codepoint.
fn sanitize(log: &[u8]) -> String {
    let filtered: Vec<u8> = log
        .iter()
        .filter(|&&b| b == 0x09 || b == 0x0A || b == 0x0D || b >= 0x20)
        .copied()
        .collect();
    String::from_utf8_lossy(&filtered)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
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

/// First `byte_budget` bytes, cut back to a line boundary (drop the partial
/// last line); no newline in window -> byte-cut at the codepoint boundary
/// (single-huge-line case). Then cap to the first `line_budget` lines.
fn take_head(s: &str, byte_budget: usize, line_budget: usize) -> String {
    if s.is_empty() || byte_budget == 0 || line_budget == 0 {
        return String::new();
    }
    let mut end = s.floor_char_boundary(byte_budget.min(s.len()));
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
    let mut start = s.ceil_char_boundary(s.len().saturating_sub(byte_budget));
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
    // Raw-log check (pre-sanitize): sanitize folds `\r\n` for display, so
    // the flag must come from the unfolded bytes.
    let has_cr = log.contains(&b'\r');
    if s.len() <= budget_bytes && total_lines <= budget_lines {
        return View {
            body: s,
            shown_lines: (total_lines, 0),
            omitted_lines: 0,
            omitted_bytes: 0,
            omitted_range: None,
            total_lines,
            total_bytes,
            has_cr,
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
        has_cr,
    }
}

/// Marker line for the `{{MARKER}}` slot. Empty string when nothing omitted.
/// Marker line for the `{{MARKER}}` slot. Empty string when nothing omitted.
pub fn resume_hint(view: &View) -> String {
    let Some((a, b)) = view.omitted_range else {
        return String::new();
    };
    if view.omitted_lines == 0 && view.omitted_bytes == 0 {
        return String::new();
    }
    format!(
        "[\u{2026} {} lines / {} chars omitted (bytes {}\u{2013}{}). grep the log path above.]",
        fmt_num(view.omitted_lines),
        fmt_num(view.omitted_bytes),
        fmt_num_u64(a),
        fmt_num_u64(b),
    )
}

/// First result line: "{label}{(note)} \u{00b7} {elapsed} \u{00b7} {total} lines[ \u{00b7} showing
/// first h + last t \u{00b7} omitted \u{00b7} grep-hint][ \u{00b7} CR folded] \u{00b7} log {path}[ \u{00b7} no output]".
/// The grep hint points at the on-disk log (never rerun to see more); the
/// log path stays last so `split("\u{00b7} log ")` parsers keep working.
/// `CR folded` discloses sanitize's CRLF folding: without it a CRLF file
/// and an LF file render identically and the model cannot see the bytes
/// that fail a byte-exact check.
pub fn header(
    report: &ExitReport,
    view: Option<&View>,
    elapsed: Duration,
    log_path: &std::path::Path,
) -> String {
    let label = report.label.clone();
    let note = report
        .note
        .as_ref()
        .map(|n| format!(" ({n})"))
        .unwrap_or_default();
    let total = view.map(|v| v.total_lines).unwrap_or(0);
    let showing = match view {
        Some(v) if v.omitted_lines > 0 || v.omitted_bytes > 0 => format!(
            " \u{00b7} showing first {} + last {} \u{00b7} {} lines / {} chars omitted \u{00b7} grep the log for more",
            fmt_num(v.shown_lines.0),
            fmt_num(v.shown_lines.1),
            fmt_num(v.omitted_lines),
            fmt_num(v.omitted_bytes),
        ),
        _ => String::new(),
    };
    let folded = match view {
        Some(v) if v.has_cr => " \u{00b7} CR folded for display",
        _ => "",
    };
    let mut out = format!(
        "{label}{note} \u{00b7} {} \u{00b7} {} lines{showing}{folded} \u{00b7} log {}",
        format_elapsed(elapsed),
        fmt_num(total),
        home_relative(log_path),
    );
    if total == 0 {
        out.push_str(" \u{00b7} no output");
    }
    out
}

#[path = "view_tests.rs"]
#[cfg(test)]
mod tests;
