use gray_markdown::HyperlinkTarget;
use ratatui::text::Line;

use super::transcript_row_is_blank;

#[derive(Debug, PartialEq)]
pub(crate) struct Shaped {
    pub(crate) lead: bool,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) hyperlinks: Vec<HyperlinkTarget>,
    pub(crate) trail: bool,
}

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

pub(crate) fn gap_due(owed: bool, transcript: &[Line<'static>]) -> bool {
    owed && transcript
        .last()
        .is_some_and(|l| !transcript_row_is_blank(l))
}

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

pub(crate) fn junction_blank_run(tail: &[Line<'static>], rows: &[Line<'static>]) -> usize {
    if tail.is_empty() {
        return 0;
    }
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
