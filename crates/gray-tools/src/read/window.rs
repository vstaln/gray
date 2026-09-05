//! T1.1 ceilings + T1.2 `cat -n` prefixes + T2.2 deferred cut for `read`.
//!
//! Pure functions only (no I/O): `ReadTool::execute` streams lines through
//! [`window`], which numbers the selected window with absolute 1-based line
//! numbers *before* the line/byte caps, so `next_offset` math is unchanged
//! and a cut never splits a line.
//!
//! Byte-budget side effect (deliberate, conservative): the ~7-byte prefix
//! counts toward the byte cap, so a byte-cut may fire a few lines earlier
//! than on raw text. Numbering after the caps would instead overshoot the
//! budget and need per-branch rework in `mod.rs` — bigger diff, same contract.
//!
//! T2.2 deferred cut: [`window`] never claims "more remains" on its own when
//! the line window fills exactly — the caller passes `has_more` (it read one
//! line past the window and actually observed it). So `cut.is_some()` ⇔ a
//! re-read at `next_offset` returns ≥1 line. A byte cut always names a line
//! already read (the one that did not fit), so it needs no peek.

/// Format one line as `cat -n` does: right-aligned width 6, then a tab.
/// `n` is the absolute 1-based file line number. Width 6 never truncates:
/// larger numbers simply overflow the field.
pub fn prefix_line(n: usize, line: &str) -> String {
    format!("{n:>6}\t{line}")
}

/// Number a selected window: `lines[0]` is file line `first_n`.
pub fn prefix_lines(first_n: usize, lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| prefix_line(first_n + i, l))
        .collect()
}

/// Spec-fixed per-line ceiling in chars (canonical home; `stream.rs`
/// re-exports it — same value, do not drift).
pub const MAX_LINE_CHARS: usize = 2000;

/// Spec-fixed window ceilings (T1.1): at most this many lines and this many
/// output bytes (prefixes included) per read.
pub const MAX_LINES: usize = 2000;
pub const MAX_BYTES: usize = 50 * 1024;

/// Env override `GRAY_READ_MAX_LINE_CHARS` (positive ints only, else default).
pub fn max_line_chars() -> usize {
    std::env::var("GRAY_READ_MAX_LINE_CHARS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(MAX_LINE_CHARS)
}

/// Env override `GRAY_READ_MAX_LINES` (positive ints only, else default).
pub fn max_lines() -> usize {
    std::env::var("GRAY_READ_MAX_LINES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(MAX_LINES)
}

/// Env override `GRAY_READ_MAX_BYTES` (positive ints only, else default).
pub fn max_bytes() -> usize {
    std::env::var("GRAY_READ_MAX_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(MAX_BYTES)
}

/// Which ceiling cut the window (T1.1 `Cut{Lines|Bytes}`).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Cut {
    Lines,
    Bytes,
}

/// One streamed input line: decoded text plus the char count of the bytes the
/// stream discarded past its per-line byte cap (`0` for ordinary lines).
/// The discard count keeps the clamp marker exact for over-long lines whose
/// full text never materializes (counted as leading UTF-8 bytes while
/// discarding — exact for valid UTF-8).
pub struct WindowLine {
    pub text: String,
    pub overflow_chars: u64,
}

/// A windowed, numbered, clamped, capped view of the file from `first_n`.
pub struct Window {
    /// Prefixed (`cat -n`) clamped lines, ready to `join("\n")`.
    pub shown: Vec<String>,
    /// The ceiling that fired, if any. `Some` ⇔ `next_offset` names a line
    /// that was actually observed (peeked past the window or read but unshown
    /// by the byte cap), so re-reading there returns ≥1 line.
    pub cut: Option<Cut>,
    /// Clamped lines among `shown` (unshown lines past a byte cut are not
    /// counted — the note describes the output the model sees).
    pub clamped: usize,
    /// Resume offset: line after the window (line cut) or the unshown line
    /// itself (byte cut). `None` when `cut` is `None`.
    pub next_offset: Option<usize>,
}

/// Clamp one line when its full char count exceeds `max`. `overflow_chars`
/// is the stream's discard count for this line (`0` when the whole line is in
/// `line`). Returns the (possibly clamped) line + hit flag. Identical to
/// [`clamp_line`] when `overflow_chars == 0`.
pub fn clamp_counted(line: &str, overflow_chars: u64, max: usize) -> (String, bool) {
    let total = line.chars().count() as u64 + overflow_chars;
    if total <= max as u64 {
        return (line.to_string(), false);
    }
    let kept: String = line.chars().take(max).collect();
    (format!("{kept} …[+{} chars]", total - max as u64), true)
}

/// Apply clamp → prefix → line/byte caps to the decoded lines starting at the
/// absolute 1-based `first_n`. `has_more` is the T2.2 deferred peek and must
/// be true only when the caller actually observed a line past `lines`: an
/// exactly-filled line window with `has_more == false` is complete, not cut.
/// A byte cut needs no peek — the unshown line is in `lines`.
pub fn window(
    first_n: usize,
    lines: &[WindowLine],
    max_lines: usize,
    max_bytes: usize,
    max_chars: usize,
    has_more: bool,
) -> Window {
    let mut shown: Vec<String> = Vec::new();
    let mut bytes_used: usize = 0;
    let mut clamped = 0;
    for (i, l) in lines.iter().enumerate() {
        let n = first_n + i;
        if shown.len() >= max_lines {
            // More lines observed than the window holds: line cut, resume
            // AFTER the last shown line.
            return Window {
                shown,
                cut: Some(Cut::Lines),
                clamped,
                next_offset: Some(n),
            };
        }
        let (c, hit) = clamp_counted(&l.text, l.overflow_chars, max_chars);
        let p = prefix_line(n, &c);
        let need = p.len() + if shown.is_empty() { 0 } else { 1 };
        if bytes_used + need > max_bytes {
            // Stop BEFORE the unshown line; resume ON it. The unshown
            // line's clamp (if any) is not counted — the note describes
            // the visible output.
            return Window {
                shown,
                cut: Some(Cut::Bytes),
                clamped,
                next_offset: Some(n),
            };
        }
        if hit {
            clamped += 1;
        }
        shown.push(p);
        bytes_used += need;
    }
    // Every supplied line shown. An exactly-filled window is a cut only when
    // the peek proved more remains (deferred decision — never guess).
    if shown.len() >= max_lines && max_lines > 0 && has_more {
        let next = first_n + lines.len();
        return Window {
            shown,
            cut: Some(Cut::Lines),
            clamped,
            next_offset: Some(next),
        };
    }
    Window {
        shown,
        cut: None,
        clamped,
        next_offset: None,
    }
}

/// Clamp one line to `max` chars (chars, not bytes): keep the first `max`
/// chars at a char boundary and append ` …[+{N} chars]` where N is the
/// omitted char count. Returns the (possibly clamped) line + hit flag.
pub fn clamp_line(line: &str, max: usize) -> (String, bool) {
    let total = line.chars().count();
    if total <= max {
        return (line.to_string(), false);
    }
    let kept: String = line.chars().take(max).collect();
    (format!("{kept} …[+{} chars]", total - max), true)
}

/// Clamp a selected window; returns the lines + clamped-line count.
/// Byte budget is charged AFTER this (caller truncates the clamped text).
pub fn clamp_lines(lines: &[&str], max: usize) -> (Vec<String>, usize) {
    let mut out = Vec::with_capacity(lines.len());
    let mut count = 0;
    for l in lines {
        let (c, hit) = clamp_line(l, max);
        if hit {
            count += 1;
        }
        out.push(c);
    }
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_matches_cat_n_layout() {
        assert_eq!(prefix_line(1, "hi"), "     1\thi");
        assert_eq!(prefix_line(412, "foo"), "   412\tfoo");
    }

    #[test]
    fn window_numbering_is_absolute_not_window_relative() {
        // offset=100 limit=3 → prefixes 100,101,102.
        let got = prefix_lines(100, &["a", "b", "c"]);
        assert_eq!(got, vec!["   100\ta", "   101\tb", "   102\tc"]);
    }

    #[test]
    fn width_never_truncates_large_numbers() {
        assert_eq!(prefix_line(1_000_000, "x"), "1000000\tx");
        assert_eq!(prefix_line(12_345_678, "x"), "12345678\tx");
    }

    #[test]
    fn empty_lines_still_carry_their_number() {
        assert_eq!(prefix_lines(2, &["", "b"]), vec!["     2\t", "     3\tb"]);
    }

    #[test]
    fn short_lines_pass_through_unclamped() {
        let (s, hit) = clamp_line("hello", 2000);
        assert_eq!(s, "hello");
        assert!(!hit);
        // Exactly at the ceiling is not clamped.
        let line = "x".repeat(2000);
        let (s, hit) = clamp_line(&line, 2000);
        assert_eq!(s, line);
        assert!(!hit);
    }

    #[test]
    fn minified_line_keeps_first_2000_chars_plus_marker() {
        // minified.js shape: one 3,900-char line + normal lines.
        let line = "a".repeat(3900);
        let (got, hit) = clamp_line(&line, 2000);
        assert!(hit);
        assert_eq!(got, format!("{} …[+1900 chars]", "a".repeat(2000)));
        // Remaining lines still shown; count is per-window.
        let (lines, count) = clamp_lines(&[&line, "b", "c"], 2000);
        assert_eq!(count, 1);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1], "b");
    }

    #[test]
    fn clamp_is_char_based_and_never_splits_a_codepoint() {
        // 2001 emoji: char 2001 is kept whole then cut, marker counts chars.
        let line = "😀".repeat(2001);
        let (got, hit) = clamp_line(&line, 2000);
        assert!(hit);
        assert_eq!(got, format!("{} …[+1 chars]", "😀".repeat(2000)));
        assert!(got.starts_with(&"😀".repeat(2000)));
    }

    #[test]
    fn ceiling_defaults_to_2000() {
        assert_eq!(MAX_LINE_CHARS, 2000);
        // Explicit-max path covers the unit logic; the env override in
        // max_line_chars() is exercised at the wave gate (serial env).
        let (got, _) = clamp_line(&"y".repeat(10), 5);
        assert_eq!(got, "yyyyy …[+5 chars]");
    }

    fn wlines(n: usize) -> Vec<WindowLine> {
        (1..=n)
            .map(|i| WindowLine {
                text: format!("line {i:04}"),
                overflow_chars: 0,
            })
            .collect()
    }

    #[test]
    fn ceilings_default_to_spec_values() {
        assert_eq!(MAX_LINES, 2000);
        assert_eq!(MAX_BYTES, 50 * 1024);
        // Env-unset defaults (overrides are exercised serially at the gate —
        // no env mutation here so parallel tests cannot flake).
        assert_eq!(max_lines(), MAX_LINES);
        assert_eq!(max_bytes(), MAX_BYTES);
    }

    #[test]
    fn exactly_filled_window_is_complete_without_peek_hit() {
        // T2.2 core: 2000 lines, file ends there (peek saw EOF) → no cut.
        let lines = wlines(2000);
        let w = window(1, &lines, MAX_LINES, MAX_BYTES, MAX_LINE_CHARS, false);
        assert_eq!(w.cut, None);
        assert_eq!(w.next_offset, None);
        assert_eq!(w.shown.len(), 2000);
        assert_eq!(w.shown[0], "     1\tline 0001");
        assert_eq!(w.shown[1999], "  2000\tline 2000");
        // Same window with one observed line past it → line cut at 2001.
        let w = window(1, &lines, MAX_LINES, MAX_BYTES, MAX_LINE_CHARS, true);
        assert_eq!(w.cut, Some(Cut::Lines));
        assert_eq!(w.next_offset, Some(2001));
        assert_eq!(w.shown.len(), 2000);
    }

    #[test]
    fn over_supplied_window_cuts_without_peek() {
        // More lines observed than the window holds: cut regardless of peek.
        let lines = wlines(2005);
        let w = window(1, &lines, MAX_LINES, MAX_BYTES, MAX_LINE_CHARS, false);
        assert_eq!(w.cut, Some(Cut::Lines));
        assert_eq!(w.next_offset, Some(2001));
        assert_eq!(w.shown.len(), 2000);
    }

    #[test]
    fn byte_cut_stops_before_the_unshown_line() {
        // 300-byte lines with prefixes (~307B): the first line that does not
        // fit is named, never emitted.
        let lines: Vec<WindowLine> = (1..=500)
            .map(|i| WindowLine {
                text: format!("{:<300}", format!("log line {i:04} ")),
                overflow_chars: 0,
            })
            .collect();
        let w = window(1, &lines, MAX_LINES, MAX_BYTES, MAX_LINE_CHARS, false);
        assert_eq!(w.cut, Some(Cut::Bytes));
        let next = w.next_offset.unwrap();
        assert!(w.shown.len() < 500);
        assert_eq!(next, w.shown.len() + 1, "resume ON the unshown line");
        assert!(w.shown[0].starts_with("     1\tlog line 0001"));
        assert_eq!(w.clamped, 0);
    }

    #[test]
    fn byte_budget_exact_fit_is_complete() {
        // Two lines sized so prefixed bytes land exactly on the budget.
        let mk = |s: &str| WindowLine {
            text: s.to_string(),
            overflow_chars: 0,
        };
        let first = prefix_line(1, "a");
        let second = prefix_line(2, "b");
        let budget = first.len() + 1 + second.len();
        let w = window(
            1,
            &[mk("a"), mk("b")],
            MAX_LINES,
            budget,
            MAX_LINE_CHARS,
            false,
        );
        assert_eq!(w.cut, None);
        assert_eq!(w.shown.len(), 2);
        let w = window(
            1,
            &[mk("a"), mk("b")],
            MAX_LINES,
            budget - 1,
            MAX_LINE_CHARS,
            false,
        );
        assert_eq!(w.cut, Some(Cut::Bytes));
        assert_eq!(w.next_offset, Some(2));
        assert_eq!(w.shown.len(), 1);
    }

    #[test]
    fn clamp_counts_shown_lines_only() {
        // A long line past the byte cut is read but unshown: it must not
        // inflate the clamp note about the visible output.
        let long = "y".repeat(3900);
        let lines = vec![
            WindowLine {
                text: "ok".to_string(),
                overflow_chars: 0,
            },
            WindowLine {
                text: long,
                overflow_chars: 0,
            },
        ];
        // Budget fits the first prefixed line but not the clamped second.
        let budget = prefix_line(1, "ok").len();
        let w = window(1, &lines, MAX_LINES, budget, MAX_LINE_CHARS, false);
        assert_eq!(w.cut, Some(Cut::Bytes));
        assert_eq!(w.next_offset, Some(2));
        assert_eq!(w.clamped, 0, "unshown clamped line is not counted");
        // Room for both: the shown clamp counts once.
        let w = window(1, &lines, MAX_LINES, MAX_BYTES, MAX_LINE_CHARS, false);
        assert_eq!(w.cut, None);
        assert_eq!(w.clamped, 1);
        assert!(w.shown[1].ends_with("…[+1900 chars]"));
    }

    #[test]
    fn overflow_chars_keep_the_clamp_marker_exact() {
        // 5000-emoji line: the stream buffers a prefix and counts the rest.
        // Total 5000 chars → marker says +3000.
        let kept_text = "😀".repeat(2000);
        let w = window(
            1,
            &[WindowLine {
                text: format!("{kept_text}�"),
                overflow_chars: 2999,
            }],
            MAX_LINES,
            MAX_BYTES,
            MAX_LINE_CHARS,
            false,
        );
        assert_eq!(w.cut, None);
        assert_eq!(w.clamped, 1);
        assert_eq!(w.shown[0], format!("     1\t{kept_text} …[+3000 chars]"));
        // clamp_counted matches clamp_line when nothing was discarded.
        let (a, ha) = clamp_counted("hello", 0, 2000);
        let (b, hb) = clamp_line("hello", 2000);
        assert_eq!((a, ha), (b, hb));
        let long = "z".repeat(3900);
        let (a, ha) = clamp_counted(&long, 0, 2000);
        let (b, hb) = clamp_line(&long, 2000);
        assert_eq!((a, ha), (b, hb));
    }
}
