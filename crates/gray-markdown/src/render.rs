use std::borrow::Cow;

use ratatui::text::{Line, Span};

use crate::buffers::{MarkdownBuffers, RenderEvent, RenderEventKind, unicode_display_width};
use crate::checkpoint::Checkpoint;
use crate::colors::anstyle_to_ratatui_style;
use crate::hyperlinks::{ChunkLinkRange, chunk_link_offsets, emit_segment_hyperlinks};
use crate::output::{HyperlinkTarget, MarkdownRenderOutput};
use crate::parse::ParsedMarkdown;
use crate::style::{all_hidden, merge_styles};

impl<'a, 'b> ParsedMarkdown<'a, 'b> {
    fn apply_transforms<'t>(&self, text: &'t str, start: usize, pretty: bool) -> Cow<'t, str> {
        if self.buffers.transforms.is_empty() {
            return Cow::Borrowed(text);
        }

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

            let t_start = transform.range.start.max(start);
            let t_end = transform.range.end.min(end);

            if t_start > pos {
                let before = &text[(pos - start)..(t_start - start)];
                result.push_str(before);
            }

            result.push_str(&transform.to);

            pos = t_end;
        }

        if !applied {
            Cow::Borrowed(text)
        } else {
            if pos < end {
                result.push_str(&text[(pos - start)..]);
            }
            Cow::Owned(result)
        }
    }

    fn build_render_events(&self) -> Vec<RenderEvent> {
        let mut events = Vec::new();
        let capacity = self.buffers.highlights.len() * 2
            + self.buffers.replaces.len() * 2
            + self.buffers.table_replaces.len() * 2;
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

        events.sort_unstable();
        events
    }

    pub fn render_ratatui(&mut self, pretty: bool) -> (MarkdownRenderOutput, Option<Checkpoint>) {
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

        let mut cur_col_in_line: usize = 0;

        let checkpoint_info = self.last_checkpoint;
        let mut checkpoint_output_lines: Option<usize> = None;

        let code_bg_style: ratatui::style::Style =
            anstyle_to_ratatui_style(self.ms.code_background);

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

            text.as_bytes()[from..to]
                .iter()
                .filter(|&&b| b == b'\n')
                .count()
        };

        for ev in &render_events {
            if replace.is_none() && table_replace.is_none() && ev.pos > last_pos {
                let split_at_checkpoint = checkpoint_output_lines.is_none()
                    && checkpoint_info
                        .map(|(_, cp_byte)| last_pos < cp_byte && cp_byte <= ev.pos)
                        .unwrap_or(false);

                let cp_byte = checkpoint_info.map(|(_, cp)| cp).unwrap_or(0);

                let cp_byte = {
                    let mut b = cp_byte;
                    while b < self.text.len() && !self.text.is_char_boundary(b) {
                        b += 1;
                    }
                    b
                };

                let ranges: &[(usize, usize)] = if split_at_checkpoint {
                    &[(last_pos, cp_byte), (cp_byte, ev.pos)]
                } else {
                    &[(last_pos, ev.pos)]
                };

                for (range_idx, &(range_start, range_end)) in ranges.iter().enumerate() {
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
                            let hidden_text = self.text[range_start..range_end].trim_start();
                            let is_code_fence =
                                hidden_text.starts_with("```") || hidden_text.starts_with("~~~");

                            if is_code_fence {
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
                            let ratatui_style: ratatui::style::Style =
                                anstyle_to_ratatui_style(style);

                            let chunk_src_start = text_start;
                            let chunk_src_end = text_start + text.len();

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
                                let ratatui_style =
                                    crate::syntax::syntect_to_ratatui_fg(*syn_style);

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

                        if !self.buffers.current_spans.is_empty() {
                            line_source_map.push(current_source_line);
                            lines.push(Line::from(std::mem::take(&mut self.buffers.current_spans)));
                        }

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

                        for link in &trepl.hyperlinks {
                            hyperlinks.push(HyperlinkTarget {
                                line_index: table_base_line + link.line_offset,
                                column_range: link.column_range.clone(),
                                url: link.url.clone(),
                                id: link.id,
                            });
                        }

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

        let len = self.text.len();
        if last_pos < len {
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

                while next_link_idx < self.buffers.link_targets.len()
                    && self.buffers.link_targets[next_link_idx].source_range.end <= chunk_src_start
                {
                    next_link_idx += 1;
                }
                let chunk_has_links = next_link_idx < self.buffers.link_targets.len()
                    && self.buffers.link_targets[next_link_idx].source_range.start < chunk_src_end;

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

        if checkpoint_output_lines.is_none()
            && let Some((_, cp_byte)) = checkpoint_info
        {
            let source_line_at_cp = self.text[..cp_byte.min(self.text.len())]
                .bytes()
                .filter(|&b| b == b'\n')
                .count();

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

        (
            MarkdownRenderOutput {
                lines,
                line_source_map,
                hyperlinks,
            },
            checkpoint,
        )
    }
}
