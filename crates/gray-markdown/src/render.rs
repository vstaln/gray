//! Markdown renderer - transforms parsed markdown buffers into styled output.
//!
//! After parsing with `MarkdownParser`, use `ParsedMarkdown` to render
//! to either ratatui Lines or ANSI strings.

use std::borrow::Cow;

use ratatui::text::{Line, Span};

use crate::buffers::{MarkdownBuffers, RenderEvent, RenderEventKind, unicode_display_width};
use crate::colors::StyleInto;
use crate::checkpoint::Checkpoint;
use crate::hyperlinks::{ChunkLinkRange, chunk_link_offsets, emit_segment_hyperlinks};
use crate::output::{HyperlinkTarget, MarkdownRenderOutput};
use crate::parse::ParsedMarkdown;
use crate::style::{all_hidden, merge_styles};



impl<'a, 'b> ParsedMarkdown<'a, 'b> {
    fn apply_transforms<'t>(&self, text: &'t str, start: usize, pretty: bool) -> Cow<'t, str> {
        if self.buffers.transforms.is_empty() {
            return Cow::Borrowed(text);
        }
        // Raw mode applies only `force` transforms (e.g. soft-break collapse).
        if !pretty && !self.buffers.transforms.iter().any(|t| t.force) {
            return Cow::Borrowed(text);
        }

        let end = start + text.len();
        let mut result = String::new();
        let mut pos = start;
        let mut applied = false;

        for transform in &self.buffers.transforms {
            if transform.range.end <= start || transform.range.start >= end {
                continue;
            }
            if !pretty && !transform.force {
                continue;
            }
            applied = true;
            // Clamp transform range to our text range
            let t_start = transform.range.start.max(start);
            let t_end = transform.range.end.min(end);

            // Copy text before transform
            if t_start > pos {
                let before = &text[(pos - start)..(t_start - start)];
                result.push_str(before);
            }

            // Apply transform
            result.push_str(&transform.to);

            pos = t_end;
        }

        if !applied {
            Cow::Borrowed(text)
        } else {
            // Copy remaining text
            if pos < end {
                result.push_str(&text[(pos - start)..]);
            }
            Cow::Owned(result)
        }
    }

    /// Build sorted render events into the provided Vec.
    fn build_render_events_into(&self, events: &mut Vec<RenderEvent>) {
        events.clear();
        let capacity = self.buffers.highlights.len() * 2
            + self.buffers.replaces.len() * 2
            + self.buffers.table_replaces.len() * 2
            ; // mermaid removed
        events.reserve(capacity);

        for (i, hl) in self.buffers.highlights.iter().enumerate() {
            events.push(RenderEvent {
                pos: hl.range.start,
                kind: RenderEventKind::Highlight,
                index: i,
                is_end: false,
            });
            events.push(RenderEvent {
                pos: hl.range.end,
                kind: RenderEventKind::Highlight,
                index: i,
                is_end: true,
            });
        }
        for (i, r) in self.buffers.replaces.iter().enumerate() {
            events.push(RenderEvent {
                pos: r.range.start,
                kind: RenderEventKind::Replace,
                index: i,
                is_end: false,
            });
            events.push(RenderEvent {
                pos: r.range.end,
                kind: RenderEventKind::Replace,
                index: i,
                is_end: true,
            });
        }
        for (i, t) in self.buffers.table_replaces.iter().enumerate() {
            events.push(RenderEvent {
                pos: t.range.start,
                kind: RenderEventKind::Table,
                index: i,
                is_end: false,
            });
            events.push(RenderEvent {
                pos: t.range.end,
                kind: RenderEventKind::Table,
                index: i,
                is_end: true,
            });
        }
        // mermaid removed: fences render as code blocks
        events.sort_unstable();
    }

    /// Build sorted render events into a new Vec.
    fn build_render_events(&self) -> Vec<RenderEvent> {
        let mut events = Vec::new();
        self.build_render_events_into(&mut events);
        events
    }

    /// Render to ANSI-styled string.
    ///
    /// If `pretty` is true, syntax markers are hidden.
    /// Returns the rendered string and a source map for copy-paste support.
    /// Render to ratatui Lines.
    ///
    /// If `pretty` is true, syntax markers are hidden.
    /// Returns rendered lines, line source map, and optional checkpoint.
    pub fn render_ratatui(&mut self, pretty: bool) -> (MarkdownRenderOutput, Option<Checkpoint>) {
        // Build render events
        let render_events = self.build_render_events();

        self.buffers.current_spans.clear();
        self.buffers.active_highlights.clear();

        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut line_source_map: Vec<usize> = Vec::new();
        let mut hyperlinks: Vec<HyperlinkTarget> = Vec::new();

        let mut last_pos = 0;
        let mut replace: Option<usize> = None;
        let mut table_replace: Option<usize> = None;
        let mut skip_leading_newline = false;
        let mut in_hidden_code_block = false;
        let mut next_link_idx: usize = 0;
        // Running display-column tracker for the in-progress line.
        let mut cur_col_in_line: usize = 0;

        let checkpoint_info = self.last_checkpoint;
        let mut checkpoint_output_lines: Option<usize> = None;

        // Style already adapted - no need to call adapt_style again
        let code_bg_style: ratatui::style::Style = self.ms.code_background.style_into();

        let in_untagged_code = |pos: usize, buffers: &MarkdownBuffers| -> bool {
            buffers
                .untagged_code_ranges
                .iter()
                .any(|range| pos >= range.start && pos < range.end)
        };

        let mut current_source_line = 0usize;
        let mut last_line_count_pos = 0usize;
        let mut pending_line_is_code = false;

        let count_newlines_in_range = |from: usize, to: usize, text: &str| -> usize {
            if to <= from {
                return 0;
            }
            let to = to.min(text.len());
            let from = from.min(to);
            // Use as_bytes() to avoid panicking on non-char-boundary offsets.
            // This is safe because '\n' (0x0A) is a single-byte ASCII value
            // that can never appear as a UTF-8 continuation byte (0x80..0xBF).
            text.as_bytes()[from..to]
                .iter()
                .filter(|&&b| b == b'\n')
                .count()
        };

        for ev in &render_events {
            if replace.is_none()
                && table_replace.is_none()
               
                && ev.pos > last_pos
            {
                // Check if we need to split text processing at the checkpoint boundary.
                // If last_pos < cp_byte <= ev.pos, we process in two parts:
                // 1. Process [last_pos..cp_byte], capture lines.len(), process [cp_byte..ev.pos]
                let split_at_checkpoint = checkpoint_output_lines.is_none()
                    && checkpoint_info
                        .map(|(_, cp_byte)| last_pos < cp_byte && cp_byte <= ev.pos)
                        .unwrap_or(false);

                let cp_byte = checkpoint_info.map(|(_, cp)| cp).unwrap_or(0);

                // Snap cp_byte to the nearest char boundary.  Checkpoint byte
                // offsets come from pulldown-cmark event ranges which should
                // always be char-aligned, but in edge cases (e.g., thematic
                // breaks followed by headings with multi-byte chars) the
                // position can land mid-character.  Snapping forward is safe
                // because it only affects where we split the text for line
                // counting — a few extra or fewer newlines in the first vs
                // second range doesn't change the total count.
                let cp_byte = {
                    let mut b = cp_byte;
                    while b < self.text.len() && !self.text.is_char_boundary(b) {
                        b += 1;
                    }
                    b
                };

                // Determine ranges to process
                let ranges: &[(usize, usize)] = if split_at_checkpoint {
                    // Process in two parts, capturing checkpoint between them
                    &[(last_pos, cp_byte), (cp_byte, ev.pos)]
                } else {
                    // Process as single range
                    &[(last_pos, ev.pos)]
                };

                for (range_idx, &(range_start, range_end)) in ranges.iter().enumerate() {
                    // After processing the first range when splitting, capture checkpoint.
                    // Flush any pending spans to `lines` first — content like a thematic
                    // break (`───`) may sit in `current_spans` without a trailing newline
                    // to flush it.  Without this flush, the checkpoint's `output_lines`
                    // count would be too low, causing the line to vanish on re-render.
                    if split_at_checkpoint && range_idx == 1 {
                        if !self.buffers.current_spans.is_empty() {
                            line_source_map.push(current_source_line);
                            let line = Line::from(std::mem::take(&mut self.buffers.current_spans));
                            lines.push(line);
                            cur_col_in_line = 0;
                        }
                        checkpoint_output_lines = Some(lines.len());
                    }

                    if range_end <= range_start {
                        continue;
                    }

                    // Update source line counter
                    if range_start > last_line_count_pos {
                        current_source_line +=
                            count_newlines_in_range(last_line_count_pos, range_start, self.text);
                        last_line_count_pos = range_start;
                    }

                    let is_hidden = pretty
                        && all_hidden(
                            self.buffers
                                .active_highlights
                                .iter()
                                .map(|&i| self.buffers.highlights[i].style),
                        );

                    if is_hidden {
                        let at_line_start = range_start == 0
                            || self.text.as_bytes().get(range_start - 1) == Some(&b'\n');
                        if at_line_start {
                            // Check if this hidden block is a code fence (``` or ~~~).
                            // Only code fences need separator handling — heading markers
                            // (#) are also hidden at line start but are unpaired.
                            let hidden_text = self.text[range_start..range_end].trim_start();
                            let is_code_fence =
                                hidden_text.starts_with("```") || hidden_text.starts_with("~~~");

                            if is_code_fence {
                                // Emit a blank separator before an OPENING fence (not
                                // closing). Prevents adjacent blocks (e.g., list → code)
                                // from collapsing their visual boundary when the hidden
                                // fence markers are removed in pretty mode.
                                if !in_hidden_code_block
                                    && lines.last().is_some_and(|l| l.width() > 0)
                                {
                                    line_source_map.push(current_source_line);
                                    lines.push(Line::default());
                                    cur_col_in_line = 0;
                                }
                                in_hidden_code_block = !in_hidden_code_block;
                            }
                            skip_leading_newline = true;
                        }
                    } else {
                        let mut text = &self.text[range_start..range_end];
                        let mut text_start = range_start;

                        if skip_leading_newline && text.starts_with('\n') {
                            text = &text[1..];
                            text_start += 1;
                        }
                        skip_leading_newline = false;

                        if !text.is_empty() {
                            let style = merge_styles(
                                self.buffers
                                    .active_highlights
                                    .iter()
                                    .map(|&i| self.buffers.highlights[i].style),
                            );

                            let transformed = self.apply_transforms(text, range_start, pretty);
                            let ratatui_style: ratatui::style::Style = style.style_into();

                            let chunk_src_start = text_start;
                            let chunk_src_end = text_start + text.len();

                            // Advance the cursor past links that ended before
                            // this chunk starts, then check if any remaining
                            // link overlaps the chunk.  Skip all hyperlink
                            // bookkeeping when none does — keeps the no-link
                            // hot path identical to the pre-feature renderer.
                            while next_link_idx < self.buffers.link_targets.len()
                                && self.buffers.link_targets[next_link_idx].source_range.end
                                    <= chunk_src_start
                            {
                                next_link_idx += 1;
                            }
                            let chunk_has_links = next_link_idx < self.buffers.link_targets.len()
                                && self.buffers.link_targets[next_link_idx].source_range.start
                                    < chunk_src_end;

                            let chunk_links: Vec<ChunkLinkRange> = if chunk_has_links {
                                chunk_link_offsets(
                                    &self.buffers.link_targets,
                                    next_link_idx,
                                    chunk_src_start,
                                    chunk_src_end,
                                    pretty,
                                    &self.buffers.transforms,
                                )
                            } else {
                                Vec::new()
                            };

                            let mut byte_offset = text_start;
                            let mut seg_x_offset: usize = 0;
                            let is_in_code = in_untagged_code(text_start, self.buffers);
                            pending_line_is_code = is_in_code;
                            for (idx, segment) in transformed.split('\n').enumerate() {
                                if idx > 0 {
                                    line_source_map.push(current_source_line);
                                    let line =
                                        Line::from(std::mem::take(&mut self.buffers.current_spans));
                                    lines.push(if is_in_code {
                                        line.style(code_bg_style)
                                    } else {
                                        line
                                    });
                                    if byte_offset > last_line_count_pos {
                                        current_source_line += count_newlines_in_range(
                                            last_line_count_pos,
                                            byte_offset,
                                            self.text,
                                        );
                                        last_line_count_pos = byte_offset;
                                    }
                                    cur_col_in_line = 0;
                                }

                                if !chunk_links.is_empty() {
                                    emit_segment_hyperlinks(
                                        &chunk_links,
                                        &self.buffers.link_targets,
                                        segment,
                                        seg_x_offset,
                                        cur_col_in_line,
                                        lines.len(),
                                        &mut hyperlinks,
                                    );
                                }

                                if !segment.is_empty() {
                                    self.buffers
                                        .current_spans
                                        .push(Span::styled(segment.to_string(), ratatui_style));
                                    cur_col_in_line += unicode_display_width(segment);
                                }
                                byte_offset += segment.len() + 1;
                                seg_x_offset += segment.len() + 1;
                            }
                        }
                    }
                }
                last_pos = ev.pos;
            }

            match ev.kind {
                RenderEventKind::Replace => {
                    if ev.is_end && replace == Some(ev.index) {
                        replace = None;
                    } else if !ev.is_end && replace.is_none() && table_replace.is_none() {
                        replace = Some(ev.index);
                        let repl = &self.buffers.replaces[ev.index];

                        // Update source line to code start
                        if repl.range.start > last_line_count_pos {
                            current_source_line += count_newlines_in_range(
                                last_line_count_pos,
                                repl.range.start,
                                self.text,
                            );
                        }
                        let code_start_source_line = current_source_line;

                        for (line_idx, line_spans) in repl.highlighted.iter().enumerate() {
                            current_source_line = code_start_source_line + line_idx;

                            for (syn_style, text) in line_spans {
                                let ratatui_style = crate::syntax::syntect_to_ratatui_fg(*syn_style);

                                for (idx, segment) in text.split('\n').enumerate() {
                                    if idx > 0 {
                                        line_source_map.push(current_source_line);
                                        let line = Line::from(std::mem::take(
                                            &mut self.buffers.current_spans,
                                        ))
                                        .style(code_bg_style);
                                        lines.push(line);
                                        current_source_line += 1;
                                        cur_col_in_line = 0;
                                    }
                                    if !segment.is_empty() {
                                        self.buffers
                                            .current_spans
                                            .push(Span::styled(segment.to_string(), ratatui_style));
                                        cur_col_in_line += unicode_display_width(segment);
                                    }
                                }
                            }

                            if !self.buffers.current_spans.is_empty() {
                                line_source_map.push(current_source_line);
                                let line =
                                    Line::from(std::mem::take(&mut self.buffers.current_spans))
                                        .style(code_bg_style);
                                lines.push(line);
                                cur_col_in_line = 0;
                            }
                        }

                        last_pos = repl.range.end;
                        let newlines_in_code =
                            count_newlines_in_range(repl.range.start, repl.range.end, self.text);
                        current_source_line = code_start_source_line + newlines_in_code;
                        last_line_count_pos = repl.range.end;

                        if checkpoint_output_lines.is_none()
                            && let Some((_, cp_byte)) = checkpoint_info
                            && last_pos >= cp_byte
                        {
                            checkpoint_output_lines = Some(lines.len());
                        }
                    }
                }
                RenderEventKind::Table => {
                    if ev.is_end && table_replace == Some(ev.index) {
                        table_replace = None;
                    } else if !ev.is_end && table_replace.is_none() && pretty {
                        table_replace = Some(ev.index);
                        let trepl = &self.buffers.table_replaces[ev.index];

                        // Flush any in-progress inline spans first. Tables
                        // always start at a line boundary (no-op), but a
                        // display-math block replacement can occur
                        // mid-paragraph (`text $$x$$ more`): without the
                        // flush, the pending "text " spans would be emitted
                        // AFTER the block lines.
                        if !self.buffers.current_spans.is_empty() {
                            line_source_map.push(current_source_line);
                            lines.push(Line::from(std::mem::take(&mut self.buffers.current_spans)));
                            // cur_col_in_line is reset unconditionally after
                            // the block lines are emitted below.
                        }

                        // Update source line to table start
                        if trepl.range.start > last_line_count_pos {
                            current_source_line += count_newlines_in_range(
                                last_line_count_pos,
                                trepl.range.start,
                                self.text,
                            );
                        }
                        let table_start_source_line = current_source_line;
                        let table_base_line = lines.len();

                        for (line_idx, styled_line) in trepl.styled_lines.iter().enumerate() {
                            let offset = trepl
                                .line_source_offsets
                                .get(line_idx)
                                .copied()
                                .unwrap_or(0);
                            current_source_line = table_start_source_line + offset;
                            line_source_map.push(current_source_line);
                            lines.push(styled_line.clone());
                        }
                        // Translate table-local hyperlink coordinates into
                        // absolute line indices and append to the global list.
                        for link in &trepl.hyperlinks {
                            hyperlinks.push(HyperlinkTarget {
                                line_index: table_base_line + link.line_offset,
                                column_range: link.column_range.clone(),
                                url: link.url.clone(),
                                id: link.id,
                            });
                        }
                        // Table emits whole pre-rendered lines; reset col so
                        // any subsequent inline content starts at column 0.
                        cur_col_in_line = 0;

                        last_pos = trepl.range.end;
                        let newlines_in_table =
                            count_newlines_in_range(trepl.range.start, trepl.range.end, self.text);
                        current_source_line = table_start_source_line + newlines_in_table;
                        last_line_count_pos = trepl.range.end;

                        if checkpoint_output_lines.is_none()
                            && let Some((_, cp_byte)) = checkpoint_info
                            && last_pos >= cp_byte
                        {
                            checkpoint_output_lines = Some(lines.len());
                        }
                    }
                }
                RenderEventKind::Highlight => {
                    if ev.is_end {
                        self.buffers.active_highlights.retain(|&x| x != ev.index);
                    } else {
                        self.buffers.active_highlights.push(ev.index);
                    }
                }
            }
        }

        // Handle remaining text
        let len = self.text.len();
        if last_pos < len {
            // Apply force transforms only; non-force transforms have
            // never been applied in this trailing path and force
            // transforms preserve byte length so source offsets below
            // stay valid.
            let raw = &self.text[last_pos..len];
            let transformed = self.apply_transforms(raw, last_pos, false);
            debug_assert_eq!(transformed.len(), raw.len());
            let text: &str = &transformed;
            let is_only_whitespace = text.as_bytes().iter().all(u8::is_ascii_whitespace);

            if !(pretty && is_only_whitespace) {
                if last_pos > last_line_count_pos {
                    current_source_line +=
                        count_newlines_in_range(last_line_count_pos, last_pos, self.text);
                    last_line_count_pos = last_pos;
                }
                let chunk_src_start = last_pos;
                let chunk_src_end = last_pos + text.len();

                // Same cursor-skip pattern as the main path: keep the no-link
                // hot path identical to the pre-feature renderer.
                while next_link_idx < self.buffers.link_targets.len()
                    && self.buffers.link_targets[next_link_idx].source_range.end <= chunk_src_start
                {
                    next_link_idx += 1;
                }
                let chunk_has_links = next_link_idx < self.buffers.link_targets.len()
                    && self.buffers.link_targets[next_link_idx].source_range.start < chunk_src_end;

                // Trailing text bypasses apply_transforms (it's emitted raw),
                // so transformed offsets equal source offsets within the chunk.
                let chunk_links: Vec<ChunkLinkRange> = if chunk_has_links {
                    chunk_link_offsets(
                        &self.buffers.link_targets,
                        next_link_idx,
                        chunk_src_start,
                        chunk_src_end,
                        false,
                        &[],
                    )
                } else {
                    Vec::new()
                };

                let mut byte_offset = last_pos;
                let mut seg_x_offset: usize = 0;
                let is_in_code = in_untagged_code(last_pos, self.buffers);
                pending_line_is_code = is_in_code;

                for (idx, segment) in text.split('\n').enumerate() {
                    if idx > 0 {
                        line_source_map.push(current_source_line);
                        let line = Line::from(std::mem::take(&mut self.buffers.current_spans));
                        lines.push(if is_in_code {
                            line.style(code_bg_style)
                        } else {
                            line
                        });
                        if byte_offset > last_line_count_pos {
                            current_source_line += count_newlines_in_range(
                                last_line_count_pos,
                                byte_offset,
                                self.text,
                            );
                            last_line_count_pos = byte_offset;
                        }
                        cur_col_in_line = 0;
                    }

                    if !chunk_links.is_empty() {
                        emit_segment_hyperlinks(
                            &chunk_links,
                            &self.buffers.link_targets,
                            segment,
                            seg_x_offset,
                            cur_col_in_line,
                            lines.len(),
                            &mut hyperlinks,
                        );
                    }

                    if !segment.is_empty() {
                        self.buffers
                            .current_spans
                            .push(Span::raw(segment.to_string()));
                        cur_col_in_line += unicode_display_width(segment);
                    }
                    byte_offset += segment.len() + 1;
                    seg_x_offset += segment.len() + 1;
                }
            }
        }

        // Emit final line. Use the membership of the chunk that produced these spans:
        // an unterminated bare fence ends its range exactly at last_pos (EOF) and the
        // range check is end-exclusive, so recomputing here would drop the code bg.
        if !self.buffers.current_spans.is_empty() {
            line_source_map.push(current_source_line);
            let final_is_code = pending_line_is_code;
            let line = Line::from(std::mem::take(&mut self.buffers.current_spans));
            lines.push(if final_is_code {
                line.style(code_bg_style)
            } else {
                line
            });
        }

        // If checkpoint wasn't captured during event processing, compute it based on
        // the number of newlines in the text up to checkpoint byte.
        // This handles cases where there are no events past the checkpoint (e.g., incomplete list items).
        if checkpoint_output_lines.is_none()
            && let Some((_, cp_byte)) = checkpoint_info
        {
            // Count newlines in text before the checkpoint byte.
            // Each newline ENDS a line, so N newlines = N complete lines.
            // However, we need to account for blank lines that are absorbed
            // into the block separator. The checkpoint is at the start of
            // the NEXT block, so lines from the frozen content should not
            // include any content that starts at or after cp_byte.
            //
            // More precise approach: count how many output lines have their
            // content entirely before cp_byte. This is tricky without tracking
            // each line's byte range.
            //
            // Logic:
            // - Each newline ENDS a line
            // - Use line_source_map to find output lines before checkpoint
            // - line_source_map[i] is the source line at which output line i was created
            // - source_line_at_cp is the source line containing cp_byte
            // - Output lines with source_line < source_line_at_cp are complete before checkpoint

            let source_line_at_cp = self.text[..cp_byte.min(self.text.len())]
                .bytes()
                .filter(|&b| b == b'\n')
                .count();

            // When the checkpoint is at or past the end of the text, ALL output
            // lines belong to the frozen content (the entire input was consumed
            // by the checkpointed block).  Otherwise, output lines created at
            // source lines strictly before the checkpoint source line are frozen.
            let complete_lines = if cp_byte >= self.text.len() {
                lines.len()
            } else {
                line_source_map
                    .iter()
                    .take_while(|&&src_line| src_line < source_line_at_cp)
                    .count()
            };

            checkpoint_output_lines = Some(complete_lines.min(lines.len()));
        }

        let checkpoint = match (checkpoint_info, checkpoint_output_lines) {
            (Some((kind, source_bytes)), Some(output_lines)) => Some(Checkpoint {
                source_bytes,
                output_lines,
                kind,
            }),
            _ => None,
        };

        // Now that `line_source_map` is final, map each parsed code block's
        // body onto its rendered (pre-wrap) line range.
        let text = self.text;
        let code_blocks = crate::output::build_code_block_spans(
            text,
            &line_source_map,
            std::mem::take(&mut self.buffers.code_blocks),
        );

        (
            MarkdownRenderOutput {
                lines,
                line_source_map,
                hyperlinks,
                code_blocks,
            },
            checkpoint,
        )
    }
}


/// Integration tests for LaTeX math rendering across all four delimiter
/// forms (`$...$`, `$$...$$`, `\(...\)`, `\[...\]`).
#[cfg(test)]
mod math_tests {
    use crate::style::test_style;
    use crate::render_markdown_ratatui_full;

    fn lines_to_text(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn pretty_lines(text: &str) -> Vec<String> {
        let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
        lines_to_text(&output.lines)
    }

    #[test]
    fn dollar_inline_math_renders_unicode() {
        let lines = pretty_lines("Energy is $E = mc^2$ here.\n\n");
        assert_eq!(lines[0], "Energy is E = mc² here.", "got: {lines:#?}");
    }

    #[test]
    fn dollar_inline_math_hides_delimiters_in_pretty_mode() {
        let lines = pretty_lines("So $x_1 + x_2$ holds.\n\n");
        assert!(!lines[0].contains('$'), "got: {lines:#?}");
        assert!(lines[0].contains("x₁ + x₂"), "got: {lines:#?}");
    }

    #[test]
    fn raw_mode_preserves_inline_math_source() {
        let text = "Energy is $E = mc^2$ here.\n\n";
        let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, false, None);
        let lines = lines_to_text(&output.lines);
        assert!(lines[0].contains("$E = mc^2$"), "got: {lines:#?}");
    }

    #[test]
    fn paren_inline_math_renders_unicode() {
        let lines = pretty_lines("Sum \\(\\alpha + \\beta\\) end.\n\n");
        assert_eq!(lines[0], "Sum α + β end.", "got: {lines:#?}");
    }

    #[test]
    fn padded_paren_inline_math_renders_unicode() {
        // Regression: whitespace just inside `\( … \)` made the normalized
        // `$ … $` violate pulldown's dollar-math flanking rule, so it used to
        // render as raw `$ … $`. The normalizer now trims that padding.
        let lines = pretty_lines("Sum \\( x+y \\) end.\n\n");
        assert_eq!(lines[0], "Sum x+y end.", "got: {lines:#?}");
        assert!(
            !lines[0].contains('$'),
            "delimiters must be gone: {lines:#?}"
        );
    }

    #[test]
    fn padded_paren_inline_math_with_braces_renders() {
        let lines = pretty_lines("Set \\( S = \\{ x : x > 0 \\} \\) defined.\n\n");
        let joined = lines.join("\n");
        assert!(joined.contains("x : x > 0"), "got: {lines:#?}");
        assert!(!joined.contains('$'), "no raw dollar math: {lines:#?}");
    }

    #[test]
    fn paren_inline_math_in_list_item() {
        let lines = pretty_lines("- implies \\(p \\to q\\)\n- plain\n\n");
        assert!(lines[0].contains("implies p → q"), "got: {lines:#?}");
    }

    #[test]
    fn paren_inline_math_in_heading() {
        let lines = pretty_lines("## About \\(\\pi^2\\)\n\n");
        assert!(lines[0].contains("About π²"), "got: {lines:#?}");
    }

    #[test]
    fn dollar_inline_math_in_heading() {
        let lines = pretty_lines("# Energy $E=mc^2$\n\n");
        assert!(lines[0].contains("Energy E=mc²"), "got: {lines:#?}");
    }

    #[test]
    fn bracket_display_math_in_heading() {
        // pulldown-cmark keeps heading content inside a `Heading` block (no
        // wrapping paragraph), so the `\[...\]` source scan must also run on
        // heading end. `$$...$$` in the same position already converts via
        // `Event::DisplayMath`.
        let lines = pretty_lines("## Identity \\[x^2 + y^2 = z^2\\]\n\nAfter.\n\n");
        let joined = lines.join("\n");
        assert!(joined.contains("x² + y² = z²"), "got: {lines:#?}");
        assert!(!joined.contains("\\["), "got: {lines:#?}");
    }

    #[test]
    fn escaped_backslash_paren_is_not_math() {
        // `\\(` is a literal backslash followed by a paren — not a math open.
        let lines = pretty_lines("Literal \\\\(x\\\\) here.\n\n");
        let joined = lines.join("\n");
        // Pulldown renders the escapes; no Unicode conversion should occur
        // and the parens must survive.
        assert!(joined.contains("(x"), "got: {lines:#?}");
    }

    #[test]
    fn emphasis_inside_paren_math_falls_back() {
        // `*nope*` becomes emphasis, splitting the text events, so the span
        // is not converted; content must still render.
        let lines = pretty_lines("a \\(*nope*\\) b\n\n");
        let joined = lines.join("\n");
        assert!(joined.contains("nope"), "got: {lines:#?}");
        assert!(!joined.contains('→'), "got: {lines:#?}");
    }

    #[test]
    fn display_math_dollar_renders_block() {
        let lines =
            pretty_lines("Before.\n\n$$\n\\int_0^1 x \\, dx = \\frac{1}{2}\n$$\n\nAfter.\n\n");
        let math_line = lines
            .iter()
            .find(|l| l.contains('∫'))
            .expect("math block line");
        assert_eq!(math_line.trim(), "∫₀¹ x dx = ½", "got: {lines:#?}");
        // Block lines are indented.
        assert!(math_line.starts_with("  "), "got: {lines:#?}");
    }

    #[test]
    fn display_math_dollar_inline_form_renders_block() {
        let lines = pretty_lines("text $$x^2 + y^2 = z^2$$ more\n\n");
        let idx_text = lines.iter().position(|l| l.contains("text")).unwrap();
        let idx_math = lines
            .iter()
            .position(|l| l.contains("x² + y² = z²"))
            .unwrap();
        let idx_more = lines.iter().position(|l| l.contains("more")).unwrap();
        assert!(idx_text < idx_math, "text before math: {lines:#?}");
        assert!(idx_math < idx_more, "math before trailing text: {lines:#?}");
    }

    #[test]
    fn display_math_bracket_renders_block() {
        let text = "The AM-GM inequality:\n\n\\[\n\\frac{a+b}{2} \\ge \\sqrt{ab}\n\\]\n\nDone.\n\n";
        let lines = pretty_lines(text);
        let math_line = lines
            .iter()
            .find(|l| l.contains('≥'))
            .expect("math block line");
        assert_eq!(math_line.trim(), "(a+b)/2 ≥ √(ab)", "got: {lines:#?}");
        assert!(!lines.join("\n").contains("\\["), "got: {lines:#?}");
    }

    #[test]
    fn display_math_bracket_single_line_renders_block() {
        let lines = pretty_lines("\\[E = mc^2\\]\n\nAfter.\n\n");
        let math_line = lines.iter().find(|l| l.contains("mc²")).expect("math line");
        assert_eq!(math_line.trim(), "E = mc²", "got: {lines:#?}");
    }

    #[test]
    fn display_math_bracket_in_raw_mode_shows_canonical_dollars() {
        // The delimiter normalizer rewrites `\[…\]` → `$$…$$` before parsing, so
        // raw mode shows the canonical `$$` form (the math→Unicode conversion is
        // still a pretty-only overlay, so the TeX body itself is preserved).
        let text = "\\[E = mc^2\\]\n\n";
        let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, false, None);
        let joined = lines_to_text(&output.lines).join("\n");
        assert!(joined.contains("$$E = mc^2$$"), "got: {joined:?}");
        assert!(!joined.contains("\\["), "got: {joined:?}");
    }

    #[test]
    fn display_math_with_lone_equals_line_renders_block() {
        // Symptom 1: a lone `=` line inside a display span is a
        // CommonMark setext underline; unjoined, the first line became an H1
        // and the math rendered as raw TeX.
        let text = "The loss:\n\n\\[\n\\boxed{\n\\mathcal{L}_{\\text{MTP}}\n=\n\\sum_{i=0}^{2}\n\\gamma^{i}\\,\n\\mathbb{E}_{\\text{positions, mask}}\n\\Big[\n\\mathrm{KL}\\big(\n  \\mathrm{softmax}(z_{\\text{torso}}^{(s_i)})\n  \\;\\big\\|\\;\n  \\mathrm{softmax}(z_{\\text{draft}}^{(i)})\n\\big)\n\\Big]\n}\n\\]\n\nAfter.\n\n";
        let lines = pretty_lines(text);
        let joined = lines.join("\n");
        let math_line = lines
            .iter()
            .find(|l| l.contains('ℒ'))
            .expect("math block line");
        assert!(math_line.contains("ℒ_(MTP) = ∑ᵢ₌₀²"), "got: {lines:#?}");
        assert!(joined.contains("softmax(z_(torso)"), "got: {lines:#?}");
        assert!(!joined.contains('$'), "no raw delimiters: {lines:#?}");
        assert!(!joined.contains("\\["), "got: {lines:#?}");
        assert!(!joined.contains("boxed"), "got: {lines:#?}");
    }

    #[test]
    fn dollar_display_math_with_lone_equals_line_renders_block() {
        let lines = pretty_lines("$$\nx\n=\ny\n$$\n\nAfter.\n\n");
        let math_line = lines
            .iter()
            .find(|l| l.contains("x = y"))
            .expect("math block line");
        assert!(math_line.starts_with("  "), "block indent: {lines:#?}");
        assert!(!lines.join("\n").contains('$'), "got: {lines:#?}");
    }

    #[test]
    fn text_subscript_in_table_cell_renders_readable() {
        // Symptom 2: `p_{\text{torso}}` in a table cell became the
        // modifier-letter run `pₜₒᵣₛₒ`, which renders with visible gaps in
        // fonts lacking those glyphs.
        let text = "| Who | Soft-teacher |\n|-----|--------------|\n| **Torso** | \\(p_{\\text{torso}}(\\cdot \\mid T_0,\\ldots,T_i)\\) |\n\n";
        let lines = pretty_lines(text);
        let joined = lines.join("\n");
        assert!(joined.contains("p_(torso)(⋅ ∣ T₀,…,Tᵢ)"), "got: {lines:#?}");
        assert!(!joined.contains('ₜ'), "no modifier-letter runs: {lines:#?}");
    }

    #[test]
    fn aligned_environment_renders_multiple_lines() {
        let text =
            "\\[\n\\begin{aligned}\nf(x) &= x^2 \\\\\ng(x) &= 2x\n\\end{aligned}\n\\]\n\nEnd.\n\n";
        let lines = pretty_lines(text);
        let idx_f = lines.iter().position(|l| l.contains("f(x) = x²")).unwrap();
        let idx_g = lines.iter().position(|l| l.contains("g(x) = 2x")).unwrap();
        assert_eq!(idx_g, idx_f + 1, "consecutive block lines: {lines:#?}");
    }

    #[test]
    fn cases_environment_renders_brace_column() {
        let text = "$$\n|x| = \\begin{cases} x & x \\ge 0 \\\\ -x & x < 0 \\end{cases}\n$$\n\n";
        let lines = pretty_lines(text);
        let joined = lines.join("\n");
        assert!(joined.contains('⎧'), "got: {lines:#?}");
        assert!(joined.contains('⎩'), "got: {lines:#?}");
    }

    #[test]
    fn inline_math_in_table_cell_renders_unicode() {
        let text = "| Col | Math |\n|-----|------|\n| a | $x^2 + 1$ |\n\n";
        let lines = pretty_lines(text);
        let joined = lines.join("\n");
        assert!(joined.contains("x² + 1"), "got: {lines:#?}");
        assert!(!joined.contains('$'), "got: {lines:#?}");
    }

    #[test]
    fn paren_inline_math_in_table_cell_renders_unicode() {
        // `\(…\)` inside a table cell must convert. Previously the
        // backslash-form scanner was disabled inside tables, leaving raw TeX.
        // Normalization rewrites `\(…\)` → `$…$` before parsing, so the existing
        // in-cell `$` path converts it.
        let text = "| Mode | Metric |\n|------|--------|\n| Rate | \\(\\alpha + \\beta\\) |\n\n";
        let lines = pretty_lines(text);
        let joined = lines.join("\n");
        assert!(joined.contains("α + β"), "got: {lines:#?}");
        assert!(
            !joined.contains("\\("),
            "raw TeX must not survive: {lines:#?}"
        );
        assert!(!joined.contains('$'), "delimiters hidden: {lines:#?}");
    }

    #[test]
    fn bracket_display_math_in_table_cell_renders_unicode() {
        // `\[…\]` inside a cell renders single-line (no room for a block).
        let text = "| Col | Math |\n|-----|------|\n| a | \\[x^2\\] |\n\n";
        let lines = pretty_lines(text);
        let joined = lines.join("\n");
        assert!(joined.contains("x²"), "got: {lines:#?}");
        assert!(!joined.contains("\\["), "got: {lines:#?}");
    }

    #[test]
    fn paren_inline_math_in_blockquote_renders_unicode() {
        let lines = pretty_lines("> energy \\(E = mc^2\\) noted\n\n");
        let joined = lines.join("\n");
        assert!(joined.contains("E = mc²"), "got: {lines:#?}");
        assert!(!joined.contains("\\("), "got: {lines:#?}");
    }

    #[test]
    fn equation_environment_converts_to_block() {
        let text = "Before.\n\n\\begin{equation}\nE = mc^2\n\\end{equation}\n\nAfter.\n\n";
        let lines = pretty_lines(text);
        let joined = lines.join("\n");
        assert!(joined.contains("E = mc²"), "got: {lines:#?}");
        assert!(!joined.contains("\\begin"), "got: {lines:#?}");
    }

    #[test]
    fn latex_in_code_span_left_verbatim() {
        // Code spans are verbatim: `\(…\)` inside backticks must NOT convert.
        let lines = pretty_lines("inline `\\(x\\)` code\n\n");
        let joined = lines.join("\n");
        assert!(joined.contains("\\(x\\)"), "code must stay raw: {lines:#?}");
    }

    #[test]
    fn display_math_in_blockquote_renders() {
        let lines = pretty_lines("> Einstein: $$E = mc^2$$\n\n");
        let joined = lines.join("\n");
        assert!(joined.contains("E = mc²"), "got: {lines:#?}");
    }

    #[test]
    fn oversized_inline_math_falls_back_to_code_styling() {
        let body = "x".repeat(crate::latex::MAX_MATH_SOURCE_LEN + 10);
        let text = format!("Big ${body}$ end.\n\n");
        let lines = pretty_lines(&text);
        let joined = lines.join("\n");
        // Content is preserved verbatim (code-style fallback), delimiters
        // hidden in pretty mode.
        assert!(joined.contains(&body), "fallback must keep raw content");
    }

    #[test]
    fn bracket_math_inside_link_label_keeps_link_target() {
        // Option A normalizes `\[x\]` → `$$x$$` everywhere outside code, so (like
        // a literal `$$…$$`) display math inside a link label now converts. This
        // construct — display math inside a link label — is degenerate and
        // exceedingly rare in model output; the invariant we keep is that the
        // link target survives.
        let lines = pretty_lines("See [\\[x\\] notes](https://example.com) now.\n\n");
        let joined = lines.join("\n");
        assert!(
            joined.contains("https://example.com"),
            "link must survive: {lines:#?}"
        );
    }

    #[test]
    fn unclosed_math_renders_without_panic() {
        for text in [
            "open $a + b\n\n",
            "open $$a + b\n\n",
            "open \\(a + b\n\n",
            "open \\[a + b\n\n",
            "$$\n\\frac{1}{\n\n",
            "\\]\n\n",
            "\\)\n\n",
        ] {
            let _ = pretty_lines(text);
        }
    }

    #[test]
    fn multiple_inline_math_spans_in_one_paragraph() {
        let lines = pretty_lines("Both $a^2$ and \\(b_1\\) and $c \\ne d$ work.\n\n");
        assert_eq!(
            lines[0], "Both a² and b₁ and c ≠ d work.",
            "got: {lines:#?}"
        );
    }

    #[test]
    fn greek_and_symbols_inline() {
        let lines =
            pretty_lines("Rate $\\lambda \\approx 0.5$ and set $S \\subseteq \\mathbb{R}^n$.\n\n");
        assert_eq!(lines[0], "Rate λ ≈ 0.5 and set S ⊆ ℝⁿ.", "got: {lines:#?}");
    }
}

/// Tests for HTML character-entity decoding in prose (`&lt;` → `<`, etc.).
#[cfg(test)]
mod entity_tests {
    use crate::style::test_style;
    use crate::render_markdown_ratatui_full;

    fn lines_to_text(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn pretty_lines(text: &str) -> Vec<String> {
        let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, true, None);
        lines_to_text(&output.lines)
    }

    fn raw_lines(text: &str) -> Vec<String> {
        let (output, _) = render_markdown_ratatui_full(text, test_style::STYLE, false, None);
        lines_to_text(&output.lines)
    }

    #[test]
    fn lt_gt_amp_decoded_in_prose() {
        let lines = pretty_lines("Use &lt;tag&gt; with a &amp; b.\n\n");
        assert_eq!(lines[0], "Use <tag> with a & b.", "got: {lines:#?}");
    }

    #[test]
    fn multiple_entities_one_paragraph() {
        let lines = pretty_lines("1 &lt; 2 &amp;&amp; 3 &gt; 2\n\n");
        assert_eq!(lines[0], "1 < 2 && 3 > 2", "got: {lines:#?}");
    }

    #[test]
    fn quote_and_apostrophe_entities() {
        let lines = pretty_lines("&quot;hello&quot; &amp; &#39;world&#39;\n\n");
        assert_eq!(lines[0], "\"hello\" & 'world'", "got: {lines:#?}");
    }

    #[test]
    fn numeric_decimal_and_hex_entities() {
        // &#60; = '<', &#x3e; = '>'
        let lines = pretty_lines("a &#60;b&#x3e; c\n\n");
        assert_eq!(lines[0], "a <b> c", "got: {lines:#?}");
    }

    #[test]
    fn full_html5_named_entities_decoded() {
        // Beyond the XML core set: these must decode in prose just like they
        // already do in table cells (via pulldown), keeping the two consistent.
        let lines = pretty_lines("&mdash; &copy; &hellip; &rarr; &times;\n\n");
        assert_eq!(lines[0], "— © … → ×", "got: {lines:#?}");
    }

    #[test]
    fn nbsp_decodes_to_no_break_space() {
        let lines = pretty_lines("a&nbsp;b\n\n");
        assert_eq!(lines[0], "a\u{a0}b", "got: {lines:#?}");
    }

    #[test]
    fn control_char_entities_are_not_injected() {
        // ESC / BEL / NUL / CR must never be substituted into terminal output;
        // the source stays literal instead.
        for (src, literal) in [
            ("x &#27; y\n\n", "&#27;"),
            ("x &#x1b; y\n\n", "&#x1b;"),
            ("x &#7; y\n\n", "&#7;"),
            ("x &#0; y\n\n", "&#0;"),
        ] {
            let lines = pretty_lines(src);
            let joined = lines.join("\n");
            assert!(
                joined.contains(literal),
                "control entity must stay literal: src={src:?} got={lines:#?}"
            );
            assert!(
                !joined.chars().any(|c| c.is_control() && c != '\n'),
                "no control char injected: src={src:?} got={lines:#?}"
            );
        }
    }

    #[test]
    fn entity_inside_link_text_decodes_and_keeps_link() {
        let lines = pretty_lines("See [a &lt; b](https://example.com) end.\n\n");
        let joined = lines.join("\n");
        assert!(joined.contains("a < b"), "link text decoded: {lines:#?}");
        assert!(
            joined.contains("https://example.com"),
            "link url survives: {lines:#?}"
        );
        assert!(!joined.contains("&lt;"), "no literal entity: {lines:#?}");
    }

    #[test]
    fn entity_inside_inline_math_does_not_corrupt() {
        // The entity sits inside a `\(...\)` math span; the math transform owns
        // those bytes, so the entity scan must not add an overlapping transform.
        let lines = pretty_lines("eq \\(a &lt; b\\) end\n\n");
        let joined = lines.join("\n");
        assert!(joined.contains("end"), "trailing text intact: {lines:#?}");
        // No doubled fragments from overlapping transforms.
        assert!(!joined.contains("endend"), "no double emit: {lines:#?}");
    }

    #[test]
    fn raw_mode_preserves_entity_source() {
        let lines = raw_lines("Use &lt;tag&gt; here.\n\n");
        assert!(
            lines[0].contains("&lt;tag&gt;"),
            "raw mode must keep source: {lines:#?}"
        );
    }

    #[test]
    fn entities_decoded_inside_emphasis_and_heading() {
        let bold = pretty_lines("**a &lt; b**\n\n");
        assert_eq!(bold[0], "a < b", "got: {bold:#?}");
        let heading = pretty_lines("## Compare &lt;T&gt;\n\n");
        assert!(
            heading.iter().any(|l| l.contains("Compare <T>")),
            "got: {heading:#?}"
        );
    }

    #[test]
    fn entities_left_literal_in_code() {
        // Inline code and fenced blocks are intentionally verbatim.
        let inline = pretty_lines("call `vec&lt;i32&gt;` now.\n\n");
        assert!(
            inline.iter().any(|l| l.contains("vec&lt;i32&gt;")),
            "inline code stays literal: {inline:#?}"
        );
        let fenced = pretty_lines("```\nGeneric&lt;T&gt;\n```\n\n");
        assert!(
            fenced.iter().any(|l| l.contains("Generic&lt;T&gt;")),
            "code block stays literal: {fenced:#?}"
        );
    }

    #[test]
    fn unknown_or_bare_ampersand_untouched() {
        // No semicolon, unknown name, and a lone `&` must all pass through.
        let lines = pretty_lines("Tom &amp Jerry &unknown; plain & text\n\n");
        assert_eq!(
            lines[0], "Tom &amp Jerry &unknown; plain & text",
            "got: {lines:#?}"
        );
    }

    #[test]
    fn entity_in_table_cell_still_decodes() {
        // Regression guard: the table cell path already decoded entities; this
        // must keep working alongside the new prose path.
        let lines = pretty_lines("| H |\n|---|\n| a &lt; b |\n\n");
        let joined = lines.join("\n");
        assert!(joined.contains("a < b"), "got: {lines:#?}");
    }

    #[test]
    fn no_panic_on_entity_edge_cases() {
        for text in [
            "&\n\n",
            "&;\n\n",
            "&#;\n\n",
            "&#x;\n\n",
            "&#0;\n\n",
            "&#27;\n\n",
            "&#x1b;\n\n",
            "trailing &lt",
            "&lt;&gt;&amp;",
            "&#xZZ;\n\n",
            "&CounterClockwiseContourIntegral;\n\n",
            // Multi-byte UTF-8 mixed with `&` in various positions: the inner
            // loop only advances over ASCII bytes, so it must not slice
            // through a multi-byte sequence.
            "& é &lt; ñ\n\n",
            "café &lt; thé\n\n",
            "🦀 & 🦀\n\n",
            "&amp;🦀&lt;\n\n",
            // Repeated `&` runs (worst case for the O(n²) bound).
            "&&&&&&&&&&&&\n\n",
            &("&".repeat(200) + "\n\n"),
        ] {
            let _ = pretty_lines(text);
        }
    }
}