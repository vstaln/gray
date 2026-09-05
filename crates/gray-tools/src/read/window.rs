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
}
