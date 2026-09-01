use std::collections::HashMap;
use std::io::Write;
use std::ops::Range;

use ratatui::style::{Color, Modifier, Style, Stylize};
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

/// Word-aware wrapping: preserves styles, respects has_bg and hyperlink guards,
/// splits at word boundaries (space) and falls back to char chunk for long words.
pub(crate) fn wrap_styled_line(line: Line<'static>, max_w: usize) -> Vec<Line<'static>> {
    // Don't wrap diff lines with background
    let has_bg = line.style.bg.is_some() || line.spans.iter().any(|s| s.style.bg.is_some());
    if has_bg {
        return vec![line];
    }
    // Don't wrap lines with OSC 8 hyperlinks
    if line.spans.iter().any(|s| s.content.contains("\x1b]8;;")) {
        return vec![line];
    }
    if line.width() <= max_w {
        return vec![line];
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
        return vec![line];
    }
    // Tokenize into words (non-space runs) with byte ranges
    let mut words: Vec<Range<usize>> = Vec::new();
    let mut i = 0usize;
    while i < flat.len() {
        // skip spaces
        while i < flat.len() && flat[i..].starts_with(' ') {
            // find next char boundary
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
        // line is all spaces? fallback to original char chunk logic
        return char_chunk_fallback(line, max_w, span_bounds, flat);
    }
    // Build wrapped ranges word-aware
    let mut out_ranges: Vec<Range<usize>> = Vec::new();
    let mut cur_start: Option<usize> = None;
    let mut cur_end: usize = 0;
    let mut cur_w: usize = 0;
    for w_range in words {
        let word_str = &flat[w_range.clone()];
        let word_w = str_display_width(word_str);
        if word_w > max_w {
            // flush current line first
            if let Some(s) = cur_start.take() {
                out_ranges.push(s..cur_end);
                cur_w = 0;
            }
            // split long word into chunks of max_w
            let chars: Vec<char> = word_str.chars().collect();
            let mut byte_offset = w_range.start;
            let mut idx = 0;
            while idx < chars.len() {
                let take = max_w.min(chars.len() - idx);
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
            // need space + word
            let needed = 1 + word_w;
            if cur_w + needed <= max_w {
                cur_end = w_range.end;
                cur_w += needed;
            } else {
                // finish current line
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
        return vec![Line::from("").style(line.style)];
    }
    // Convert ranges to Lines preserving styles
    let mut result: Vec<Line<'static>> = Vec::new();
    for r in out_ranges {
        let sliced = slice_line_spans(&line, &span_bounds, &r);
        // patch with line style
        let mut spans: Vec<Span<'static>> = Vec::new();
        for s in sliced.spans {
            // owned conversion
            let owned_content = s.content.into_owned();
            spans.push(Span::styled(owned_content, s.style.patch(line.style)));
        }
        let mut new_line = Line::from(spans).style(line.style);
        new_line.alignment = line.alignment;
        result.push(new_line);
    }
    if result.is_empty() {
        result.push(Line::from("").style(line.style));
    }
    result
}

fn char_chunk_fallback(line: Line<'static>, max_w: usize, span_bounds: Vec<(Range<usize>, Style)>, _flat: String) -> Vec<Line<'static>> {
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

pub(crate) fn format_user_prompt_lines(text: &str) -> Vec<Line<'static>> {
    let sanitized = crate::tui::sanitize_user_text(text);
    let prompt_color = Color::Rgb(180, 180, 180);
    let text_primary = Color::Rgb(225, 225, 225);
    let bg_style = Style::default().bg(Color::Rgb(22, 22, 22));
    let mut lines = Vec::new();
    lines.push(Line::from("").style(bg_style));
    let arrow_span = Span::styled(" ❯ ", Style::default().fg(prompt_color).add_modifier(Modifier::BOLD));
    let lines_raw: Vec<&str> = sanitized.split('\n').collect();
    for (i, raw_line) in lines_raw.iter().enumerate() {
        let prefix = if i == 0 { arrow_span.clone() } else { Span::raw("   ") };
        if raw_line.is_empty() {
            lines.push(Line::from(vec![prefix]).style(bg_style));
        } else {
            lines.push(Line::from(vec![
                prefix,
                Span::styled((*raw_line).to_string(), Style::default().fg(text_primary)),
            ]).style(bg_style));
        }
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
        self.history_entries.push(super::TranscriptEntry::Lines(lines.clone()));
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

    pub fn push_user_prompt(&mut self, text: &str) {
        self.ensure_gap(1);
        let lines = format_user_prompt_lines(text);
        let height = lines.len() as u16;
        let block = ratatui::widgets::Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)));
        let _ = self.terminal.insert_before(height, |buf| {
            Paragraph::new(lines.clone()).block(block).render(buf.area, buf);
        });
        self.history_entries.push(super::TranscriptEntry::UserPrompt(text.to_string()));
        self.transcript.extend(lines);
        if self.transcript.len() > 1000 { self.transcript.drain(0..100); }
        let _ = std::io::stdout().flush();
    }

    pub fn push_tool_box(&mut self, mut header: Line<'static>, body: Vec<Line<'static>>) {
        self.ensure_gap(1);
        let bg_color = Color::Rgb(22, 22, 22);
        let bg_style = Style::default().bg(bg_color);
        let w = self.width().max(10);
        let max_w = w.saturating_sub(2).max(1);

        header.style = header.style.patch(bg_style);
        for span in header.spans.iter_mut() {
            span.style = span.style.bg(bg_color);
        }

        let mut box_lines: Vec<Line<'static>> = Vec::new();
        box_lines.push(Line::from("").style(bg_style));

        let wrapped_header = wrap_styled_line(header, max_w);
        for mut l in wrapped_header {
            l.style = l.style.patch(bg_style);
            for span in l.spans.iter_mut() {
                span.style = span.style.bg(bg_color);
            }
            box_lines.push(l);
        }

        for line in body {
            let mut line = line;
            for span in line.spans.iter_mut() {
                if span.content.contains('\t') {
                    let expanded = crate::tool_fmt::expand_tabs(&span.content, 4);
                    *span = Span::styled(expanded, span.style.bg(bg_color));
                } else {
                    span.style = span.style.bg(bg_color);
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
        let height = box_lines.len() as u16;
        let block = ratatui::widgets::Block::default().style(bg_style);
        let _ = self.terminal.insert_before(height, |buf| {
            Paragraph::new(box_lines.clone()).block(block).render(buf.area, buf);
        });
        self.history_entries.push(super::TranscriptEntry::ToolBox(box_lines.clone()));
        self.transcript.extend(box_lines);
        self.ensure_gap(1);
        if self.transcript.len() > 1000 { self.transcript.drain(0..100); }
        let _ = std::io::stdout().flush();
    }

    pub fn push_line(&mut self, line: String) { self.push_line_styled(line, Style::default()); }

    pub(crate) fn push_line_styled(&mut self, line: String, style: Style) {
        let w = self.width().max(10);
        let max_w = w.saturating_sub(2).max(1);
        let chars: Vec<char> = line.chars().collect();
        let mut lines: Vec<Line<'static>> = Vec::new();
        if chars.is_empty() {
            lines.push(Line::from(""));
        } else {
            for chunk in chars.chunks(max_w) {
                let text: String = chunk.iter().collect();
                lines.push(Line::from(vec![
                    left_pad(),
                    Span::styled(text, style),
                ]));
            }
        }
        let h = lines.len() as u16;
        self.history_entries.push(super::TranscriptEntry::Lines(lines.clone()));
        self.transcript.extend(lines.clone());
        if self.transcript.len() > 1000 { self.transcript.drain(0..100); }
        let _ = self.terminal.insert_before(h, |buf| {
            Paragraph::new(lines.clone()).render(buf.area, buf);
        });
        let _ = std::io::stdout().flush();
    }

    pub fn push_line_spans(&mut self, line: Line<'static>) {
        let w = self.width().max(10);
        let max_w = w.saturating_sub(2).max(1);
        let wrapped = wrap_styled_line(line, max_w);
        let mut padded_wrapped = Vec::with_capacity(wrapped.len());
        for mut l in wrapped {
            if !l.spans.is_empty() {
                l.spans.insert(0, left_pad());
            }
            padded_wrapped.push(l);
        }
        let h = padded_wrapped.len() as u16;
        self.history_entries.push(super::TranscriptEntry::Lines(padded_wrapped.clone()));
        self.transcript.extend(padded_wrapped.clone());
        if self.transcript.len() > 1000 { self.transcript.drain(0..100); }
        let _ = self.terminal.insert_before(h, |buf| {
            Paragraph::new(padded_wrapped.clone()).render(buf.area, buf);
        });
        let _ = std::io::stdout().flush();
    }

    pub fn push_styled_lines(&mut self, lines: Vec<Line<'static>>) {
        self.push_styled_lines_with_hyperlinks(lines, &[], 0);
    }

    pub fn push_styled_lines_with_hyperlinks(
        &mut self,
        lines: Vec<Line<'static>>,
        hyperlinks: &[HyperlinkTarget],
        line_offset: usize,
    ) {
        if lines.is_empty() { return; }
        let w = self.width().max(10);
        let max_w = w.saturating_sub(2).max(1);
        let mut by_line: HashMap<usize, Vec<&HyperlinkTarget>> = HashMap::new();
        for h in hyperlinks {
            by_line.entry(h.line_index).or_default().push(h);
        }
        // Build all wrapped lines first plus per-line hyperlink info
        let mut all_wrapped: Vec<(Line<'static>, Vec<HyperlinkTarget>)> = Vec::new();
        for (idx, line) in lines.into_iter().enumerate() {
            let line_idx = line_offset + idx;
            let line_hyperlinks = by_line.get(&line_idx).cloned().unwrap_or_default();
            let wrapped = if !line_hyperlinks.is_empty() { vec![line] } else { wrap_styled_line(line, max_w) };
            for mut l in wrapped {
                let hl_owned: Vec<HyperlinkTarget> = line_hyperlinks.iter().map(|h| (*h).clone()).collect();
                if !l.spans.is_empty() {
                    l.spans.insert(0, left_pad());
                }
                all_wrapped.push((l, hl_owned));
            }
        }
        let total_h = all_wrapped.len() as u16;
        let lines_only: Vec<Line<'static>> = all_wrapped.iter().map(|(l,_)| l.clone()).collect();
        self.history_entries.push(super::TranscriptEntry::Lines(lines_only.clone()));
        self.transcript.extend(lines_only.clone());
        if self.transcript.len() > 1000 { self.transcript.drain(0..100); }
        // Batch insert with hyperlink OSC injection per row
        let _ = self.terminal.insert_before(total_h, |buf| {
            let area = buf.area;
            for (i, (line, hls)) in all_wrapped.iter().enumerate() {
                let row_area = ratatui::layout::Rect { x: area.x, y: area.y + i as u16, width: area.width, height: 1 };
                // render line at row
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
        let _ = std::io::stdout().flush();
    }

    pub fn push_dim(&mut self, line: String) {
        let styled = Line::from(vec![
            left_pad(),
            Span::styled(line, Style::new().add_modifier(Modifier::DIM)),
        ]);
        self.history_entries.push(super::TranscriptEntry::Lines(vec![styled.clone()]));
        self.transcript.push(styled.clone());
        if self.transcript.len() > 1000 { self.transcript.drain(0..100); }
        let _ = self.terminal.insert_before(1, |buf| {
            Paragraph::new(styled).render(buf.area, buf);
        });
        let _ = std::io::stdout().flush();
    }

    pub fn push_action(&mut self, text: &str, detail: Option<&str>) {
        let mut spans = vec![
            Span::raw(" "),
            Span::styled("✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD)),
            Span::styled(text.to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ];
        if let Some(d) = detail {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(d.to_string(), Style::default().fg(Color::Rgb(140, 140, 140))));
        }
        let line = Line::from(spans);
        self.history_entries.push(super::TranscriptEntry::Lines(vec![line.clone()]));
        self.transcript.push(line.clone());
        if self.transcript.len() > 1000 { self.transcript.drain(0..100); }
        let _ = self.terminal.insert_before(1, |buf| {
            Paragraph::new(line).render(buf.area, buf);
        });
        let _ = std::io::stdout().flush();
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
                                let (name, args) = tool_calls.get(id).map(|(n, a)| (n.as_str(), Some(a))).unwrap_or(("tool", None));
                                let lines = crate::tool_fmt::format_tool_result_lines_with_context(name, args, content, *is_error, Some(cwd));
                                if !lines.is_empty() { self.push_styled_lines(lines); }
                            }
                            _ => {}
                        }
                    }
                    if !user_text.is_empty() { self.push_user_prompt(&user_text); }
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
                                let header = crate::tool_fmt::format_tool_call_header(name, args, Some(cwd));
                                self.ensure_gap(1);
                                self.push_line_spans(header);
                            }
                            _ => {}
                        }
                    }
                }
                gray_core::Role::System => {
                    for block in &entry.message.content {
                        if let gray_core::ContentBlock::ToolResult { id, content, is_error } = block {
                            let (name, args) = tool_calls.get(id).map(|(n, a)| (n.as_str(), Some(a))).unwrap_or(("tool", None));
                            let lines = crate::tool_fmt::format_tool_result_lines_with_context(name, args, content, *is_error, Some(cwd));
                            if !lines.is_empty() { self.push_styled_lines(lines); }
                        }
                    }
                }
            }
        }
        if let Some(last_usage) = entries.iter().rev().find_map(|e| e.usage) {
            self.set_usage(last_usage);
        }
        // pi-style: seam gap provided by viewport box padding, not transcript trailing blank
    }
}

