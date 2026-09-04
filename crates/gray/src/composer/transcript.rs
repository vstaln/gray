use std::collections::HashMap;
use std::io::Write;
use std::ops::Range;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use gray_markdown::HyperlinkTarget;

use super::Tui;

fn thinking_style() -> Style {
    Style::default()
        .fg(Color::Rgb(140, 140, 140))
        .add_modifier(Modifier::ITALIC)
}

/// Left padding, omp-style: routed through the global tight flag.
fn left_pad() -> Span<'static> {
    Span::raw(" ".repeat(crate::tui::padding_x(1)))
}

fn strip_ansi(s: &str) -> String {
    crate::tui::strip_ansi(s)
}

fn str_display_width(s: &str) -> usize {
    crate::tui::visible_width(s)
}

fn slice_line_spans<'a>(
    original: &'a Line<'a>,
    span_bounds: &[(Range<usize>, Style)],
    range: &Range<usize>,
) -> Line<'a> {
    let start_byte = range.start;
    let end_byte = range.end;
    let mut acc: Vec<Span<'a>> = Vec::new();
    for (i, (r, style)) in span_bounds.iter().enumerate() {
        let s = r.start;
        let e = r.end;
        if e <= start_byte { continue; }
        if s >= end_byte { break; }
        let seg_start = start_byte.max(s);
        let seg_end = end_byte.min(e);
        if seg_end > seg_start {
            let local_start = seg_start - s;
            let local_end = seg_end - s;
            let content = original.spans[i].content.as_ref();
            // ensure boundaries are char boundaries (they should be since we only cut at word boundaries which are char boundaries)
            let slice = &content[local_start..local_end];
            acc.push(Span { style: *style, content: std::borrow::Cow::Borrowed(slice) });
        }
        if e >= end_byte { break; }
    }
    Line { style: original.style, alignment: original.alignment, spans: acc }
}

/// Word-aware wrapping: preserves styles, respects hyperlink guards,
/// splits at word boundaries (space) and falls back to char chunk for long words.
pub(crate) fn wrap_styled_line(line: Line<'static>, max_w: usize) -> Vec<Line<'static>> {
    wrap_styled_line_with_ranges(line, max_w).into_iter().map(|(l, _)| l).collect()
}

/// Same as [`wrap_styled_line`] but also returns each output row's source
/// byte range on the original (unwrapped) flat line, so callers can map
/// absolute columns (hyperlinks) onto wrapped rows.
pub(crate) fn wrap_styled_line_with_ranges(line: Line<'static>, max_w: usize) -> Vec<(Line<'static>, Range<usize>)> {
    // Don't wrap lines with OSC 8 hyperlinks
    if line.spans.iter().any(|s| s.content.contains("\x1b]8;;")) {
        return vec![(line, 0..usize::MAX)];
    }
    if line.width() <= max_w {
        return vec![(line, 0..usize::MAX)];
    }
    // Flatten line for word analysis
    let mut flat = String::new();
    let mut span_bounds: Vec<(Range<usize>, Style)> = Vec::new();
    let mut acc = 0usize;
    for s in &line.spans {
        let text = s.content.as_ref();
        let start = acc;
        flat.push_str(text);
        acc += text.len();
        span_bounds.push((start..acc, s.style));
    }
    if flat.is_empty() {
        return vec![(line, 0..usize::MAX)];
    }

    // Detect if this line has a gutter prefix (e.g. " 12 | " or "    | ")
    let gutter_info = if let Some(bar_idx) = flat.find(" | ") {
        if bar_idx <= 14 && flat[..bar_idx].chars().all(|c| c.is_ascii_digit() || c == ' ') {
            let gutter_end = bar_idx + 3;
            let gutter_style = span_bounds
                .iter()
                .find(|(r, _)| r.start <= bar_idx && bar_idx < r.end)
                .map(|(_, s)| *s)
                .unwrap_or(line.style);
            let cont_gutter_str = format!("{:>width$} | ", "", width = bar_idx);
            Some((gutter_end, cont_gutter_str, gutter_style))
        } else {
            None
        }
    } else {
        None
    };

    let (content_start, cont_gutter_str, gutter_style, eff_max_w) = if let Some((g_end, ref c_str, g_style)) = gutter_info {
        let g_width = str_display_width(&flat[..g_end]);
        let avail = max_w.saturating_sub(g_width).max(10);
        (g_end, Some(c_str.clone()), Some(g_style), avail)
    } else {
        (0, None, None, max_w)
    };

    // Tokenize content into words (non-space runs) with byte ranges
    let mut words: Vec<Range<usize>> = Vec::new();
    let mut i = content_start;
    while i < flat.len() {
        while i < flat.len() && flat[i..].starts_with(' ') {
            let ch = flat[i..].chars().next().unwrap();
            i += ch.len_utf8();
        }
        if i >= flat.len() { break; }
        let start = i;
        while i < flat.len() {
            let ch = flat[i..].chars().next().unwrap();
            if ch == ' ' { break; }
            i += ch.len_utf8();
        }
        words.push(start..i);
    }
    if words.is_empty() {
        // column mapping is approximate here; word-split lines are the norm.
        return char_chunk_fallback(line, max_w, flat).into_iter().map(|l| (l, 0..usize::MAX)).collect();
    }

    // Build wrapped ranges word-aware
    let mut out_ranges: Vec<Range<usize>> = Vec::new();
    let mut cur_start: Option<usize> = None;
    let mut cur_end: usize = 0;
    let mut cur_w: usize = 0;
    for w_range in words {
        let word_str = &flat[w_range.clone()];
        let word_w = str_display_width(word_str);
        if word_w > eff_max_w {
            if let Some(s) = cur_start.take() {
                out_ranges.push(s..cur_end);
                cur_w = 0;
            }
            let chars: Vec<char> = word_str.chars().collect();
            let mut byte_offset = w_range.start;
            let mut idx = 0;
            while idx < chars.len() {
                let take = eff_max_w.min(chars.len() - idx);
                let chunk_chars = &chars[idx..idx+take];
                let chunk_str: String = chunk_chars.iter().collect();
                let byte_len = chunk_str.len();
                out_ranges.push(byte_offset..byte_offset+byte_len);
                byte_offset += byte_len;
                idx += take;
            }
            continue;
        }
        if cur_start.is_none() {
            cur_start = Some(w_range.start);
            cur_end = w_range.end;
            cur_w = word_w;
        } else {
            let needed = 1 + word_w;
            if cur_w + needed <= eff_max_w {
                cur_end = w_range.end;
                cur_w += needed;
            } else {
                out_ranges.push(cur_start.take().unwrap()..cur_end);
                cur_start = Some(w_range.start);
                cur_end = w_range.end;
                cur_w = word_w;
            }
        }
    }
    if let Some(s) = cur_start {
        out_ranges.push(s..cur_end);
    }
    if out_ranges.is_empty() {
        return vec![(Line::from("").style(line.style), 0..0)];
    }

    // Convert ranges to Lines preserving styles and prepending gutter
    let mut result: Vec<(Line<'static>, Range<usize>)> = Vec::new();
    for (row_idx, r) in out_ranges.into_iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if let (Some(c_str), Some(g_style)) = (&cont_gutter_str, gutter_style) {
            if row_idx == 0 {
                let g_sliced = slice_line_spans(&line, &span_bounds, &(0..content_start));
                for s in g_sliced.spans {
                    spans.push(Span::styled(s.content.into_owned(), s.style.patch(line.style)));
                }
            } else {
                spans.push(Span::styled(c_str.clone(), g_style.patch(line.style)));
            }
        }
        let sliced = slice_line_spans(&line, &span_bounds, &r);
        for s in sliced.spans {
            spans.push(Span::styled(s.content.into_owned(), s.style.patch(line.style)));
        }
        let mut new_line = Line::from(spans).style(line.style);
        new_line.alignment = line.alignment;
        result.push((new_line, r));
    }
    if result.is_empty() {
        result.push((Line::from("").style(line.style), 0..0));
    }
    result
}

fn char_chunk_fallback(line: Line<'static>, max_w: usize, _flat: String) -> Vec<Line<'static>> {
    let line_style = line.style;
    let mut result = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_w = 0usize;
    for span in line.spans {
        let span_w = span.width();
        if current_w + span_w <= max_w {
            current_w += span_w;
            current_spans.push(span);
        } else {
            let chars: Vec<char> = span.content.chars().collect();
            let style = span.style;
            let mut i = 0;
            while i < chars.len() {
                let avail = max_w.saturating_sub(current_w);
                if avail == 0 {
                    if !current_spans.is_empty() {
                        result.push(Line::from(std::mem::take(&mut current_spans)).style(line_style));
                    }
                    current_w = 0;
                    continue;
                }
                let take = avail.min(chars.len() - i);
                let chunk: String = chars[i..i + take].iter().collect();
                current_spans.push(Span::styled(chunk, style));
                current_w += take;
                i += take;
                if current_w >= max_w {
                    result.push(Line::from(std::mem::take(&mut current_spans)).style(line_style));
                    current_w = 0;
                }
            }
        }
    }
    if !current_spans.is_empty() {
        result.push(Line::from(current_spans).style(line_style));
    }
    if result.is_empty() {
        result.push(Line::from("").style(line_style));
    }
    result
}

pub(crate) fn format_user_prompt_lines(text: &str, attached: &[std::path::PathBuf], width: usize) -> Vec<Line<'static>> {
    let sanitized = crate::tui::sanitize_user_text(text);
    let prompt_color = Color::Rgb(180, 180, 180);
    let text_primary = Color::Rgb(225, 225, 225);
    let dim_color = Color::Rgb(140, 140, 140);
    let bg_style = Style::default().bg(Color::Rgb(22, 22, 22));
    let mut lines = Vec::new();
    lines.push(Line::from("").style(bg_style));
    let arrow_span = Span::styled(" ❯ ", Style::default().fg(prompt_color).add_modifier(Modifier::BOLD));
    let max_w = width.saturating_sub(4).max(1);
    let lines_raw: Vec<&str> = sanitized.split('\n').collect();
    for (i, raw_line) in lines_raw.iter().enumerate() {
        let prefix = if i == 0 { arrow_span.clone() } else { Span::raw("   ") };
        if raw_line.is_empty() {
            lines.push(Line::from(vec![prefix]).style(bg_style));
        } else {
            let chars: Vec<char> = raw_line.chars().collect();
            for (ci, chunk) in chars.chunks(max_w).enumerate() {
                let row_prefix = if ci == 0 { prefix.clone() } else { Span::raw("   ") };
                lines.push(Line::from(vec![
                    row_prefix,
                    Span::styled(chunk.iter().collect::<String>(), Style::default().fg(text_primary)),
                ]).style(bg_style));
            }
        }
    }
    if !attached.is_empty() {
        let names = attached.iter().filter_map(|p| p.file_name().and_then(|n| n.to_str())).collect::<Vec<_>>().join(", ");
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(format!("↳ attached: {names}"), Style::default().fg(dim_color)),
        ]).style(bg_style));
    }
    lines.push(Line::from("").style(bg_style));
    lines
}

// ---------------------------------------------------------------------------
// Tui transcript methods (batch insert_before)
// ---------------------------------------------------------------------------
impl Tui {
    pub(crate) fn ensure_gap(&mut self, n: usize) {
        let trailing = self.transcript.iter().rev().take_while(|l| {
            l.style.bg.is_none() && l.spans.iter().all(|s| s.style.bg.is_none() && s.content.trim().is_empty())
        }).count();
        let need = n.saturating_sub(trailing);
        if need == 0 { return; }
        let lines: Vec<Line<'static>> = (0..need).map(|_| Line::from("")).collect();
        let h = need as u16;
        let _ = self.terminal.insert_before(h, |buf| {
            Paragraph::new(lines.clone()).render(buf.area, buf);
        });
        self.history_entries.push(super::TranscriptEntry::Gap(need));
        self.transcript.extend(lines);
    }

    pub fn stream(&mut self, chunk: &str) {
        let toks = (chunk.chars().count() + 3) / 4;
        self.live_streamed_tokens += toks.max(1);
        self.pending.push_str(&strip_ansi(chunk));
        while let Some(idx) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=idx).collect();
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            if trimmed.is_empty() && self.transcript.last().is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty())) {
                continue;
            }
            let style = if self.thinking { thinking_style() } else { Style::default() };
            self.push_line_styled(trimmed.to_string(), style);
        }
        let _ = self.draw();
    }

    pub fn stream_thinking(&mut self, chunk: &str) {
        self.turn_had_thinking = true;
        let toks = (chunk.chars().count() + 3) / 4;
        self.live_streamed_tokens += toks.max(1);
        if self.hide_thinking {
            let _ = self.draw();
            return;
        }
        if !self.thinking {
            self.ensure_gap(1);
        }
        if self.status.as_ref().map(|s| s.1.as_str()) != Some("Thinking") {
            self.set_status(Some("Thinking"));
        }
        self.thinking = true;
        self.pending.push_str(&strip_ansi(chunk));
        let w = self.width().max(10);
        let max_w = w.saturating_sub(4).max(1);
        while let Some(idx) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=idx).collect();
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            self.push_line_styled(trimmed.to_string(), thinking_style());
        }
        if self.pending.chars().count() >= max_w {
            let chars: Vec<char> = self.pending.chars().collect();
            let line: String = chars[..max_w].iter().collect();
            self.pending = chars[max_w..].iter().collect();
            self.push_line_styled(line, thinking_style());
        }
        let _ = self.draw();
    }

    pub fn set_hide_thinking(&mut self, hide: bool) { self.hide_thinking = hide; }

    pub fn stream_text(&mut self, chunk: &str) {
        let toks = (chunk.chars().count() + 3) / 4;
        self.live_streamed_tokens += toks.max(1);
        self.end_thinking_run(true);
        if self.status.as_ref().map(|s| s.1.as_str()) != Some("Working") {
            self.set_status(Some("Working"));
        }
        let clean = strip_ansi(chunk);
        self.markdown_renderer.push_and_render(&clean, Some(gray_markdown::get_syntect()));
        let frozen_len = self.markdown_renderer.frozen_lines_len();
        if frozen_len > self.committed_markdown_lines {
            if self.committed_markdown_lines == 0 {
                self.ensure_gap(1);
            }
            let view = self.markdown_renderer.view();
            let new_lines: Vec<Line<'static>> = view.lines[self.committed_markdown_lines..frozen_len].to_vec();
            let hyperlinks = view.hyperlinks.to_vec();
            let offset = self.committed_markdown_lines;
            self.committed_markdown_lines = frozen_len;
            self.push_styled_lines_with_hyperlinks(new_lines, &hyperlinks, offset);
        }
        let _ = self.draw();
    }

    pub fn end_thinking(&mut self) {
        self.end_thinking_run(true);
        let _ = self.draw();
    }

    pub(crate) fn end_thinking_run(&mut self, spacer: bool) {
        if !self.thinking && self.pending.is_empty() { return; }
        self.thinking = false;
        if !self.hide_thinking {
            if !self.pending.is_empty() {
                let rest = std::mem::take(&mut self.pending);
                self.push_line_styled(rest, thinking_style());
            }
            if spacer {
                self.ensure_gap(1);
            }
        } else {
            self.pending.clear();
        }
    }

    /// Echoes a submitted prompt as a card. `trailing_gap` leaves one blank
    /// below the card for the breathing room before the next prompt; slash
    /// commands pass false so their `say()` feedback hugs the card instead
    /// (dismissed-modal breathing room is restored by `restore_viewport`).
    pub fn push_user_prompt(&mut self, text: &str, attached: &[std::path::PathBuf], trailing_gap: bool) {
        self.ensure_gap(1);
        let lines = format_user_prompt_lines(text, attached, self.width().max(10));
        let height = lines.len() as u16;
        let block = ratatui::widgets::Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)));
        let _ = self.terminal.insert_before(height, |buf| {
            Paragraph::new(lines.clone()).block(block).render(buf.area, buf);
        });
        self.history_entries.push(super::TranscriptEntry::UserPrompt(text.to_string(), attached.to_vec()));
        self.transcript.extend(lines);
        // Trailing gap after every chat card — command and prompt alike.
        // Handlers that print feedback (say()) treat the gap as idempotent;
        // handlers that print nothing (dismissed modal) still leave breathing
        // room before the next prompt instead of jamming against the card.
        // Slash-command cards skip it (trailing_gap=false): their feedback
        // hugs the card, and restore_viewport() covers the dismissed modal.
        if trailing_gap {
            self.ensure_gap(1);
        }
        if self.transcript.len() > 1000 { self.transcript.drain(0..100); }
        let _ = std::io::stdout().flush();
    }
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
            l.style = l.style.patch(bg_style);
            for span in l.spans.iter_mut() {
                span.style = span.style.bg(bg_color);
            }
            box_lines.push(l);
        }
    }
    box_lines.push(Line::from("").style(bg_style));
    box_lines
}

impl Tui {
    pub fn push_tool_box(&mut self, header: Line<'static>, body: Vec<Line<'static>>) {
        self.ensure_gap(1);
        let w = self.width().max(10);
        let box_lines = format_tool_box_lines(header.clone(), &body, w);
        let height = box_lines.len() as u16;
        let block = ratatui::widgets::Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)));
        let _ = self.terminal.insert_before(height, |buf| {
            Paragraph::new(box_lines.clone()).block(block).render(buf.area, buf);
        });
        self.history_entries.push(super::TranscriptEntry::ToolBox {
            header,
            body,
        });
        self.transcript.extend(box_lines);
        self.ensure_gap(1);
        if self.transcript.len() > 1000 { self.transcript.drain(0..100); }
        let _ = std::io::stdout().flush();
    }

    pub fn push_line(&mut self, line: String) { self.push_line_styled(line, Style::default()); }

    pub(crate) fn push_line_styled(&mut self, line: String, style: Style) {
        let l = Line::from(vec![Span::styled(line, style)]);
        self.push_styled_lines_with_hyperlinks(vec![l], &[], 0);
    }

    pub fn push_line_spans(&mut self, line: Line<'static>) {
        self.push_styled_lines_with_hyperlinks(vec![line], &[], 0);
    }

    pub fn push_styled_lines(&mut self, lines: Vec<Line<'static>>) {
        self.push_styled_lines_with_hyperlinks(lines, &[], 0);
    }

    pub(crate) fn render_and_insert_styled_lines(
        &mut self,
        lines: &[Line<'static>],
        hyperlinks: &[HyperlinkTarget],
        width: usize,
    ) -> Vec<Line<'static>> {
        if lines.is_empty() { return Vec::new(); }
        let max_w = width.saturating_sub(2).max(1);
        let mut by_line: HashMap<usize, Vec<&HyperlinkTarget>> = HashMap::new();
        for h in hyperlinks {
            by_line.entry(h.line_index).or_default().push(h);
        }
        let mut all_wrapped: Vec<(Line<'static>, Vec<HyperlinkTarget>)> = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            let line_hyperlinks = by_line.get(&idx).cloned().unwrap_or_default();
            for (mut l, src_range) in wrap_styled_line_with_ranges(line.clone(), max_w) {
                let hl_owned: Vec<HyperlinkTarget> = if src_range.end == usize::MAX {
                    line_hyperlinks.iter().map(|h| (*h).clone()).collect()
                } else {
                    // Translate each hyperlink's absolute columns into this
                    // row's coordinates; drop parts landing on other rows.
                    line_hyperlinks.iter().filter_map(|h| {
                        let s = h.column_range.start.max(src_range.start);
                        let e = h.column_range.end.min(src_range.end);
                        if s >= e { return None; }
                        let mut hc = (*h).clone();
                        hc.column_range = (s - src_range.start)..(e - src_range.start);
                        Some(hc)
                    }).collect()
                };
                if !l.spans.is_empty() {
                    l.spans.insert(0, left_pad());
                }
                all_wrapped.push((l, hl_owned));
            }
        }
        let total_h = all_wrapped.len() as u16;
        let lines_only: Vec<Line<'static>> = all_wrapped.iter().map(|(l,_)| l.clone()).collect();
        let _ = self.terminal.insert_before(total_h, |buf| {
            let area = buf.area;
            for (i, (line, hls)) in all_wrapped.iter().enumerate() {
                let row_area = ratatui::layout::Rect { x: area.x, y: area.y + i as u16, width: area.width, height: 1 };
                Paragraph::new(line.clone()).render(row_area, buf);
                for h in hls {
                    let pad = crate::tui::padding_x(1);
                    for col in h.column_range.clone() {
                        let padded_col = col + pad;
                        if padded_col >= area.width as usize { continue; }
                        let x = area.x + padded_col as u16;
                        let y = area.y + i as u16;
                        if x >= area.x + area.width || y >= area.y + area.height { continue; }
                        let cell = &mut buf[(x, y)];
                        if cell.symbol().trim().is_empty() { continue; }
                        let sym = cell.symbol().to_string();
                        let new_sym = format!("\x1b]8;;{}\x07{}\x1b]8;;\x07", h.url, sym);
                        cell.set_symbol(&new_sym);
                    }
                }
            }
        });
        lines_only
    }

    pub fn push_styled_lines_with_hyperlinks(
        &mut self,
        lines: Vec<Line<'static>>,
        hyperlinks: &[HyperlinkTarget],
        _line_offset: usize,
    ) {
        if lines.is_empty() { return; }
        let w = self.width().max(10);
        let lines_only = self.render_and_insert_styled_lines(&lines, hyperlinks, w);
        self.history_entries.push(super::TranscriptEntry::StyledLines {
            lines,
            hyperlinks: hyperlinks.to_vec(),
        });
        self.transcript.extend(lines_only);
        if self.transcript.len() > 1000 { self.transcript.drain(0..100); }
        let _ = std::io::stdout().flush();
    }

    pub fn push_dim(&mut self, line: String) {
        let styled = Line::from(vec![
            Span::styled(line, Style::new().add_modifier(Modifier::DIM)),
        ]);
        self.push_styled_lines_with_hyperlinks(vec![styled], &[], 0);
    }

    pub fn push_action(&mut self, text: &str, detail: Option<&str>) {
        let mut spans = vec![
            Span::styled("✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD)),
            Span::styled(text.to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ];
        if let Some(d) = detail {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(d.to_string(), Style::default().fg(Color::Rgb(140, 140, 140))));
        }
        let line = Line::from(spans);
        self.push_styled_lines_with_hyperlinks(vec![line], &[], 0);
    }

    /// Re-renders a `request_user_input` Q&A from stored history so resumed
    /// sessions show what was asked and answered (previously skipped).
    pub fn push_question_replay(&mut self, args: Option<&serde_json::Value>, content: &str) {
        let questions = args
            .and_then(|a| a.get("questions"))
            .and_then(|q| q.as_array())
            .cloned()
            .unwrap_or_default();
        // ToolResult content is `{"answers":{id:{"answers":[...]}}}`.
        let answer_map: HashMap<String, Vec<String>> = serde_json::from_str::<serde_json::Value>(content)
            .ok()
            .and_then(|v| v.get("answers").cloned())
            .and_then(|v| v.as_object().cloned())
            .map(|obj| {
                obj.into_iter()
                    .map(|(k, v)| {
                        let list = v
                            .get("answers")
                            .and_then(|a| a.as_array())
                            .map(|arr| {
                                arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()
                            })
                            .unwrap_or_default();
                        (k, list)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut lines = Vec::new();
        for q in &questions {
            let id = q.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let text = q.get("question").and_then(|v| v.as_str()).unwrap_or("question");
            match answer_map.get(id) {
                Some(list) if !list.is_empty() => {
                    lines.push(format!("• {text}"));
                    lines.push(format!("  {} answered: {}", "↳", list.join(" · ")));
                }
                // Stacked like answered: outcome on its own row, never inline.
                _ => {
                    lines.push(format!("• {text}"));
                    lines.push("  ↳ skipped".to_string());
                }
            }
        }
        if !lines.is_empty() {
            self.push_dim(lines.join("\n"));
        }
    }

    /// Replays a previous session's message history into the TUI scrollback.
    pub fn replay_session_history(&mut self, entries: &[gray_session::SessionEntry], cwd: &std::path::Path) {
        let mut tool_calls: HashMap<String, (String, serde_json::Value)> = HashMap::new();
        for entry in entries {
            match entry.message.role {
                gray_core::Role::User => {
                    let mut user_text = String::new();
                    for block in &entry.message.content {
                        match block {
                            gray_core::ContentBlock::Text { text } => {
                                if !user_text.is_empty() { user_text.push('\n'); }
                                user_text.push_str(text);
                            }
                            gray_core::ContentBlock::ToolResult { id, content, is_error } => {
                                let (name, args) = tool_calls.remove(id).map(|(n, a)| (n, Some(a))).unwrap_or_else(|| ("tool".to_string(), None));
                                if name == "request_user_input" {
                                    self.push_question_replay(args.as_ref(), content);
                                } else {
                                    let header = args.as_ref().map(|a| crate::tool_fmt::format_tool_call_header(&name, a, Some(cwd))).unwrap_or_else(|| ratatui::text::Line::from(name.clone()));
                                    let lines = crate::tool_fmt::format_tool_result_lines_with_context(&name, args.as_ref(), content, *is_error, Some(cwd));
                                    self.push_tool_box(header, lines);
                                }
                            }
                            _ => {}
                        }
                    }
                    if !user_text.is_empty() {
                        self.push_user_prompt(&user_text, &[], true);
                        // Feed composer input history so Up/Down recall works
                        // for prompts from the resumed session.
                        self.history.push(user_text.clone());
                        if self.history.len() > 100 { self.history.remove(0); }
                        self.history_idx = None;
                    }
                }
                gray_core::Role::Assistant => {
                    for block in &entry.message.content {
                        match block {
                            gray_core::ContentBlock::Thinking { .. } => {}
                            gray_core::ContentBlock::Text { text } => {
                                let clean = strip_ansi(text);
                                if !clean.trim().is_empty() {
                                    self.ensure_gap(1);
                                    let (output, _) = gray_markdown::render_markdown_ratatui_full(&clean, gray_markdown::gray_markdown_style(), true, Some(gray_markdown::get_syntect()));
                                    self.push_styled_lines_with_hyperlinks(output.lines, &output.hyperlinks, 0);
                                }
                            }
                            gray_core::ContentBlock::ToolUse { id, name, args } => {
                                tool_calls.insert(id.clone(), (name.clone(), args.clone()));
                            }
                            _ => {}
                        }
                    }
                }
                gray_core::Role::System => {
                    for block in &entry.message.content {
                        if let gray_core::ContentBlock::ToolResult { id, content, is_error } = block {
                            let (name, args) = tool_calls.remove(id).map(|(n, a)| (n, Some(a))).unwrap_or_else(|| ("tool".to_string(), None));
                            if name == "request_user_input" {
                                self.push_question_replay(args.as_ref(), content);
                            } else {
                                let header = args.as_ref().map(|a| crate::tool_fmt::format_tool_call_header(&name, a, Some(cwd))).unwrap_or_else(|| ratatui::text::Line::from(name.clone()));
                                let lines = crate::tool_fmt::format_tool_result_lines_with_context(&name, args.as_ref(), content, *is_error, Some(cwd));
                                self.push_tool_box(header, lines);
                            }
                        }
                    }
                }
            }
        }
        for (_id, (name, args)) in tool_calls {
            if name == "request_user_input" {
                self.push_question_replay(Some(&args), "{}");
                continue;
            }
            let header = crate::tool_fmt::format_tool_call_header(&name, &args, Some(cwd));
            self.push_tool_box(header, Vec::new());
        }
        if let Some(last_usage) = entries.iter().rev().find_map(|e| e.usage) {
            self.set_usage(last_usage);
        }
        // pi-style: seam gap provided by viewport box padding, not transcript trailing blank
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn repro_screenshot_wrap() {
        let text = "Invoking the conversation-start skill to establish skill discovery before responding to the poetry request. Evaluating whether to invoke the brainstorming or writing skill for the creative request.";
        for max_w in [60usize, 80, 100, 120, 148] {
            let line = Line::from(vec![Span::raw(text.to_string())]);
            let out = wrap_styled_line_with_ranges(line, max_w);
            eprintln!("=== max_w={max_w} rows={} ===", out.len());
            for (l, _) in &out {
                let row: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                eprintln!("  [{:>3}] {:?}", unicode_width::UnicodeWidthStr::width(row.as_str()), row);
            }
        }
    }

    #[test]
    fn wrap_ranges_round_trip_and_identity() {
        // identity: short line maps to the whole source
        let short = Line::from(vec![Span::raw("hello world")]);
        let out = wrap_styled_line_with_ranges(short, 20);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.end, usize::MAX);

        // long line: rows fit max_w and each row's text equals the source slice
        let text = "the quick brown fox jumps over the lazy dog again and again until it wraps somewhere";
        let long = Line::from(vec![Span::raw(text.to_string())]);
        let max_w = 24;
        let out = wrap_styled_line_with_ranges(long, max_w);
        assert!(out.len() > 1);
        let mut prev_end = 0usize;
        for (l, r) in &out {
            let row_text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(row_text, &text[r.clone()], "row text must equal its source slice");
            assert!(r.start >= prev_end, "ranges ascend without overlap");
            prev_end = r.end;
        }
    }
}

