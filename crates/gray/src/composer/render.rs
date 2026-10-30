use super::*;

impl Tui {
    pub(crate) fn reanchor_viewport(&mut self, cols: u16) {
        self.last_width = cols;
        if let Ok((_, rows)) = crossterm::terminal::size() {
            self.last_height = rows;
        }
        self.pending_resize = None;

        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
        self.atomic(|t| {
            let _ = t.terminal.clear();
        });
    }

    pub(super) fn commit_markdown_tail(&mut self) {
        self.drain_reveal();

        if !self.stream_punct_hold.is_empty() {
            let held = std::mem::take(&mut self.stream_punct_hold);
            self.markdown_renderer
                .push_and_render(&held, Some(gray_markdown::get_syntect()));
        }
        let output = std::mem::replace(
            &mut self.markdown_renderer,
            gray_markdown::StreamingMarkdownRenderer::new(
                gray_markdown::gray_markdown_style(),
                true,
            ),
        )
        .finish_tail_into_output(Some(gray_markdown::get_syntect()));
        if output.lines.len() > self.committed_markdown_lines {
            if self.committed_markdown_lines == 0 {
                self.ensure_gap();
            }
            let remaining_lines: Vec<Line<'static>> =
                output.lines[self.committed_markdown_lines..].to_vec();
            let offset = self.committed_markdown_lines;

            self.push_styled_lines_with_hyperlinks(remaining_lines, &output.hyperlinks, offset);
        }
        self.committed_markdown_lines = 0;
    }

    pub(crate) fn begin_sync(&mut self) {
        if self.sync_depth == 0 {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::BeginSynchronizedUpdate
            );
        }
        self.sync_depth += 1;
    }

    pub(crate) fn end_sync(&mut self) {
        self.sync_depth = self.sync_depth.saturating_sub(1);
        if self.sync_depth == 0 {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::terminal::EndSynchronizedUpdate
            );
        }
    }

    pub(crate) fn atomic<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.begin_sync();
        self.batch_depth += 1;
        let out = f(self);
        self.batch_depth -= 1;
        if self.batch_depth == 0 {
            let _ = self.draw();
        }
        self.end_sync();
        out
    }

    pub(crate) fn insert_paragraph(
        &mut self,
        lines: &[Line<'static>],
        bg: Option<ratatui::style::Color>,
    ) {
        if self.modal_open {
            self.modal_pending
                .push(PendingInsert::Rows(lines.to_vec(), bg));
            return;
        }
        if let Some(batch) = &mut self.insert_batch {
            batch.extend(lines.iter().cloned().map(|line| (line, Vec::new(), bg)));
            return;
        }
        let height = lines.len() as u16;
        self.atomic(|t| {
            let _ = draw::settle_band(t);
            let _ = t.terminal.insert_before(height, |buf| {
                let mut p = Paragraph::new(lines.to_vec());
                if let Some(bg) = bg {
                    p = p.block(Block::default().style(Style::default().bg(bg)));
                }
                p.render(buf.area, buf);
            });
        });
    }

    pub(crate) fn drain_inflight_for_reflow(&mut self) {
        if self.thinking && !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.append_thinking_text(&rest, false);
        }
        // Do NOT commit the markdown tail here: finishing the renderer
        // mid-stream freezes the still-growing paragraph into history and the
        // rest of the reply continues in a fresh renderer as a second block —
        // a word torn across the seam plus a gap row. The live tail stays in
        // `markdown_renderer` and `stream_tail_rows` re-wraps it at the new
        // width on the next draw.
        self.drain_reveal();
    }

    pub(crate) fn repaint_for_palette(&mut self) {
        if palette_repaint_due(self.painted_palette, crate::theme::theme(), self.modal_open) {
            self.reflow_on_resize(self.last_width);
        }
    }

    pub(crate) fn reflow_on_resize(&mut self, new_cols: u16) {
        self.atomic(|t| t.reflow_on_resize_inner(new_cols));
    }

    pub(super) fn reflow_on_resize_inner(&mut self, new_cols: u16) {
        // One synchronized update: the terminal never shows the cleared
        // screen between the wipe and the repaint (the intro handoff used
        // to blank the finished logo for a frame).
        self.begin_sync();
        self.reflow_unsynced(new_cols);
        self.end_sync();
    }

    pub(super) fn reflow_unsynced(&mut self, new_cols: u16) {
        self.painted_palette = crate::theme::theme();
        let _ = self.hide_background();
        self.last_width = new_cols;
        if let Ok((_, rows)) = crossterm::terminal::size() {
            self.last_height = rows;
        }

        self.drain_inflight_for_reflow();

        self.modal_pending.clear();

        let (cols, rows) = crossterm::terminal::size().unwrap_or((new_cols, self.last_height));
        let _ = self
            .terminal
            .clear_scrollback_and_screen(ratatui::layout::Size::new(cols, rows));

        let w = new_cols as usize;
        self.transcript.clear();
        self.gap_owed = false;
        self.begin_insert_batch();
        let entries = std::mem::take(&mut self.history_entries);
        for entry in &entries {
            match entry {
                TranscriptEntry::Welcome => {
                    let lines = build_welcome_lines(w);
                    self.record_rows(lines.clone());
                    self.insert_paragraph(&lines, Some(crate::theme::theme().window_bg));
                }
                TranscriptEntry::UserPrompt(text, attached) => {
                    let lines =
                        crate::composer::transcript::format_user_prompt_lines(text, attached, w);
                    self.emit_rows(lines, Some(crate::theme::theme().surface_bg));
                }
                TranscriptEntry::ToolBox { header, body } => {
                    let lines =
                        crate::composer::transcript::format_tool_box_lines(header.clone(), body, w);
                    self.emit_rows(lines, Some(crate::theme::theme().surface_bg));
                }
                TranscriptEntry::ToolBoxLines { header, body } => {
                    let lines =
                        crate::composer::transcript::format_tool_box_lines_multi(header, body, w);
                    self.emit_rows(lines, Some(crate::theme::theme().surface_bg));
                }
                TranscriptEntry::StyledLines { lines, hyperlinks } => {
                    self.emit_styled(lines.clone(), hyperlinks.clone());
                }
                TranscriptEntry::ThinkingRun(text) => {
                    let rows = crate::composer::transcript::thinking_run_rows(
                        text,
                        crate::composer::transcript::prose_width(w),
                        false,
                    );
                    self.emit_rows(rows, Some(crate::theme::theme().window_bg));
                }
                TranscriptEntry::Gap(_) => self.gap_owed = true,
                TranscriptEntry::Mascot => {
                    if let Some(lines) = crate::mascot::mascot_lines(
                        u16::try_from(w).unwrap_or(u16::MAX),
                        self.last_height.max(1),
                        Some(w),
                    ) {
                        self.emit_styled(lines, Vec::new());
                    }
                }
            }
        }
        self.history_entries = entries;
        self.end_insert_batch();

        let _ = self.draw();
    }

    pub(crate) fn retract_slash_echo(&mut self, cmd: &str) -> bool {
        let echoed = matches!(
            self.history_entries.last(),
            Some(TranscriptEntry::UserPrompt(text, _)) if text.trim() == cmd
        );
        if !echoed {
            return false;
        }
        self.history_entries.pop();
        self.reflow_on_resize(self.last_width);
        true
    }

    pub(crate) fn pop_mascot(&mut self) -> bool {
        crate::mascot::set_mascot_shown(false);
        if !crate::composer::transcript::drop_mascot_entry(&mut self.history_entries) {
            return false;
        }
        self.reflow_on_resize(self.last_width);
        true
    }
}
