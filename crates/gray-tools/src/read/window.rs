//! T1.2 — `cat -n` line prefixes for the `read` tool.
//!
//! Pure functions only (no I/O, no env): `ReadTool::execute` numbers the
//! selected window with absolute 1-based line numbers *before* the line/byte
//! caps, so truncation math (`output_lines`, `next_offset`) is unchanged and
//! `truncate_head` still never splits a line.
//!
//! Byte-budget side effect (deliberate, conservative): the ~7-byte prefix
//! counts toward the 50 KiB cap, so a byte-cut may fire a few lines earlier
//! than on raw text. Numbering after the caps would instead overshoot the
//! budget and need per-branch rework in `mod.rs` — bigger diff, same contract.

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
/// stages a copy until the integrator dedups — same value, do not drift).
pub const MAX_LINE_CHARS: usize = 2000;

/// Env override `GRAY_READ_MAX_LINE_CHARS` (positive ints only, else default).
pub fn max_line_chars() -> usize {
    std::env::var("GRAY_READ_MAX_LINE_CHARS")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(MAX_LINE_CHARS)
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
}
