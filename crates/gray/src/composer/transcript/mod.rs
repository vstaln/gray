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

pub(crate) use cards::format_tool_box_lines;
pub(crate) use rows::{
    format_user_prompt_lines, left_pad, strip_ansi, thinking_style, word_flush_cut,
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
        let h = need as u16;
        let _ = self.terminal.insert_before(h, |buf| {
            Paragraph::new(lines.clone()).render(buf.area, buf);
        });
        self.history_entries.push(super::TranscriptEntry::Gap(need));
        self.transcript.extend(lines);
        cap_history_entries(&mut self.history_entries);
    }

    pub fn stream(&mut self, chunk: &str) {
        let toks = chunk.chars().count().div_ceil(4);
        self.live_streamed_tokens += toks.max(1);
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

    pub fn stream_thinking(&mut self, chunk: &str) {
        self.turn_had_thinking = true;
        let toks = chunk.chars().count().div_ceil(4);
        self.live_streamed_tokens += toks.max(1);
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
        self.pending.push_str(&strip_ansi(chunk));
        let w = self.width().max(10);
        let max_w = w.saturating_sub(4).max(1);
        while let Some(idx) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=idx).collect();
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            self.thinking_lines.push(trimmed.to_string());
        }
        if display_width(&self.pending) >= max_w {
            let chars: Vec<char> = self.pending.chars().collect();
            let cut = word_flush_cut(&chars, max_w);
            let line: String = chars[..cut].iter().collect();
            self.pending = chars[cut..].iter().collect();
            self.thinking_lines.push(line);
        }
        let _ = self.draw();
    }

    pub fn set_hide_thinking(&mut self, hide: bool) {
        self.hide_thinking = hide;
    }

    pub fn stream_text(&mut self, chunk: &str) {
        let toks = chunk.chars().count().div_ceil(4);
        self.live_streamed_tokens += toks.max(1);
        self.end_thinking_run(true);
        if self.status.as_ref().map(|s| s.1.as_str()) != Some("Working") {
            self.set_status(Some("Working"));
        }
        let clean = strip_ansi(chunk);
        // Feed the live viewport width so tables lay out to fit (or fall back
        // to records) instead of rendering wide and shredding downstream.
        // No-op when unchanged; resize mid-table only affects new tables.
        let tw = self.width().max(10).saturating_sub(2);
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
        if !self.thinking && self.pending.is_empty() && self.thinking_lines.is_empty() {
            return;
        }
        // Opencode parity (`Thought: <duration>` header + blank + body):
        // rows buffer during the run and flush header-first here —
        // scrollback is append-only (`insert_before`), so unlike opencode's
        // re-rendered header it can't sit on top while streaming.
        let elapsed = self.thinking_started.take().map(|s| s.elapsed());
        self.thinking = false;
        if !self.hide_thinking {
            if !self.pending.is_empty() {
                let rest = std::mem::take(&mut self.pending);
                self.thinking_lines.push(rest);
            }
            if !self.thinking_lines.is_empty() {
                self.ensure_gap(1);
                let rows = std::mem::take(&mut self.thinking_lines);
                for row in rows {
                    self.push_line_styled(row, thinking_style());
                }
                if let Some(d) = elapsed {
                    self.ensure_gap(1);
                    self.push_line_spans(thought_summary_line(d));
                }
            }
            if spacer {
                self.ensure_gap(1);
            }
        } else {
            self.pending.clear();
            self.thinking_lines.clear();
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
        let height = lines.len() as u16;
        let block =
            ratatui::widgets::Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)));
        let _ = self.terminal.insert_before(height, |buf| {
            Paragraph::new(lines.clone())
                .block(block)
                .render(buf.area, buf);
        });
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

/// Bottom summary: gray `⬡ Thought for <duration>` under the body, matching
/// the thinking text above — same hexagon marker as the live status.
fn thought_summary_line(elapsed: Duration) -> Line<'static> {
    Line::from(vec![Span::styled(
        format!("⬡ Thought for {}", fmt_thought_duration(elapsed)),
        Style::default().fg(Color::Rgb(140, 140, 140)),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_flush_cut_breaks_at_spaces() {
        let chars: Vec<char> = "hello world foo".chars().collect();
        let cut = word_flush_cut(&chars, 8);
        let row: String = chars[..cut].iter().collect();
        let rest: String = chars[cut..].iter().collect();
        assert_eq!(row, "hello ");
        assert_eq!(rest, "world foo");
    }

    #[test]
    fn word_flush_cut_hard_cuts_overlong_word() {
        let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyz".chars().collect();
        assert_eq!(word_flush_cut(&chars, 8), 8);
    }

    #[test]
    fn word_flush_cut_exact_fit_pushes_whole() {
        let chars: Vec<char> = "hi you".chars().collect();
        assert_eq!(word_flush_cut(&chars, 6), 6);
    }

    #[test]
    fn word_flush_cut_budgets_cells_not_chars() {
        let chars: Vec<char> = "界界界界".chars().collect();
        // 2×2 cells fit; a third wide char would overflow the 5-cell budget.
        assert_eq!(word_flush_cut(&chars, 5), 2);
    }

    #[test]
    fn thought_duration_matches_opencode_locale() {
        assert_eq!(fmt_thought_duration(Duration::from_millis(198)), "198ms");
        assert_eq!(fmt_thought_duration(Duration::from_millis(5800)), "5.8s");
        assert_eq!(fmt_thought_duration(Duration::from_millis(9800)), "9.8s");
        assert_eq!(fmt_thought_duration(Duration::from_millis(61_500)), "1m 1s");
    }

    #[test]
    fn thought_summary_line_names_duration() {
        let line = thought_summary_line(Duration::from_millis(5800));
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "⬡ Thought for 5.8s");
        assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(140, 140, 140)));
    }

    #[test]
    fn user_prompt_wraps_at_word_boundaries() {
        let text = "write a very long poem about the restless sea";
        let lines = format_user_prompt_lines(text, &[], 24);
        // content rows (skip blank margins) preserve the text exactly
        let bodies: Vec<String> = lines
            .iter()
            .filter_map(|l| l.spans.get(1))
            .map(|s| s.content.to_string())
            .collect();
        assert!(bodies.len() > 1);
        assert_eq!(bodies.concat(), text);
        // every row except the last ends at a space or is a full hard cut
        let max_w = 24usize - 4;
        for b in &bodies[..bodies.len() - 1] {
            assert!(
                b.ends_with(' ') || b.chars().count() == max_w,
                "mid-word break: {b:?}"
            );
        }
    }

    #[test]
    fn user_prompt_wraps_wide_chars_by_cells() {
        let text = "界界界界界";
        let lines = format_user_prompt_lines(text, &[], 6); // 2-cell budget
        let bodies: Vec<Span<'static>> = lines
            .iter()
            .filter_map(|l| l.spans.get(1).cloned())
            .collect();
        assert_eq!(
            bodies
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>(),
            text
        );
        assert!(bodies.len() > 1);
        for b in &bodies {
            assert!(b.width() <= 2, "row overflows: {:?}", b.content);
        }
    }

    #[test]
    fn diff_rows_pad_edge_to_edge() {
        use crate::tool_fmt::{DIFF_DELETE_BG, DIFF_INSERT_BG};
        let header = Line::from("Ran edit");
        let body = vec![
            Line::from(vec![Span::styled(
                "  1 | - old",
                Style::default().bg(DIFF_DELETE_BG),
            )])
            .style(Style::default().bg(DIFF_DELETE_BG)),
            Line::from(vec![Span::styled(
                "  1 | + new",
                Style::default().bg(DIFF_INSERT_BG),
            )])
            .style(Style::default().bg(DIFF_INSERT_BG)),
            Line::from(vec![Span::raw("  2 |   same")]),
        ];
        let lines = format_tool_box_lines(header, &body, 80);
        let row_w = |l: &Line<'static>| l.spans.iter().map(|s| s.width()).sum::<usize>();
        // margin, header, breathing row, then the three body rows
        assert_eq!(lines.len(), 7);
        // tinted rows span the full width (no dark strip on the right)
        assert_eq!(row_w(&lines[3]), 80);
        assert_eq!(row_w(&lines[4]), 80);
        // untinted rows are untouched (card block bg shows through, same color)
        assert!(row_w(&lines[5]) < 80);
    }

    #[test]
    fn wrap_ranges_round_trip_and_identity() {
        // identity: short line maps to the whole source
        let short = Line::from(vec![Span::raw("hello world")]);
        let out = wrap_styled_line_with_ranges(short, 20);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.end, usize::MAX);

        // long line: rows fit max_w and each row's text equals the source slice
        let text =
            "the quick brown fox jumps over the lazy dog again and again until it wraps somewhere";
        let long = Line::from(vec![Span::raw(text.to_string())]);
        let max_w = 24;
        let out = wrap_styled_line_with_ranges(long, max_w);
        assert!(out.len() > 1);
        let mut prev_end = 0usize;
        for (l, r) in &out {
            let row_text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(
                row_text,
                &text[r.clone()],
                "row text must equal its source slice"
            );
            assert!(r.start >= prev_end, "ranges ascend without overlap");
            prev_end = r.end;
        }
    }

    // UNRUN (cargo test banned in X session; run in TTY/CI): over-cap push
    // evicts oldest-first, mirroring the transcript >1000/drain-100 guard.
    #[test]
    fn history_entries_cap_evicts_oldest_first_unrun() {
        let mut entries: Vec<crate::composer::TranscriptEntry> = (0..1001)
            .map(crate::composer::TranscriptEntry::Gap)
            .collect();
        cap_history_entries(&mut entries);
        assert_eq!(entries.len(), 901);
        match &entries[0] {
            crate::composer::TranscriptEntry::Gap(n) => assert_eq!(*n, 100),
            other => panic!("must drop gaps 0..100 oldest-first, got {other:?}"),
        }
    }

    // UNRUN (cargo test banned in X session; run in TTY/CI): at-cap is a no-op.
    #[test]
    fn history_entries_cap_keeps_at_most_1000_unrun() {
        let mut entries: Vec<crate::composer::TranscriptEntry> = (0..1000)
            .map(crate::composer::TranscriptEntry::Gap)
            .collect();
        cap_history_entries(&mut entries);
        assert_eq!(entries.len(), 1000);
    }
}
