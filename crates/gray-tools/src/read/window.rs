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

/// Spec-fixed per-line ceiling in chars (canonical home; `stream.rs` uses it
/// for its byte cap — same value, do not drift).
pub const MAX_LINE_CHARS: usize = 2000;

/// Spec-fixed window ceilings (T1.1): at most this many lines and this many
/// output bytes (prefixes included) per read.
pub const MAX_LINES: usize = 2000;
pub const MAX_BYTES: usize = 50 * 1024;

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
/// `line`). Returns the (possibly clamped) line + hit flag.
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

#[path = "window_tests.rs"]
#[cfg(test)]
mod tests;
