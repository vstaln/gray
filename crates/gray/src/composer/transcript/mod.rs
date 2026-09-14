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
    format_user_prompt_lines, left_pad, thinking_style, word_flush_cut, wrap_styled_line,
    wrap_styled_line_with_ranges,
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
        assert_eq!(text, "✻ Thought for 5.8s");
        assert_eq!(
            line.spans[0].style.fg,
            // Single gray palette (no global read — keeps this test
            // hermetic under parallel execution).
            Some(crate::theme::GRAY_UI_THEME.text_muted)
        );
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
        use crate::tool_fmt::{diff_delete_bg, diff_insert_bg};
        let header = Line::from("Ran edit");
        let body = vec![
            Line::from(vec![Span::styled(
                "  1 | - old",
                Style::default().bg(diff_delete_bg()),
            )])
            .style(Style::default().bg(diff_delete_bg())),
            Line::from(vec![Span::styled(
                "  1 | + new",
                Style::default().bg(diff_insert_bg()),
            )])
            .style(Style::default().bg(diff_insert_bg())),
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

    /// Resize regression: the live thinking cut (`word_flush_cut` at the
    /// old width) followed by a reflow re-wrap (each stored chunk wrapped
    /// at the new width) must preserve every word — narrowing or widening
    /// the window mid-turn must never "shorten" the reasoning.
    #[test]
    fn thinking_survives_resize_round_trip() {
        let text = "The second command was blocked by a guard because of `curl ... | python3`? \
            Weird, the first one worked. Let me avoid pipes into interpreters and write to a file instead";
        let norm = text.split_whitespace().collect::<Vec<_>>().join(" ");
        for (w_live, w_new) in [(100usize, 40usize), (40usize, 100usize)] {
            // live cut, mirroring `stream_thinking`
            let max_live = w_live.saturating_sub(4).max(1);
            let mut chunks: Vec<String> = Vec::new();
            let mut rest: Vec<char> = text.chars().collect();
            while !rest.is_empty() {
                let s: String = rest.iter().collect();
                if display_width(&s) < max_live {
                    chunks.push(s);
                    break;
                }
                let cut = word_flush_cut(&rest, max_live);
                assert!(cut > 0, "cut must progress");
                chunks.push(rest[..cut].iter().collect());
                rest = rest[cut..].to_vec();
            }
            // reflow re-wrap, mirroring `reflow_on_resize` (render budget w-2)
            let mut rows: Vec<String> = Vec::new();
            for c in &chunks {
                let line = Line::from(vec![Span::styled(c.clone(), thinking_style())]);
                for w in wrap_styled_line(line, w_new.saturating_sub(2).max(1)) {
                    rows.push(w.spans.iter().map(|s| s.content.as_ref()).collect());
                }
            }
            // inter-row boundary spaces are re-flowable (live chunks keep
            // a trailing space the wrapper then drops); words must survive
            let got = rows
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(
                got, norm,
                "text lost resizing {w_live} -> {w_new}: {rows:?}"
            );
        }
    }

    /// Paragraph has no `.wrap()`: any row wider than the viewport is
    /// hard-clipped at the right edge (the "shortened" symptom). The wrapper
    /// must therefore never emit an over-budget row, including overlong
    /// words (URLs) that force hard cuts.
    #[test]
    fn wrapped_rows_never_exceed_budget() {
        let text = format!(
            "{} https://api.github.com/repos/some/really/long/path/that/never/breaks/at/all",
            "word ".repeat(50)
        );
        for w in [20usize, 40, 80, 120] {
            for row in wrap_styled_line(Line::from(text.clone()), w) {
                let rw: usize = row.spans.iter().map(|s| s.width()).sum();
                assert!(rw <= w, "row overflows budget {w}: {row:?}");
            }
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
