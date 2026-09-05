//! Transcript box/line insertion (`impl Tui`, split from `transcript`).

use super::*;

impl Tui {
    pub fn push_tool_box(&mut self, header: Line<'static>, body: Vec<Line<'static>>) {
        self.insert_tool_box(header, body);
        self.ensure_gap(1);
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = std::io::stdout().flush();
    }

    /// Tool box with no trailing gap: the card hugs the input box. Used for
    /// the gateway boot card (its final state is committed once, in one message).
    pub fn push_tool_box_no_gap(&mut self, header: Line<'static>, body: Vec<Line<'static>>) {
        self.insert_tool_box(header, body);
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = std::io::stdout().flush();
    }

    fn insert_tool_box(&mut self, header: Line<'static>, body: Vec<Line<'static>>) {
        self.ensure_gap(1);
        let w = self.width().max(10);
        let box_lines = format_tool_box_lines(header.clone(), &body, w);
        let height = box_lines.len() as u16;
        let block =
            ratatui::widgets::Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)));
        let _ = self.terminal.insert_before(height, |buf| {
            Paragraph::new(box_lines.clone())
                .block(block)
                .render(buf.area, buf);
        });
        self.history_entries
            .push(crate::composer::TranscriptEntry::ToolBox { header, body });
        self.transcript.extend(box_lines);
    }

    /// Live gateway boot panel: rendered in the viewport above the input
    /// while platforms connect, then committed as ONE static card.
    /// `header` is e.g. "Gateway autostarted"; rows come from
    /// [`crate::repl::gateway_boot_rows`] (`  └─ Discord — connecting…`).
    pub fn begin_gateway_boot(
        &mut self,
        header: &str,
        board: &gray_gateway::status::GatewayStatusBoard,
    ) {
        self.gateway_boot = Some(GatewayBootPanel {
            header: header.to_string(),
            rows: crate::repl::gateway_boot_rows(board),
        });
        let _ = self.draw();
    }

    /// Refreshes the live panel rows. Never touches `status`: the boot card
    /// above the input is the only boot indicator (no top shimmer row).
    pub fn refresh_gateway_boot(&mut self, board: &gray_gateway::status::GatewayStatusBoard) {
        if let Some(p) = self.gateway_boot.as_mut() {
            p.rows = crate::repl::gateway_boot_rows(board);
        }
    }

    /// Clears the live panel + status and commits the final board state as
    /// ONE card, followed by one bare row so the card never fuses with the
    /// input band. Same formatter + same painter as the live panel, so the
    /// commit is a no-op visually: nothing shifts, nothing restyles.
    pub fn finish_gateway_boot(&mut self, board: &gray_gateway::status::GatewayStatusBoard) {
        let Some(panel) = self.gateway_boot.take() else {
            return;
        };
        if self.status_is_gateway_or_empty() {
            self.status = None;
        }
        let (header, body) =
            gateway_boot_card_parts(&panel.header, &crate::repl::gateway_boot_rows(board));
        self.ensure_gap(1);
        let w = self.width().max(10);
        let box_lines = format_gateway_boot_card(header.clone(), &body, w);
        let height = box_lines.len() as u16;
        let _ = self.terminal.insert_before(height, |buf| {
            paint_card(&box_lines, buf.area, buf);
        });
        self.history_entries
            .push(crate::composer::TranscriptEntry::ToolBox { header, body });
        self.transcript.extend(box_lines);
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = std::io::stdout().flush();
        // Same bare row the live panel keeps between the card and the input
        // band (draw.rs `boot_gap_h`), so committing never moves the input.
        self.ensure_gap(1);
        let _ = self.draw();
    }

    fn status_is_gateway_or_empty(&self) -> bool {
        self.status
            .as_ref()
            .map(|(_, l)| l.starts_with("Gateway"))
            .unwrap_or(true)
    }

    pub fn push_line(&mut self, line: String) {
        self.push_line_styled(line, Style::default());
    }

    pub(crate) fn push_line_styled(&mut self, line: String, style: Style) {
        let l = Line::from(vec![Span::styled(line, style)]);
        self.push_styled_lines_with_hyperlinks(vec![l], &[], 0);
    }

    pub fn push_line_spans(&mut self, line: Line<'static>) {
        self.push_styled_lines_with_hyperlinks(vec![line], &[], 0);
    }

    pub fn push_styled_lines(&mut self, lines: Vec<Line<'static>>) {
        self.push_styled_lines_with_hyperlinks(lines, &[], 0);
    }

    pub(crate) fn render_and_insert_styled_lines(
        &mut self,
        lines: &[Line<'static>],
        hyperlinks: &[HyperlinkTarget],
        width: usize,
    ) -> Vec<Line<'static>> {
        if lines.is_empty() {
            return Vec::new();
        }
        let max_w = width.saturating_sub(2).max(1);
        let mut by_line: HashMap<usize, Vec<&HyperlinkTarget>> = HashMap::new();
        for h in hyperlinks {
            by_line.entry(h.line_index).or_default().push(h);
        }
        let mut all_wrapped: Vec<(Line<'static>, Vec<HyperlinkTarget>)> = Vec::new();
        for (idx, line) in lines.iter().enumerate() {
            let line_hyperlinks = by_line.get(&idx).cloned().unwrap_or_default();
            for (mut l, src_range) in wrap_styled_line_with_ranges(line.clone(), max_w) {
                let hl_owned: Vec<HyperlinkTarget> = if src_range.end == usize::MAX {
                    line_hyperlinks.iter().map(|h| (*h).clone()).collect()
                } else {
                    // Translate each hyperlink's absolute columns into this
                    // row's coordinates; drop parts landing on other rows.
                    line_hyperlinks
                        .iter()
                        .filter_map(|h| {
                            let s = h.column_range.start.max(src_range.start);
                            let e = h.column_range.end.min(src_range.end);
                            if s >= e {
                                return None;
                            }
                            let mut hc = (*h).clone();
                            hc.column_range = (s - src_range.start)..(e - src_range.start);
                            Some(hc)
                        })
                        .collect()
                };
                if !l.spans.is_empty() {
                    l.spans.insert(0, left_pad());
                }
                all_wrapped.push((l, hl_owned));
            }
        }
        let total_h = all_wrapped.len() as u16;
        let lines_only: Vec<Line<'static>> = all_wrapped.iter().map(|(l, _)| l.clone()).collect();
        let _ = self.terminal.insert_before(total_h, |buf| {
            let area = buf.area;
            for (i, (line, hls)) in all_wrapped.iter().enumerate() {
                let row_area = ratatui::layout::Rect {
                    x: area.x,
                    y: area.y + i as u16,
                    width: area.width,
                    height: 1,
                };
                Paragraph::new(line.clone()).render(row_area, buf);
                for h in hls {
                    let pad = crate::tui::padding_x(1);
                    for col in h.column_range.clone() {
                        let padded_col = col + pad;
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
        });
        lines_only
    }

    pub fn push_styled_lines_with_hyperlinks(
        &mut self,
        lines: Vec<Line<'static>>,
        hyperlinks: &[HyperlinkTarget],
        _line_offset: usize,
    ) {
        if lines.is_empty() {
            return;
        }
        let w = self.width().max(10);
        let lines_only = self.render_and_insert_styled_lines(&lines, hyperlinks, w);
        self.history_entries
            .push(crate::composer::TranscriptEntry::StyledLines {
                lines,
                hyperlinks: hyperlinks.to_vec(),
            });
        self.transcript.extend(lines_only);
        if self.transcript.len() > 1000 {
            self.transcript.drain(0..100);
        }
        let _ = std::io::stdout().flush();
    }

    pub fn push_dim(&mut self, line: String) {
        let styled = Line::from(vec![Span::styled(
            line,
            Style::new().add_modifier(Modifier::DIM),
        )]);
        self.push_styled_lines_with_hyperlinks(vec![styled], &[], 0);
    }

    pub fn push_action(&mut self, text: &str, detail: Option<&str>) {
        let mut spans = vec![
            Span::styled(
                "✓ ",
                Style::default()
                    .fg(Color::Rgb(74, 222, 128))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                text.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(d) = detail {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                d.to_string(),
                Style::default().fg(Color::Rgb(140, 140, 140)),
            ));
        }
        let line = Line::from(spans);
        self.push_styled_lines_with_hyperlinks(vec![line], &[], 0);
    }

    /// Re-renders a `request_user_input` Q&A from stored history so resumed
    /// sessions show what was asked and answered (previously skipped).
    pub fn push_question_replay(&mut self, args: Option<&serde_json::Value>, content: &str) {
        let questions = args
            .and_then(|a| a.get("questions"))
            .and_then(|q| q.as_array())
            .cloned()
            .unwrap_or_default();
        // ToolResult content is `{"answers":{id:{"answers":[...]}}}`.
        let answer_map: HashMap<String, Vec<String>> =
            serde_json::from_str::<serde_json::Value>(content)
                .ok()
                .and_then(|v| v.get("answers").cloned())
                .and_then(|v| v.as_object().cloned())
                .map(|obj| {
                    obj.into_iter()
                        .map(|(k, v)| {
                            let list = v
                                .get("answers")
                                .and_then(|a| a.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                        .collect()
                                })
                                .unwrap_or_default();
                            (k, list)
                        })
                        .collect()
                })
                .unwrap_or_default();
        let mut lines = Vec::new();
        for q in &questions {
            let id = q.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let text = q
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("question");
            match answer_map.get(id) {
                Some(list) if !list.is_empty() => {
                    lines.push(format!("• {text}"));
                    lines.push(format!("  {} answered: {}", "↳", list.join(" · ")));
                }
                // Stacked like answered: outcome on its own row, never inline.
                _ => {
                    lines.push(format!("• {text}"));
                    lines.push("  ↳ skipped".to_string());
                }
            }
        }
        if !lines.is_empty() {
            self.push_dim(lines.join("\n"));
        }
    }

    /// Replays a previous session's message history into the TUI scrollback.
    pub fn replay_session_history(
        &mut self,
        entries: &[gray_session::SessionEntry],
        cwd: &std::path::Path,
    ) {
        let mut tool_calls: HashMap<String, (String, serde_json::Value)> = HashMap::new();
        for entry in entries {
            match entry.message.role {
                gray_core::Role::User => {
                    let mut user_text = String::new();
                    for block in &entry.message.content {
                        match block {
                            gray_core::ContentBlock::Text { text } => {
                                if !user_text.is_empty() {
                                    user_text.push('\n');
                                }
                                user_text.push_str(text);
                            }
                            gray_core::ContentBlock::ToolResult {
                                id,
                                content,
                                is_error,
                            } => {
                                let (name, args) = tool_calls
                                    .remove(id)
                                    .map(|(n, a)| (n, Some(a)))
                                    .unwrap_or_else(|| ("tool".to_string(), None));
                                if name == "request_user_input" {
                                    self.push_question_replay(args.as_ref(), content);
                                } else {
                                    let header = args
                                        .as_ref()
                                        .map(|a| {
                                            crate::tool_fmt::format_tool_call_header(
                                                &name,
                                                a,
                                                Some(cwd),
                                            )
                                        })
                                        .unwrap_or_else(|| ratatui::text::Line::from(name.clone()));
                                    let lines =
                                        crate::tool_fmt::format_tool_result_lines_with_context(
                                            &name,
                                            args.as_ref(),
                                            content,
                                            *is_error,
                                            Some(cwd),
                                        );
                                    self.push_tool_box(header, lines);
                                }
                            }
                            _ => {}
                        }
                    }
                    if !user_text.is_empty() {
                        self.push_user_prompt(&user_text, &[], true);
                        // Feed composer input history so Up/Down recall works
                        // for prompts from the resumed session.
                        self.history.push(user_text.clone());
                        if self.history.len() > 100 {
                            self.history.remove(0);
                        }
                        self.history_idx = None;
                    }
                }
                gray_core::Role::Assistant => {
                    for block in &entry.message.content {
                        match block {
                            gray_core::ContentBlock::Thinking { .. } => {}
                            gray_core::ContentBlock::Text { text } => {
                                let clean = strip_ansi(text);
                                if !clean.trim().is_empty() {
                                    self.ensure_gap(1);
                                    // Same budget the insert wrapper will use (width-2):
                                    // tables fit by construction instead of shredding.
                                    let tw = self.width().max(10).saturating_sub(2);
                                    let mut buffers = gray_markdown::MarkdownBuffers::new();
                                    let (output, _) =
                                        gray_markdown::render_markdown_ratatui_with_buffers_width(
                                            &clean,
                                            gray_markdown::gray_markdown_style(),
                                            true,
                                            &mut buffers,
                                            Some(gray_markdown::get_syntect()),
                                            Some(tw),
                                        );
                                    self.push_styled_lines_with_hyperlinks(
                                        output.lines,
                                        &output.hyperlinks,
                                        0,
                                    );
                                }
                            }
                            gray_core::ContentBlock::ToolUse { id, name, args } => {
                                tool_calls.insert(id.clone(), (name.clone(), args.clone()));
                            }
                            _ => {}
                        }
                    }
                }
                gray_core::Role::System => {
                    for block in &entry.message.content {
                        if let gray_core::ContentBlock::ToolResult {
                            id,
                            content,
                            is_error,
                        } = block
                        {
                            let (name, args) = tool_calls
                                .remove(id)
                                .map(|(n, a)| (n, Some(a)))
                                .unwrap_or_else(|| ("tool".to_string(), None));
                            if name == "request_user_input" {
                                self.push_question_replay(args.as_ref(), content);
                            } else {
                                let header = args
                                    .as_ref()
                                    .map(|a| {
                                        crate::tool_fmt::format_tool_call_header(
                                            &name,
                                            a,
                                            Some(cwd),
                                        )
                                    })
                                    .unwrap_or_else(|| ratatui::text::Line::from(name.clone()));
                                let lines = crate::tool_fmt::format_tool_result_lines_with_context(
                                    &name,
                                    args.as_ref(),
                                    content,
                                    *is_error,
                                    Some(cwd),
                                );
                                self.push_tool_box(header, lines);
                            }
                        }
                    }
                }
            }
        }
        for (_id, (name, args)) in tool_calls {
            if name == "request_user_input" {
                self.push_question_replay(Some(&args), "{}");
                continue;
            }
            let header = crate::tool_fmt::format_tool_call_header(&name, &args, Some(cwd));
            self.push_tool_box(header, Vec::new());
        }
        if let Some(last_usage) = entries.iter().rev().find_map(|e| e.usage) {
            self.set_usage(last_usage);
        }
        // pi-style: seam gap provided by viewport box padding, not transcript trailing blank
    }
}
