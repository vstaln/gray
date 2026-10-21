use std::collections::HashMap;
use std::io::Write;
use std::ops::Range;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use gray_markdown::HyperlinkTarget;

use crate::text_width::display_width;

use super::Tui;

mod boxes;
mod cards;
mod margins;
pub(crate) mod rows;

pub(crate) use crate::tui::strip_ansi;
pub(crate) use boxes::render_markdown_lines;
pub(crate) use cards::{format_tool_box_lines, format_tool_box_lines_multi};
pub(crate) use rows::{
    GUTTER, format_user_prompt_lines, left_pad, prose_width, thinking_paint_text,
    thinking_replay_lines, thinking_run_rows, thinking_style, transcript_row_is_blank,
    word_flush_cut, wrap_styled_line, wrap_styled_line_with_ranges,
};

pub(crate) fn paint_insert_rows(
    buf: &mut ratatui::buffer::Buffer,
    rows: &[crate::composer::BatchedInsert],
) {
    let area = buf.area;
    for (i, (line, hls, bg)) in rows.iter().enumerate() {
        let row_area = ratatui::layout::Rect {
            x: area.x,
            y: area.y + i as u16,
            width: area.width,
            height: 1,
        };
        let mut p = Paragraph::new(line.clone());
        if let Some(bg) = bg {
            p = p.style(Style::default().bg(*bg));
        }
        p.render(row_area, buf);
        for h in hls {
            for col in h.column_range.clone() {
                let padded_col = col + GUTTER;
                if padded_col >= area.width as usize {
                    continue;
                }
                let x = area.x + padded_col as u16;
                let y = area.y + i as u16;
                if x >= area.x + area.width || y >= area.y + area.height {
                    continue;
                }
                let cell = &mut buf[(x, y)];
                if cell.symbol().trim().is_empty() {
                    continue;
                }
                let sym = cell.symbol().to_string();
                let new_sym = format!("\x1b]8;;{}\x07{}\x1b]8;;\x07", h.url, sym);
                cell.set_symbol(&new_sym);
            }
        }
    }
}

pub(crate) fn cap_history_entries(entries: &mut Vec<super::TranscriptEntry>) {
    if entries.len() > 1000 {
        entries.drain(0..100);
    }
}

pub(crate) fn drop_mascot_entry(entries: &mut Vec<super::TranscriptEntry>) -> bool {
    match entries
        .iter()
        .rposition(|e| matches!(e, super::TranscriptEntry::Mascot))
    {
        Some(i) => {
            entries.remove(i);
            true
        }
        None => false,
    }
}

pub(crate) fn layout_history(
    entries: &[super::TranscriptEntry],
    w: usize,
    mascot_h: u16,
) -> Vec<Line<'static>> {
    use super::TranscriptEntry;
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut owed = false;
    let place = |out: &mut Vec<Line<'static>>, owed: &mut bool, rows: Vec<Line<'static>>| {
        let shaped = margins::shape_block(rows, Vec::new());
        if margins::admit(owed, out, &shaped) {
            out.push(Line::default());
        }
        out.extend(shaped.lines);
    };
    for entry in entries {
        match entry {
            TranscriptEntry::Welcome => out.extend(super::build_welcome_lines(w)),
            TranscriptEntry::UserPrompt(text, attached) => {
                place(
                    &mut out,
                    &mut owed,
                    format_user_prompt_lines(text, attached, w),
                );
            }
            TranscriptEntry::ToolBox { header, body } => {
                place(
                    &mut out,
                    &mut owed,
                    format_tool_box_lines(header.clone(), body, w),
                );
            }
            TranscriptEntry::ToolBoxLines { header, body } => {
                place(
                    &mut out,
                    &mut owed,
                    format_tool_box_lines_multi(header, body, w),
                );
            }
            TranscriptEntry::StyledLines { lines, .. } => place(&mut out, &mut owed, lines.clone()),
            TranscriptEntry::ThinkingRun(text) => {
                place(
                    &mut out,
                    &mut owed,
                    thinking_run_rows(text, prose_width(w), false),
                );
            }
            TranscriptEntry::Mascot => {
                let art = crate::mascot::mascot_lines(
                    u16::try_from(w).unwrap_or(u16::MAX),
                    mascot_h,
                    Some(w),
                );
                place(&mut out, &mut owed, art.unwrap_or_default());
            }
            TranscriptEntry::Gap(_) => owed = true,
        }
    }
    out
}

pub(crate) fn attach_stream_punctuation(
    entries: &mut Vec<super::TranscriptEntry>,
    target: Option<usize>,
    suffix: &str,
) -> Option<usize> {
    if suffix.is_empty() {
        return target;
    }

    let append_to_lines = |lines: &mut Vec<Line<'static>>| {
        let Some(line) = lines
            .iter_mut()
            .rev()
            .find(|line| !transcript_row_is_blank(line))
        else {
            return false;
        };
        if let Some(span) = line.spans.last_mut() {
            let mut content = span.content.to_string();
            content.push_str(suffix);
            span.content = content.into();
        } else {
            line.spans.push(Span::raw(suffix.to_string()));
        }
        true
    };

    if let Some(index) = target {
        if let Some(super::TranscriptEntry::StyledLines { lines, .. }) = entries.get_mut(index)
            && append_to_lines(lines)
        {
            return Some(index);
        }

        entries.push(super::TranscriptEntry::StyledLines {
            lines: vec![Line::from(suffix.to_string())],
            hyperlinks: Vec::new(),
        });
        cap_history_entries(entries);
        return None;
    }

    entries.push(super::TranscriptEntry::StyledLines {
        lines: vec![Line::from(suffix.to_string())],
        hyperlinks: Vec::new(),
    });
    cap_history_entries(entries);
    None
}

pub(crate) fn is_orphan_stream_punctuation(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && text.chars().all(|c| {
            matches!(
                c,
                '.' | ','
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '…'
                    | '—'
                    | '–'
                    | '"'
                    | '\''
                    | '“'
                    | '”'
                    | '‘'
                    | '’'
            )
        })
}

pub(crate) fn should_drop_stream_punctuation(round_boundary: bool, text: &str) -> bool {
    round_boundary && is_orphan_stream_punctuation(text)
}

pub(crate) fn should_hold_stream_chunk(hold_len: usize, text: &str) -> bool {
    hold_len > 0 || is_orphan_stream_punctuation(text)
}

impl Tui {
    pub(crate) fn ensure_gap(&mut self) {
        if !self.gap_owed {
            self.history_entries.push(super::TranscriptEntry::Gap(1));
            cap_history_entries(&mut self.history_entries);
        }
        self.gap_owed = true;
    }

    pub(crate) fn pay_gap(&mut self) {
        if margins::gap_due(self.gap_owed, &self.transcript) {
            self.write_gap();
        }
        self.gap_owed = false;
    }

    fn write_gap(&mut self) {
        let blank = vec![Line::default()];
        self.insert_paragraph(&blank, Some(crate::theme::theme().window_bg));
        self.record_rows(blank);
    }

    pub(crate) fn record_rows(&mut self, rows: Vec<Line<'static>>) {
        debug_assert!(
            margins::junction_blank_run(&self.transcript, &rows) <= 1,
            "two blank rows between blocks"
        );
        self.transcript.extend(rows);
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
    }

    pub(crate) fn emit_styled(
        &mut self,
        lines: Vec<Line<'static>>,
        hyperlinks: Vec<HyperlinkTarget>,
    ) {
        self.drain_reveal();
        let shaped = margins::shape_block(lines, hyperlinks);
        let w = self.width().max(10);
        self.atomic(|t| {
            if margins::admit(&mut t.gap_owed, &t.transcript, &shaped) {
                t.write_gap();
            }
            if !shaped.lines.is_empty() {
                let painted =
                    t.render_and_insert_styled_lines(&shaped.lines, &shaped.hyperlinks, w);
                t.record_rows(painted);
            }
        });
    }

    pub(crate) fn emit_rows(&mut self, rows: Vec<Line<'static>>, bg: Option<Color>) {
        self.drain_reveal();
        let shaped = margins::shape_block(rows, Vec::new());
        self.atomic(|t| {
            if margins::admit(&mut t.gap_owed, &t.transcript, &shaped) {
                t.write_gap();
            }
            if !shaped.lines.is_empty() {
                t.insert_paragraph(&shaped.lines, bg);
                t.record_rows(shaped.lines);
            }
        });
    }

    pub fn stream(&mut self, chunk: &str) {
        self.pending.push_str(&strip_ansi(chunk));
        while let Some(idx) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=idx).collect();
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            let style = if self.thinking {
                thinking_style()
            } else {
                Style::default()
            };
            self.push_line_styled(trimmed.to_string(), style);
        }
        let _ = self.draw();
    }

    pub(crate) fn live_width(&mut self) -> usize {
        if let Ok((cols, _)) = crossterm::terminal::size()
            && cols.max(20) != self.last_width
        {
            self.pending_resize = None;
            self.reflow_on_resize(cols.max(20));
        }
        self.width().max(10)
    }

    pub fn stream_thinking(&mut self, chunk: &str) {
        self.turn_had_thinking = true;

        let clean = strip_ansi(chunk);
        self.streamed_bytes = self.streamed_bytes.saturating_add(clean.len() as u64);
        if self.hide_thinking {
            let _ = self.draw();
            return;
        }

        self.release_held_punctuation("");
        if !self.thinking {
            self.ensure_gap();
        }
        if self.status.as_ref().map(|s| s.1.as_str()) != Some("Thinking") {
            self.set_status(Some("Thinking"));
        }
        self.thinking = true;

        gray_core::event::append_thinking_chunk(&mut self.pending, &clean);
        let w = self.live_width();
        let max_w = prose_width(w);
        while let Some(idx) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=idx).collect();
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');

            self.flush_thinking_fragment(trimmed, true);
        }

        if display_width(&self.pending) > max_w {
            let chars: Vec<char> = self.pending.chars().collect();
            let cut = word_flush_cut(&chars, max_w);
            let line: String = chars[..cut].iter().collect();
            self.pending = chars[cut..].iter().collect();
            self.flush_thinking_fragment(&line, false);
        }
        let _ = self.draw();
    }

    pub fn set_hide_thinking(&mut self, hide: bool) {
        self.hide_thinking = hide;
    }

    pub fn stream_text(&mut self, chunk: &str) {
        self.end_thinking_run(true);
        if self.status.as_ref().map(|s| s.1.as_str()) != Some("Working") {
            self.set_status(Some("Working"));
        }
        let clean = strip_ansi(chunk);
        if should_drop_stream_punctuation(self.stream_round_boundary, &clean) {
            self.stream_round_target = attach_stream_punctuation(
                &mut self.history_entries,
                self.stream_round_target,
                clean.trim(),
            );

            self.stream_round_boundary = true;
            self.streamed_bytes = self.streamed_bytes.saturating_add(clean.len() as u64);
            let _ = self.draw();
            return;
        }
        if self.stream_round_boundary && clean.trim().is_empty() {
            self.streamed_bytes = self.streamed_bytes.saturating_add(clean.len() as u64);
            let _ = self.draw();
            return;
        }
        if should_hold_stream_chunk(self.stream_punct_hold.len(), &clean) {
            self.streamed_bytes = self.streamed_bytes.saturating_add(clean.len() as u64);
            self.stream_punct_hold.push_str(&clean);
            let _ = self.draw();
            return;
        }
        self.stream_round_boundary = false;
        self.stream_round_target = None;
        if !clean.trim().is_empty() {
            self.stream_round_had_text = true;
        }

        self.streamed_bytes = self.streamed_bytes.saturating_add(clean.len() as u64);

        let mut clean = clean;
        if !self.stream_punct_hold.is_empty() {
            let held = std::mem::take(&mut self.stream_punct_hold);
            clean = format!("{held}{clean}");
        }
        self.feed_stream_text(&clean);
        let _ = self.draw();
    }

    pub(crate) fn enable_reveal_pacing(&mut self) {
        self.reveal_pacing = true;
    }

    fn feed_stream_text(&mut self, clean: &str) {
        if self.reveal_pacing {
            self.reveal_buf.push_str(clean);
            self.tick_reveal();
        } else {
            self.feed_markdown(clean);
        }
    }

    pub(crate) fn tick_reveal(&mut self) {
        if self.modal_open {
            return;
        }
        if self.intro_started.is_some() {
            // The baked intro runs at frame cadence, not at the 100ms status
            // tick — redraw on every ticker pass or it looks like a slideshow.
            let _ = self.draw();
            return;
        }
        if self.home_reveal.is_some() {
            // Band fade-in after the intro: frame cadence until it lands.
            let _ = self.draw();
        }
        if self.reveal_buf.is_empty() {
            return;
        }
        let chars: Vec<char> = self.reveal_buf.chars().collect();
        let mut n = chars.len().div_ceil(20).max(3).min(chars.len());
        if n < chars.len() && !chars[n - 1].is_whitespace() {
            let mut end = n;
            while end < chars.len() && !chars[end].is_whitespace() {
                end += 1;
            }
            if end - n <= 8 {
                n = end;
            }
        }
        let released: String = chars[..n].iter().collect();
        self.reveal_buf.drain(..released.len());
        self.feed_markdown(&released);
        let _ = self.draw();
    }

    pub(crate) fn drain_reveal(&mut self) {
        if self.reveal_buf.is_empty() || self.feeding_markdown {
            return;
        }
        let rest = std::mem::take(&mut self.reveal_buf);
        self.feed_markdown(&rest);
    }

    fn feed_markdown(&mut self, clean: &str) {
        let was_feeding = self.feeding_markdown;
        self.feeding_markdown = true;
        let tw = prose_width(self.live_width());
        self.markdown_renderer.set_max_table_width(Some(tw));
        self.markdown_renderer
            .push_and_render(clean, Some(gray_markdown::get_syntect()));
        let frozen_len = self.markdown_renderer.frozen_lines_len();
        if frozen_len > self.committed_markdown_lines {
            self.atomic(|t| {
                if t.committed_markdown_lines == 0 {
                    t.ensure_gap();
                }
                let view = t.markdown_renderer.view();
                let new_lines: Vec<Line<'static>> =
                    view.lines[t.committed_markdown_lines..frozen_len].to_vec();
                let hyperlinks = view.hyperlinks.to_vec();
                let offset = t.committed_markdown_lines;
                t.committed_markdown_lines = frozen_len;
                t.push_styled_lines_with_hyperlinks(new_lines, &hyperlinks, offset);
            });
        }
        self.feeding_markdown = was_feeding;
    }

    pub(crate) fn release_held_punctuation(&mut self, following: &str) {
        if self.stream_punct_hold.is_empty() {
            return;
        }
        let held = std::mem::take(&mut self.stream_punct_hold);
        let clean = format!("{held}{following}");
        self.feed_stream_text(&clean);
    }

    pub(crate) fn discard_held_punctuation(&mut self) {
        self.drain_reveal();
        self.stream_punct_hold.clear();
    }

    pub fn end_thinking(&mut self) {
        self.end_thinking_run(true);

        self.release_held_punctuation("");
        let _ = self.draw();
    }

    pub(crate) fn end_thinking_run(&mut self, spacer: bool) {
        if !self.thinking && self.pending.is_empty() {
            return;
        }

        self.thinking = false;
        if self.hide_thinking {
            self.pending.clear();
            return;
        }
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.flush_thinking_fragment(&rest, false);
        }
        if spacer {
            self.ensure_gap();
        }
    }

    pub fn push_user_prompt(
        &mut self,
        text: &str,
        attached: &[std::path::PathBuf],
        trailing_gap: bool,
    ) {
        self.atomic(|t| {
            t.ensure_gap();
            let lines = format_user_prompt_lines(text, attached, t.width().max(10));
            t.history_entries.push(super::TranscriptEntry::UserPrompt(
                text.to_string(),
                attached.to_vec(),
            ));
            t.emit_rows(lines, Some(crate::theme::theme().surface_bg));
            if trailing_gap {
                t.ensure_gap();
            }
        });
        cap_history_entries(&mut self.history_entries);
        let _ = std::io::stdout().flush();
    }
}

/// Marker that opens a synthetic user message carrying a background-task
/// notification to the model. Stored in the session like any user message,
/// but replayed as a compact dim line instead of a prompt block.
pub(crate) const BG_NOTICE_MARKER: &str = "[Background task notification]";

/// Marker that opens a synthetic user message carrying a cron delivery.
pub(crate) const CRON_DELIVERY_MARKER: &str = "[Cron delivery: ";

fn is_notice_marker(line: &str) -> bool {
    line.starts_with(BG_NOTICE_MARKER)
        || line.starts_with(CRON_DELIVERY_MARKER)
        || line.starts_with(BG_JOBS_MARKER)
        || line.starts_with(CONTEXT_UPDATE_MARKER)
        || line.starts_with(SUBAGENT_DONE_MARKER)
}

/// Marker that opens a synthetic user message listing still-running
/// background jobs (agent_loop injects it before an idle wait).
pub(crate) const BG_JOBS_MARKER: &str = "[Background jobs still running]";

/// Marker that opens a synthetic user message carrying hook context.
pub(crate) const CONTEXT_UPDATE_MARKER: &str = "[Context update]";

/// Marker that opens the gray-subagents plugin's completion report: the
/// preamble is model-facing scaffolding; the report lines follow it.
pub(crate) const SUBAGENT_DONE_MARKER: &str = "Subagent finished";

/// Injected nudge lines that ARE the notice — self-contained agent-loop
/// scaffolding (no marker header, no body to capture). Replayed as the
/// dim line itself; anything after them in the same segment is still
/// scaffolding and gets swallowed.
const NUDGE_LINE_PREFIXES: &[&str] = &[
    "previous response truncated",
    "executed tool calls but returned empty",
    "no tool calls have run yet",
    "your last message contained raw tool-call markup",
    "Before finishing, verify the deliverable:",
    "Continue the pending user request",
];

/// Split a stored user prompt into the part worth replaying as a prompt
/// block and one-line summaries of any injected-notification segments.
/// `↳ from` session sends stay visible — they carry real user text.
pub(crate) fn split_synthetic_prompt(text: &str) -> (String, Vec<String>) {
    let mut visible: Vec<&str> = Vec::new();
    let mut notices: Vec<String> = Vec::new();
    let mut in_notice = false;
    let mut captured = false;
    for line in text.lines() {
        if is_notice_marker(line) {
            in_notice = true;
            captured = false;
            continue;
        }
        if NUDGE_LINE_PREFIXES.iter().any(|p| line.starts_with(p)) {
            in_notice = true;
            captured = true;
            notices.push(line.trim().to_string());
            continue;
        }
        if line.starts_with(crate::session_send::SEND_FROM_PREFIX) {
            in_notice = false;
        }
        if in_notice {
            if !captured && !line.trim().is_empty() {
                notices.push(line.trim().to_string());
                captured = true;
            }
        } else {
            visible.push(line);
        }
    }
    (visible.join("\n").trim_end().to_string(), notices)
}

#[path = "mod_tests.rs"]
#[cfg(test)]
mod tests;
