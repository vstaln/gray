//! The transcript's one margin rule: exactly one unpainted blank row between
//! blocks, never two, and never a blank row left at the bottom.
//!
//! A gap is *owed* by a block boundary (`Tui::ensure_gap`, or blank rows at a
//! block's edges) and *paid* as the prefix of the next content, never written
//! as a suffix. The scrollback therefore always ends in content, and the
//! band's seam row is the one gap between the transcript and the dock or the
//! input box. Every writer (live streaming, cards, notices, resize reflow,
//! session replay) goes through the same funnel, so they cannot disagree.
//!
//! Blank rows *inside* a block are its content (a code block's empty lines)
//! and pass through untouched.

use gray_markdown::HyperlinkTarget;
use ratatui::text::Line;

use super::transcript_row_is_blank;

/// A block with its edge blanks lifted out: `lead`/`trail` say a boundary was
/// there, `lines` start and end in content (or are empty).
#[derive(Debug, PartialEq)]
pub(crate) struct Shaped {
    pub(crate) lead: bool,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) hyperlinks: Vec<HyperlinkTarget>,
    pub(crate) trail: bool,
}

/// Lifts the blank rows at both edges of a block out into boundary marks,
/// keeping hyperlinks attached to their rows. Pure for testability (`Tui`
/// needs a TTY).
pub(crate) fn shape_block(lines: Vec<Line<'static>>, hyperlinks: Vec<HyperlinkTarget>) -> Shaped {
    let start = lines
        .iter()
        .take_while(|l| transcript_row_is_blank(l))
        .count();
    let end = lines.len()
        - lines[start..]
            .iter()
            .rev()
            .take_while(|l| transcript_row_is_blank(l))
            .count();
    let lead = start > 0;
    let trail = end < lines.len();
    let hyperlinks = hyperlinks
        .into_iter()
        .filter_map(|mut h| {
            (start..end).contains(&h.line_index).then(|| {
                h.line_index -= start;
                h
            })
        })
        .collect();
    let lines = lines[start..end].to_vec();
    Shaped {
        lead,
        lines,
        hyperlinks,
        trail,
    }
}

/// Whether an owed gap is paid in front of the next content: only above a
/// content row. Above nothing (an empty transcript) or above a blank row (the
/// welcome banner's own trailing gap) the margin already exists.
pub(crate) fn gap_due(owed: bool, transcript: &[Line<'static>]) -> bool {
    owed && transcript
        .last()
        .is_some_and(|l| !transcript_row_is_blank(l))
}

/// One block through the funnel: folds its edge blanks into `owed` and says
/// whether one gap row goes in front of `shaped.lines`. The caller writes the
/// gap (when told), then the lines. A block with no content only moves the
/// boundary along. This is the whole margin policy; `Tui` and the tests both
/// drive it, so the tests exercise exactly what runs live.
pub(crate) fn admit(owed: &mut bool, transcript: &[Line<'static>], shaped: &Shaped) -> bool {
    *owed |= shaped.lead;
    if shaped.lines.is_empty() {
        *owed |= shaped.trail;
        return false;
    }
    let gap = gap_due(*owed, transcript);
    *owed = shaped.trail;
    gap
}

/// The longest run of consecutive blank rows where `rows` join `tail`: the
/// tail's trailing blanks plus the new rows' leading ones. The funnel keeps
/// this at one or less; anything more is the doubled margin.
pub(crate) fn junction_blank_run(tail: &[Line<'static>], rows: &[Line<'static>]) -> usize {
    let trailing = tail
        .iter()
        .rev()
        .take_while(|l| transcript_row_is_blank(l))
        .count();
    let leading = rows
        .iter()
        .take_while(|l| transcript_row_is_blank(l))
        .count();
    trailing + leading
}

#[path = "margins_tests.rs"]
#[cfg(test)]
mod tests;
