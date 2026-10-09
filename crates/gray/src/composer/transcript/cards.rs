//! Transcript cards: tool boxes (split from `transcript`).

use super::*;

pub(crate) fn format_tool_box_lines(
    header: Line<'static>,
    body: &[Line<'static>],
    width: usize,
) -> Vec<Line<'static>> {
    let bg_color = crate::theme::theme().surface_bg;
    let bg_style = Style::default().bg(bg_color);
    let max_w = width.saturating_sub(4).max(1);
    // Body rows carry their own 2-col lead (tool_fmt's `"  "`), so their
    // budget is the header's painted total (`width - 2`), not `max_w`:
    // tool_fmt pre-wraps numbered/diff rows to exactly `width - 2`, and
    // re-wrapping them 2 cols narrower here orphaned each full row's last
    // word ("lines", "5d") onto its own continuation row.
    let body_w = width.saturating_sub(2).max(1);

    // One padding row above and one below, painted edge to edge with real
    // cells: they are the card's own, so the header never touches its top
    // edge. The unpainted blank row that separates the card from its
    // neighbours is `ensure_gap`'s (codex's rule, one row between blocks),
    // and a painted row never counts as that gap (`transcript_row_is_blank`).
    let margin_row = || -> Line<'static> {
        Line::from(Span::styled(" ".repeat(width.max(1)), bg_style)).style(bg_style)
    };

    let mut box_lines: Vec<Line<'static>> = Vec::new();
    box_lines.push(margin_row());

    let wrapped_header = header_rows(header, max_w);
    for mut l in wrapped_header {
        l.style = l.style.patch(bg_style);
        l.spans
            .insert(0, Span::styled(" ".repeat(GUTTER), bg_style));
        for span in l.spans.iter_mut() {
            span.style = span.style.bg(bg_color);
        }
        box_lines.push(l);
    }

    // One breathing-room row between the command header and its output.
    if !body.is_empty() {
        box_lines.push(Line::from("").style(bg_style));
    }

    for line in body {
        let mut line = line.clone();
        for span in line.spans.iter_mut() {
            if span.content.contains('\t') {
                let expanded = crate::tool_fmt::expand_tabs(&span.content);
                *span = Span::styled(expanded, span.style);
            }
        }
        let wrapped_body = wrap_styled_line(line, body_w);
        for mut l in wrapped_body {
            let line_bg = l
                .style
                .bg
                .or_else(|| l.spans.iter().find_map(|s| s.style.bg))
                .unwrap_or(bg_color);

            l.style = l.style.bg(line_bg);
            for span in l.spans.iter_mut() {
                if span.style.bg.is_none() {
                    span.style = span.style.bg(line_bg);
                }
            }
            if line_bg != bg_color {
                // Diff rows (red/green) must run edge-to-edge: Paragraph only
                // paints span cells, so padding to max_w leaves the last
                // `width - max_w` cells in the card bg (dark strip on the
                // right). Same unstyled-cells cause as the footer band.
                let current_w: usize = l.spans.iter().map(|s| s.width()).sum();
                if current_w < width {
                    l.spans.push(Span::styled(
                        " ".repeat(width - current_w),
                        Style::default().bg(line_bg),
                    ));
                }
            }
            box_lines.push(l);
        }
    }
    box_lines.push(margin_row());
    box_lines
}

/// Header rows for a card. The `· in <dir>` / `· in background` detail
/// always gets its own row under the verb, so the path never shares the
/// command's row. Other ` · ` details stay inline and only break onto their
/// own row when the header is too wide for the box.
pub(crate) fn header_rows(header: Line<'static>, max_w: usize) -> Vec<Line<'static>> {
    const SEP: &str = " \u{00b7} ";
    let in_detail = header
        .spans
        .iter()
        .position(|s| s.content.starts_with(" \u{00b7} in "))
        .filter(|&i| i > 0);
    let cut = match in_detail {
        Some(i) => Some(i),
        None if header.width() <= max_w => return vec![header],
        None => header
            .spans
            .iter()
            .position(|s| s.content.starts_with(SEP))
            .filter(|&i| i > 0),
    };
    let Some(cut) = cut else {
        return wrap_styled_line(header, max_w);
    };
    let mut head = header.clone();
    let mut tail_spans = head.spans.split_off(cut);
    // The detail sits under the verb, past the `⬢ ` bullet.
    let indent = "  ";
    let first = &tail_spans[0];
    tail_spans[0] = Span::styled(
        first.content.trim_start_matches(SEP).to_string(),
        first.style,
    );
    let mut rows = wrap_styled_line(head, max_w);
    // Further ` · ` parts after the first ride the same detail row.
    let tail_w = max_w.saturating_sub(indent.len()).max(1);
    let tail = Line::from(tail_spans).style(header.style);
    for mut row in wrap_styled_line(tail, tail_w) {
        row.spans.insert(0, Span::raw(indent));
        rows.push(row);
    }
    rows
}
