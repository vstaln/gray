use std::collections::HashMap;
use std::io::Write;
use std::ops::Range;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use gray_markdown::HyperlinkTarget;

use super::GatewayBootPanel;
use super::Tui;

mod boxes;
mod cards;
mod rows;

pub(crate) use cards::{
    format_gateway_boot_card, format_tool_box_lines, gateway_boot_card_parts,
    is_gateway_boot_header, paint_card,
};
pub use rows::redact_command_echo;
pub(crate) use rows::{
    format_user_prompt_lines, left_pad, strip_ansi, thinking_style, word_flush_cut,
    wrap_styled_line, wrap_styled_line_with_ranges,
};

// ---------------------------------------------------------------------------
// Tui transcript methods (batch insert_before)
// ---------------------------------------------------------------------------
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
        if !self.thinking && self.pending.is_empty() {
            return;
        }
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
        // hugs the card, and restore_viewport() covers the dismissed modal.
        if trailing_gap {
            self.ensure_gap(1);
        }
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = std::io::stdout().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::cards::CARD_BG;
    use super::*;

    #[test]
    fn gateway_boot_card_is_tight_with_one_space_header() {
        let (header, body) = gateway_boot_card_parts(
            "Gateway autostarted",
            &["  └─ Discord — connected as Gray".to_string()],
        );
        assert!(is_gateway_boot_header(&header));
        assert!(!is_gateway_boot_header(&Line::from("Ran foo")));
        let lines = format_gateway_boot_card(header, &body, 80);
        // top margin, header, row directly below (no middle blank), bottom margin.
        assert_eq!(lines.len(), 4);
        let text: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        assert_eq!(text[0], "");
        assert_eq!(text[1], " Gateway autostarted");
        assert_eq!(text[2], "  └─ Discord — connected as Gray");
        assert_eq!(text[3], "");
        // Every row is a full-width block with the card bg baked into each span:
        // the live viewport and insert_before paint the same thing.
        for l in &lines {
            assert_eq!(l.width(), 80, "row must span the full card width");
            assert!(
                l.spans.iter().all(|s| s.style.bg == Some(CARD_BG)),
                "every span carries the card bg"
            );
        }
    }

    #[test]
    fn redact_command_echo_hides_connect_token() {
        assert_eq!(
            redact_command_echo("/gateway connect discord secret-token"),
            "/gateway connect discord ••••"
        );
        // No token yet: untouched.
        assert_eq!(
            redact_command_echo("/gateway connect discord"),
            "/gateway connect discord"
        );
        assert_eq!(
            redact_command_echo("/gateway pairing approve discord ABC123"),
            "/gateway pairing approve discord ••••"
        );
        // Anything else: untouched.
        assert_eq!(redact_command_echo("/gateway status"), "/gateway status");
        assert_eq!(redact_command_echo("hello world"), "hello world");
    }

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
}
