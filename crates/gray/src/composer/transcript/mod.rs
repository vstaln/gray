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
mod rows;

pub(crate) use crate::tui::strip_ansi;
pub(crate) use boxes::render_markdown_lines;
pub(crate) use cards::format_tool_box_lines;
pub(crate) use rows::{
    GUTTER, format_user_prompt_lines, left_pad, prose_width, thinking_paint_text,
    thinking_replay_lines, thinking_run_rows, thinking_style, transcript_row_is_blank,
    word_flush_cut, wrap_styled_line, wrap_styled_line_with_ranges,
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

/// `/hehe` again: drops the graychan marker entry so the transcript is
/// back to the default gray ASCII welcome. Drops the LAST one (the art
/// `/hehe` just added); returns false when none is there.
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

/// Lays history out at width `w` through the margin funnel without a
/// terminal: the setup screen's backdrop. Same blocks, same `admit`, so the
/// copy behind a modal keeps the live transcript's margins. Styled rows stay
/// unwrapped (the backdrop clips them); `mascot_h` is the screen height the
/// `/hehe` art is sized for.
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
            // The banner keeps its own spacing, blank rows included.
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

/// Attach a punctuation suffix to the anchored prose block. The live
/// terminal intentionally does not repaint that old scrollback row; history
/// remains the source of truth for a later reflow/replay.
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
        // The anchor was evicted or changed shape. Preserve the suffix as its
        // own history row rather than attaching it to an unrelated warning.
        entries.push(super::TranscriptEntry::StyledLines {
            lines: vec![Line::from(suffix.to_string())],
            hyperlinks: Vec::new(),
        });
        cap_history_entries(entries);
        return None;
    }

    // No anchor means the original prose was not available; never attach to
    // whichever unrelated styled row happens to be newest.
    entries.push(super::TranscriptEntry::StyledLines {
        lines: vec![Line::from(suffix.to_string())],
        hyperlinks: Vec::new(),
    });
    cap_history_entries(entries);
    None
}

/// A punctuation-only first text delta after a completed provider round is
/// a continuation of text already painted before the round boundary.  It is
/// not useful as a fresh paragraph in the live transcript (and commonly
/// appears as a lone `.` row).  Keep the state predicate pure so the live
/// gate is explicit and testable.
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

/// Whether `stream_text` should hold this delta instead of rendering it:
/// a punctuation-only chunk with no round boundary armed (a mid-round
/// split or the post-interrupt tail), or any further chunk while a burst
/// is already held (punctuation and the whitespace separators between
/// bursts ride along until a meaningful delta releases them). Pure so the
/// live gate is explicit and testable (`Tui::new` needs a TTY).
pub(crate) fn should_hold_stream_chunk(hold_len: usize, text: &str) -> bool {
    hold_len > 0 || is_orphan_stream_punctuation(text)
}

impl Tui {
    /// Marks a block boundary: one gap is owed, paid as the prefix of the
    /// next content (see `margins`). Writes nothing, so a boundary never
    /// leaves a blank row at the bottom and repeated boundaries never stack.
    pub(crate) fn ensure_gap(&mut self) {
        if !self.gap_owed {
            self.history_entries.push(super::TranscriptEntry::Gap(1));
            cap_history_entries(&mut self.history_entries);
        }
        self.gap_owed = true;
    }

    /// Pays an owed gap now, with no block to follow it (the band is about
    /// to leave: `shutdown`). Only above content, like every gap.
    pub(crate) fn pay_gap(&mut self) {
        if margins::gap_due(self.gap_owed, &self.transcript) {
            self.write_gap();
        }
        self.gap_owed = false;
    }

    fn write_gap(&mut self) {
        let blank = vec![Line::default()];
        self.insert_paragraph(&blank, None);
        self.record_rows(blank);
    }

    /// Mirrors painted rows into `transcript`. Every painted row lands here,
    /// so this is where a doubled margin would show: debug builds assert it
    /// cannot.
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

    /// The funnel for logical lines (prose, notices, errors): blank rows at
    /// the block's edges become boundaries, an owed gap is paid in front, and
    /// the rows are wrapped and painted. Live commits, reflow and replay all
    /// pass through here, so they lay out alike.
    pub(crate) fn emit_styled(
        &mut self,
        lines: Vec<Line<'static>>,
        hyperlinks: Vec<HyperlinkTarget>,
    ) {
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

    /// The same funnel for rows already laid out at the current width
    /// (cards, a thinking run's rows, the mascot). `bg` paints card rows.
    pub(crate) fn emit_rows(&mut self, rows: Vec<Line<'static>>, bg: Option<Color>) {
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
        // Thinking is not markdown prose. Leave the continuation guard
        // untouched: a provider may emit reasoning between a tool result and
        // the text delta that continues the pre-tool sentence.
        // Count before the hidden early-return: hidden reasoning still
        // bills output, so the live pill must keep ticking either way.
        let clean = strip_ansi(chunk);
        self.streamed_bytes = self.streamed_bytes.saturating_add(clean.len() as u64);
        if self.hide_thinking {
            let _ = self.draw();
            return;
        }
        // Release a held punctuation burst before the Thought rows land:
        // it continues the prose paragraph above, not the reasoning run.
        self.release_held_punctuation("");
        if !self.thinking {
            self.ensure_gap();
        }
        if self.status.as_ref().map(|s| s.1.as_str()) != Some("Thinking") {
            self.set_status(Some("Thinking"));
        }
        self.thinking = true;
        // Reasoning rides inside output_tokens (confirmed at the call site),
        // so it feeds the live pill estimate too — otherwise tokens freeze
        // mid-thought and only jump once the answer streams.
        // Bare sentence boundaries from stripped providers
        // (`truncated.Identifying`): exactly one space when the join
        // needs it, never doubled, BPE splits untouched.
        gray_core::event::append_thinking_chunk(&mut self.pending, &clean);
        let w = self.live_width();
        let max_w = prose_width(w);
        while let Some(idx) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=idx).collect();
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            // Stored terminated (its `\n` rides along); the blank-on-blank
            // guard lives in `paint_thinking_fragment` and in the reflow
            // renderer alike, so stored source and paint agree exactly.
            self.flush_thinking_fragment(trimmed, true);
        }
        // Strictly wider than a row: a buffer that exactly fills one could
        // still be mid-word ("actu" + "ally"), and flushing it whole leaves
        // the next chunk's leading space to open the following row.
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
            // A provider round boundary resets the markdown renderer.  If the
            // next delta is only sentence punctuation, it continues text that
            // was already painted before the boundary; do not commit a lone
            // punctuation row to the live transcript. Keep it in history so
            // reflow/replay can carry the character forward.
            self.stream_round_target = attach_stream_punctuation(
                &mut self.history_entries,
                self.stream_round_target,
                clean.trim(),
            );
            // Keep the guard armed for another punctuation-only chunk; it is
            // cleared only when a meaningful text delta arrives.
            self.stream_round_boundary = true;
            self.streamed_bytes = self.streamed_bytes.saturating_add(clean.len() as u64);
            let _ = self.draw();
            return;
        }
        if self.stream_round_boundary && clean.trim().is_empty() {
            // Whitespace is a separator, not a new text block. Keep the
            // continuation armed until the next meaningful delta.
            self.streamed_bytes = self.streamed_bytes.saturating_add(clean.len() as u64);
            let _ = self.draw();
            return;
        }
        if should_hold_stream_chunk(self.stream_punct_hold.len(), &clean) {
            // No round boundary armed, but the delta is punctuation-only:
            // a mid-round split or the post-interrupt tail. Feeding the
            // streaming renderer would freeze it as its own lone `.`
            // paragraph row (checkpoints only advance on complete
            // blocks). Hold the burst; it is released into the renderer
            // by the next meaningful delta, a tool/reconnect event, or
            // the turn end — and dropped when nothing follows.
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
        // Live pill estimate ticks per chunk; exact usage reports (set_usage)
        // and TurnEnd bills overwrite it. Bytes/4, the repo estimate heuristic.
        self.streamed_bytes = self.streamed_bytes.saturating_add(clean.len() as u64);
        // Release a held punctuation burst ahead of this delta so the
        // renderer's source order matches arrival order (the held chunk
        // continues the paragraph this delta belongs to).
        let mut clean = clean;
        if !self.stream_punct_hold.is_empty() {
            let held = std::mem::take(&mut self.stream_punct_hold);
            clean = format!("{held}{clean}");
        }
        // Feed the live viewport width so tables lay out to fit (or fall back
        // to records) instead of rendering wide and shredding downstream.
        // Reflows first when the size actually changed (same ~75ms trailing
        // debounce as the idle ticker), so a resize mid-table re-lays the
        // committed rows instead of only affecting new tables.
        let tw = prose_width(self.live_width());
        self.markdown_renderer.set_max_table_width(Some(tw));
        self.markdown_renderer
            .push_and_render(&clean, Some(gray_markdown::get_syntect()));
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
        let _ = self.draw();
    }

    /// Flushes a held punctuation burst back into the streaming renderer
    /// (prepended to `following`, which may be empty). Called when evidence
    /// arrives that the held chunk was not an orphan continuation: a
    /// meaningful text delta (`stream_text`), thinking starting, a tool/
    /// reconnect event, or the turn end.
    pub(crate) fn release_held_punctuation(&mut self, following: &str) {
        if self.stream_punct_hold.is_empty() {
            return;
        }
        let held = std::mem::take(&mut self.stream_punct_hold);
        let clean = format!("{held}{following}");
        let tw = prose_width(self.live_width());
        self.markdown_renderer.set_max_table_width(Some(tw));
        self.markdown_renderer
            .push_and_render(&clean, Some(gray_markdown::get_syntect()));
        let frozen_len = self.markdown_renderer.frozen_lines_len();
        if frozen_len > self.committed_markdown_lines {
            if self.committed_markdown_lines == 0 {
                self.ensure_gap();
            }
            let view = self.markdown_renderer.view();
            let new_lines: Vec<Line<'static>> =
                view.lines[self.committed_markdown_lines..frozen_len].to_vec();
            let hyperlinks = view.hyperlinks.to_vec();
            let offset = self.committed_markdown_lines;
            self.committed_markdown_lines = frozen_len;
            self.push_styled_lines_with_hyperlinks(new_lines, &hyperlinks, offset);
        }
    }

    /// Drops a held punctuation burst without rendering it. The burst was
    /// the turn's final delta (interrupt, stream end): as an orphan it
    /// freezes as a lone `.` row in the live transcript, which is exactly
    /// what the hold exists to prevent.
    pub(crate) fn discard_held_punctuation(&mut self) {
        self.stream_punct_hold.clear();
    }

    pub fn end_thinking(&mut self) {
        self.end_thinking_run(true);
        // A held punctuation burst rides a reasoning-only interlude:
        // thinking text cannot start a new prose block, so release what
        // arrived before it (mid-round split into a `Thought for…` run).
        self.release_held_punctuation("");
        let _ = self.draw();
    }

    pub(crate) fn end_thinking_run(&mut self, spacer: bool) {
        if !self.thinking && self.pending.is_empty() {
            return;
        }
        // The run's rows already streamed live; only the never-flushed tail
        // lands here. No `✻ Thought for` summary: the turn-end footer in
        // `end_turn` is the single Thought line per turn — it alone knows the
        // billed output tokens (exact TurnEnd usage, reasoning included),
        // while a per-run duration line duplicated every reasoning round and
        // re-stamped bogus 0ms lines on stray trailing chunks.
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

    /// Echoes a submitted prompt as a card. `trailing_gap` marks the card's
    /// end as a block boundary; slash commands pass false so their `say()`
    /// feedback (`└ …`) hangs from the card as part of the same block.
    /// Cancelled pickers (dismissed modals) print no feedback, so each of
    /// their `Ok(false)`/`Ok(None)` arms marks the boundary via `ensure_gap`.
    pub fn push_user_prompt(
        &mut self,
        text: &str,
        attached: &[std::path::PathBuf],
        trailing_gap: bool,
    ) {
        self.atomic(|t| {
            // The card is its own block: the unpainted gap sits outside its
            // painted padding.
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

#[path = "mod_tests.rs"]
#[cfg(test)]
mod tests;
