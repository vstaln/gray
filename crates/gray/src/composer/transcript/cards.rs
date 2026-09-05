//! Transcript cards: gateway boot + tool boxes (split from `transcript`).

use super::*;

/// Shared gateway boot card content: single source for the live viewport
/// panel and the committed final card so `Gateway started` /
/// `Gateway autostarted` never drift (same header style, same dim rows,
/// same two-space indent from [`crate::repl::gateway_boot_rows`]).
pub(crate) fn gateway_boot_dim() -> Style {
    Style::default().fg(Color::Rgb(140, 140, 140))
}

pub(crate) fn gateway_boot_card_parts(
    header: &str,
    rows: &[String],
) -> (Line<'static>, Vec<Line<'static>>) {
    let header = Line::from(Span::styled(
        header.to_string(),
        Style::default()
            .fg(Color::Rgb(225, 225, 225))
            .add_modifier(Modifier::BOLD),
    ));
    let dim = gateway_boot_dim();
    let body = rows
        .iter()
        .map(|r| Line::from(Span::styled(r.clone(), dim)))
        .collect();
    (header, body)
}

/// Card surface color shared by tool boxes, prompt echoes, the input band
/// and the gateway boot card.
pub(crate) const CARD_BG: Color = Color::Rgb(22, 22, 22);

/// Bakes the card bg into every cell of one row: patches the line style and
/// every span that has no bg of its own, then pads to `width` with bg-filled
/// spaces. Rows that carry their own bg (diff red/green) keep it. After this
/// a row looks the same whether ratatui paints it through `insert_before`
/// or through the inline viewport's frame buffer — no reliance on
/// `Line`/`Block` style inheritance, which the inline viewport drops.
pub(crate) fn pad_card_row(mut line: Line<'static>, width: usize) -> Line<'static> {
    let bg_style = Style::default().bg(CARD_BG);
    line.style = line.style.patch(bg_style);
    for span in line.spans.iter_mut() {
        if span.style.bg.is_none() {
            span.style = span.style.bg(CARD_BG);
        }
    }
    let used: usize = line.spans.iter().map(|s| s.width()).sum();
    if used < width {
        line.spans
            .push(Span::styled(" ".repeat(width - used), bg_style));
    }
    line
}

/// Paints card rows into `area`: floods the whole area with the card bg
/// first (so clipped or short rows never leak terminal bg), then lays the
/// rows on top. The ONE painter for both surfaces — `insert_before` for the
/// committed card and `frame.buffer_mut()` for the live viewport panel.
pub(crate) fn paint_card(
    lines: &[Line<'static>],
    area: ratatui::layout::Rect,
    buf: &mut ratatui::buffer::Buffer,
) {
    let area = area.intersection(buf.area);
    if area.width == 0 || area.height == 0 {
        return;
    }
    buf.set_style(area, Style::default().bg(CARD_BG));
    Paragraph::new(lines.to_vec()).render(area, buf);
}

/// Tight gateway boot card: top margin, header with ONE leading space, rows
/// directly below (no breathing row), bottom margin. Every row is padded to
/// the full `width` with the card bg baked in — see [`pad_card_row`] — so
/// the live panel and the committed card are byte-for-byte the same block.
pub(crate) fn format_gateway_boot_card(
    header: Line<'static>,
    body: &[Line<'static>],
    width: usize,
) -> Vec<Line<'static>> {
    let max_w = width.saturating_sub(4).max(1);
    let mut rows: Vec<Line<'static>> = Vec::new();
    rows.push(Line::from(""));
    for mut l in wrap_styled_line(header, max_w) {
        l.spans.insert(0, Span::raw(" "));
        rows.push(l);
    }
    for line in body {
        let mut line = line.clone();
        for span in line.spans.iter_mut() {
            if span.content.contains('\t') {
                let expanded = crate::tool_fmt::expand_tabs(&span.content, 4);
                *span = Span::styled(expanded, span.style);
            }
        }
        rows.extend(wrap_styled_line(line, max_w));
    }
    rows.push(Line::from(""));
    rows.into_iter().map(|l| pad_card_row(l, width)).collect()
}

/// True for the two gateway boot headers, so resize reflow can re-render
/// the tight boot card instead of the generic tool box.
pub(crate) fn is_gateway_boot_header(header: &Line<'static>) -> bool {
    let text: String = header.spans.iter().map(|s| s.content.as_ref()).collect();
    text == "Gateway started" || text == "Gateway autostarted"
}

pub(crate) fn format_tool_box_lines(
    header: Line<'static>,
    body: &[Line<'static>],
    width: usize,
) -> Vec<Line<'static>> {
    let bg_color = Color::Rgb(22, 22, 22);
    let bg_style = Style::default().bg(bg_color);
    let max_w = width.saturating_sub(4).max(1);

    let mut box_lines: Vec<Line<'static>> = Vec::new();
    box_lines.push(Line::from("").style(bg_style));

    let wrapped_header = wrap_styled_line(header, max_w);
    for mut l in wrapped_header {
        l.style = l.style.patch(bg_style);
        l.spans.insert(0, Span::styled("  ", bg_style));
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
                let expanded = crate::tool_fmt::expand_tabs(&span.content, 4);
                *span = Span::styled(expanded, span.style);
            }
        }
        let wrapped_body = wrap_styled_line(line, max_w);
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
    box_lines.push(Line::from("").style(bg_style));
    box_lines
}
