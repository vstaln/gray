use std::collections::HashMap;
use std::io::Write;
use std::ops::Range;
use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use gray_markdown::HyperlinkTarget;

use crate::text_width::display_width;

use super::Tui;

mod boxes;
mod cards;
mod rows;

pub(crate) use crate::tui::strip_ansi;
pub(crate) use cards::format_tool_box_lines;
pub(crate) use rows::{
    format_user_prompt_lines, left_pad, thinking_replay_lines, thinking_style, word_flush_cut,
    wrap_styled_line, wrap_styled_line_with_ranges,
};

// ---------------------------------------------------------------------------
// Tui transcript methods (batch insert_before)
// ---------------------------------------------------------------------------
/// Bounds `history_entries` with the same oldest-first policy as the
/// `transcript` `> 1000 / drain 0..100` guards: without this, long sessions
/// grow the vec without bound. `reflow_on_resize` just re-emits whatever
/// remains (eviction only shortens resize scrollback, like the transcript
/// cap) and `replay_session_history` reads `SessionEntry`s, not this vec,
/// so resume is unaffected.
pub(crate) fn cap_history_entries(entries: &mut Vec<super::TranscriptEntry>) {
    if entries.len() > 1000 {
        entries.drain(0..100);
    }
}

impl Tui {
    pub(crate) fn ensure_gap(&mut self, n: usize) {
        let trailing = self
            .transcript
            .iter()
            .rev()
            .take_while(|l| {
                l.style.bg.is_none()
                    && l.spans
                        .iter()
                        .all(|s| s.style.bg.is_none() && s.content.trim().is_empty())
            })
            .count();
        let need = n.saturating_sub(trailing);
        if need == 0 {
            return;
        }
        let lines: Vec<Line<'static>> = (0..need).map(|_| Line::from("")).collect();
        self.insert_paragraph(&lines, None);
        self.history_entries.push(super::TranscriptEntry::Gap(need));
        self.transcript.extend(lines);
        cap_history_entries(&mut self.history_entries);
    }

    pub fn stream(&mut self, chunk: &str) {
        self.pending.push_str(&strip_ansi(chunk));
        while let Some(idx) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=idx).collect();
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            if trimmed.is_empty()
                && self
                    .transcript
                    .last()
                    .is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
            {
                continue;
            }
            let style = if self.thinking {
                thinking_style()
            } else {
                Style::default()
            };
            self.push_line_styled(trimmed.to_string(), style);
        }
        let _ = self.draw();
    }

    /// Live terminal width for wrapping cuts. `last_width` goes stale
    /// between resizes (the reflow debounce only updates it ~75ms+ later),
    /// so wrapping against it paints rows for the old geometry: narrowing
    /// then yields over-wide rows that Paragraph hard-clips at the right
    /// edge, reading as "shortened" reasoning. Falls back to `last_width`
    /// when the size probe fails (headless tests).
    pub(crate) fn live_width(&mut self) -> usize {
        // Gated on an actual change: an unconditional reflow here would
        // clear + re-emit the whole scrollback on every streamed chunk.
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
        if self.hide_thinking {
            let _ = self.draw();
            return;
        }
        if !self.thinking {
            self.ensure_gap(1);
            self.thinking_started = Some(Instant::now());
        }
        if self.status.as_ref().map(|s| s.1.as_str()) != Some("Thinking") {
            self.set_status(Some("Thinking"));
        }
        self.thinking = true;
        // Bare sentence boundaries from stripped providers
        // (`truncated.Identifying`): exactly one space when the join
        // needs it, never doubled, BPE splits untouched.
        gray_core::event::append_thinking_chunk(&mut self.pending, &strip_ansi(chunk));
        let w = self.live_width();
        let max_w = w.saturating_sub(4).max(1);
        while let Some(idx) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=idx).collect();
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            self.push_line_styled(trimmed.to_string(), thinking_style());
        }
        if display_width(&self.pending) >= max_w {
            let chars: Vec<char> = self.pending.chars().collect();
            let cut = word_flush_cut(&chars, max_w);
            let line: String = chars[..cut].iter().collect();
            self.pending = chars[cut..].iter().collect();
            self.push_line_styled(line, thinking_style());
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
        // Feed the live viewport width so tables lay out to fit (or fall back
        // to records) instead of rendering wide and shredding downstream.
        // Reflows first when the size actually changed (same ~75ms trailing
        // debounce as the idle ticker), so a resize mid-table re-lays the
        // committed rows instead of only affecting new tables.
        let tw = self.live_width().saturating_sub(2);
        self.markdown_renderer.set_max_table_width(Some(tw));
        self.markdown_renderer
            .push_and_render(&clean, Some(gray_markdown::get_syntect()));
        let frozen_len = self.markdown_renderer.frozen_lines_len();
        if frozen_len > self.committed_markdown_lines {
            if self.committed_markdown_lines == 0 {
                self.ensure_gap(1);
            }
            let view = self.markdown_renderer.view();
            let new_lines: Vec<Line<'static>> =
                view.lines[self.committed_markdown_lines..frozen_len].to_vec();
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
        if !self.thinking && self.pending.is_empty() {
            return;
        }
        // Rows already streamed live; only the `✻ Thought for <duration>`
        // summary lands here, after the body (scrollback is append-only).
        let elapsed = self.thinking_started.take().map(|s| s.elapsed());
        self.thinking = false;
        if self.hide_thinking {
            self.pending.clear();
            return;
        }
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.push_line_styled(rest, thinking_style());
        }
        if let Some(d) = elapsed {
            self.ensure_gap(1);
            self.push_line_spans(thought_summary_line(d));
        }
        if spacer {
            self.ensure_gap(1);
            self.release_dock_seam();
        }
    }

    /// Echoes a submitted prompt as a card. `trailing_gap` leaves one blank
    /// below the card for the breathing room before the next prompt; slash
    /// commands pass false so their `say()` feedback hugs the card instead.
    /// Cancelled pickers (dismissed modals) print no feedback, so each of
    /// their `Ok(false)`/`Ok(None)` arms restores the gap via `ensure_gap`.
    pub fn push_user_prompt(
        &mut self,
        text: &str,
        attached: &[std::path::PathBuf],
        trailing_gap: bool,
    ) {
        self.ensure_gap(1);
        let lines = format_user_prompt_lines(text, attached, self.width().max(10));
        self.insert_paragraph(&lines, Some(crate::theme::theme().surface_bg));
        self.history_entries
            .push(super::TranscriptEntry::UserPrompt(
                text.to_string(),
                attached.to_vec(),
            ));
        self.transcript.extend(lines);
        // Trailing gap after every chat card — command and prompt alike.
        // Handlers that print feedback (say()) treat the gap as idempotent;
        // handlers that print nothing (dismissed modal) still leave breathing
        // room before the next prompt instead of jamming against the card.
        // Slash-command cards skip it (trailing_gap=false): their feedback
        // hugs the card, and each dismissed-modal arm adds the gap itself.
        if trailing_gap {
            self.ensure_gap(1);
        }
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        cap_history_entries(&mut self.history_entries);
        let _ = std::io::stdout().flush();
    }
}

/// Port of opencode's `Locale.duration`: `198ms`, `5.8s`, `1m 2s`.
pub(crate) fn fmt_thought_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 3_600_000 {
        let secs = ms as f64 / 1000.0;
        if secs < 60.0 {
            format!("{secs:.1}s")
        } else {
            format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
        }
    } else {
        format!("{}h {}m", ms / 3_600_000, (ms % 3_600_000) / 60_000)
    }
}

/// Bottom summary: gray `✻ Thought for <duration>` under the body, matching
/// the turn-end Thought line (same star marker; duration-only — the
/// provider's true reasoning count isn't known until TurnEnd, and a
/// streamed estimate here would under-report billed reasoning).
fn thought_summary_line(elapsed: Duration) -> Line<'static> {
    Line::from(vec![Span::styled(
        format!("✻ Thought for {}", fmt_thought_duration(elapsed)),
        Style::default().fg(crate::theme::theme().text_muted),
    )])
}

#[path = "mod_tests.rs"]
#[cfg(test)]
mod tests;
