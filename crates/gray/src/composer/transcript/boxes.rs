use super::*;

impl Tui {
    pub fn push_tool_box(&mut self, header: Line<'static>, body: Vec<Line<'static>>) {
        self.atomic(|t| {
            t.insert_tool_box(header, body);
            t.ensure_gap();
        });
        cap_history_entries(&mut self.history_entries);
        let _ = std::io::stdout().flush();
    }

    pub fn push_tool_box_lines(&mut self, header: Vec<Line<'static>>, body: Vec<Line<'static>>) {
        self.atomic(|t| {
            t.insert_tool_box_lines(header, body);
            t.ensure_gap();
        });
        cap_history_entries(&mut self.history_entries);
        let _ = std::io::stdout().flush();
    }

    fn insert_tool_box_lines(&mut self, header: Vec<Line<'static>>, body: Vec<Line<'static>>) {
        self.flush_markdown();
        self.ensure_gap();
        let w = self.width().max(10);
        let box_lines = format_tool_box_lines_multi(&header, &body, w);
        self.history_entries
            .push(crate::composer::TranscriptEntry::ToolBoxLines { header, body });
        self.emit_rows(box_lines, crate::theme::theme().surface_bg);
    }

    fn insert_tool_box(&mut self, header: Line<'static>, body: Vec<Line<'static>>) {
        self.flush_markdown();
        self.ensure_gap();
        let w = self.width().max(10);
        let box_lines = format_tool_box_lines(header.clone(), &body, w);
        self.history_entries
            .push(crate::composer::TranscriptEntry::ToolBox { header, body });
        self.emit_rows(box_lines, crate::theme::theme().surface_bg);
    }

    pub(crate) fn push_line_styled(&mut self, line: String, style: Style) {
        let l = Line::from(vec![Span::styled(line, style)]);
        self.push_styled_lines_with_hyperlinks(vec![l], &[], 0);
    }

    pub(crate) fn append_thinking_text(&mut self, fragment: &str, terminated: bool) {
        if !matches!(
            self.history_entries.last(),
            Some(crate::composer::TranscriptEntry::ThinkingRun(_))
        ) {
            self.history_entries
                .push(crate::composer::TranscriptEntry::ThinkingRun(String::new()));
            cap_history_entries(&mut self.history_entries);
        }
        if let Some(crate::composer::TranscriptEntry::ThinkingRun(run)) =
            self.history_entries.last_mut()
        {
            run.push_str(fragment);
            if terminated {
                run.push('\n');
            }
        }
    }

    pub(crate) fn thinking_run_mid_line(&self) -> bool {
        matches!(
            self.history_entries.last(),
            Some(crate::composer::TranscriptEntry::ThinkingRun(run))
                if !run.is_empty() && !run.ends_with('\n')
        )
    }

    pub(crate) fn flush_thinking_fragment(&mut self, fragment: &str, terminated: bool) {
        self.drain_reveal();
        let mid_line = self.thinking_run_mid_line();
        self.append_thinking_text(fragment, terminated);
        if let Some(text) = thinking_paint_text(fragment, mid_line) {
            self.paint_thinking_fragment(text.to_string());
        }
    }

    pub(crate) fn paint_thinking_fragment(&mut self, fragment: String) {
        let line = Line::from(vec![Span::styled(fragment, thinking_style())]);
        self.emit_styled(vec![line], Vec::new());
        let _ = std::io::stdout().flush();
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
        let max_w = prose_width(width);
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
        let lines_only: Vec<Line<'static>> = all_wrapped.iter().map(|(l, _)| l.clone()).collect();
        if self.modal_open {
            self.modal_pending
                .push(crate::composer::PendingInsert::Styled(all_wrapped));
            return lines_only;
        }
        self.insert_styled_rows(all_wrapped);
        lines_only
    }

    pub(crate) fn insert_styled_rows(
        &mut self,
        all_wrapped: Vec<(Line<'static>, Vec<HyperlinkTarget>)>,
    ) {
        if all_wrapped.is_empty() {
            return;
        }
        let bg = crate::theme::theme().window();
        let rows: Vec<crate::composer::BatchedInsert> = all_wrapped
            .into_iter()
            .map(|(line, hls)| (line, hls, bg))
            .collect();
        if let Some(batch) = &mut self.insert_batch {
            batch.extend(rows);
            return;
        }
        let total_h = rows.len() as u16;
        let _ = self.atomic(|t| {
            let _ = crate::composer::draw::settle_band(t);
            t.terminal.insert_before(total_h, |buf| {
                paint_insert_rows(buf, &rows);
            })
        });
    }

    pub fn push_styled_lines_with_hyperlinks(
        &mut self,
        lines: Vec<Line<'static>>,
        hyperlinks: &[HyperlinkTarget],
        line_offset: usize,
    ) {
        if lines.is_empty() {
            return;
        }
        let rebased = rebase_hyperlinks_for_slice(hyperlinks, line_offset, lines.len());

        self.drain_reveal();
        self.history_entries
            .push(crate::composer::TranscriptEntry::StyledLines {
                lines: lines.clone(),
                hyperlinks: rebased.clone(),
            });
        self.emit_styled(lines, rebased);
        cap_history_entries(&mut self.history_entries);
        let _ = std::io::stdout().flush();
    }

    pub(crate) fn push_mascot(&mut self) -> bool {
        let w = self.width().max(10);
        let cols = u16::try_from(w).unwrap_or(u16::MAX);
        let rows = self.last_height.max(1);
        let Some(lines) = crate::mascot::mascot_lines(cols, rows, Some(w)) else {
            return false;
        };
        crate::mascot::set_mascot_shown(true);
        self.flush_markdown();
        self.ensure_gap();
        self.history_entries
            .push(crate::composer::TranscriptEntry::Mascot);
        cap_history_entries(&mut self.history_entries);
        self.emit_styled(lines, Vec::new());
        self.ensure_gap();
        let _ = std::io::stdout().flush();
        true
    }

    pub fn push_dim(&mut self, line: String) {
        // On home a one-liner (quit hint, command reply) is a toast over
        // the box, not a column-0 transcript row.
        if !line.contains('\n')
            && self.home_toast(Line::styled(
                line.trim().to_string(),
                Style::default().fg(crate::theme::theme().tool_dim),
            ))
        {
            return;
        }
        let lines: Vec<Line<'static>> = line
            .split('\n')
            .map(|l| {
                Line::from(vec![Span::styled(
                    l.to_string(),
                    Style::new().add_modifier(Modifier::DIM),
                )])
            })
            .collect();
        self.push_styled_lines_with_hyperlinks(lines, &[], 0);
    }

    pub fn push_error(&mut self, msg: &str) {
        self.push_styled_lines_with_hyperlinks(error_lines(msg), &[], 0);
    }

    pub fn push_compaction_summary(&mut self, summary: &str) {
        let (lines, hyperlinks) = render_markdown_lines(summary, Some(self.width()));
        if lines.is_empty() {
            return;
        }
        self.push_styled_lines_with_hyperlinks(lines, &hyperlinks, 0);
    }

    pub fn push_action(&mut self, text: &str, detail: Option<&str>) {
        let mut spans = vec![
            Span::styled(
                "✓ ",
                Style::default()
                    .fg(crate::theme::theme().success)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                text.to_string(),
                Style::default()
                    .fg(crate::theme::theme().text_bright)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(d) = detail {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                d.to_string(),
                Style::default().fg(crate::theme::theme().text_muted),
            ));
        }
        let line = Line::from(spans);
        if self.home_toast(line.clone()) {
            return;
        }
        let line = self.indent_home_receipt(line);
        self.push_styled_lines_with_hyperlinks(vec![line], &[], 0);
    }

    fn indent_home_receipt(&self, mut line: Line<'static>) -> Line<'static> {
        let indent = home_receipt_indent(self.is_home(), self.width());
        if indent > 0 {
            line.spans.insert(0, Span::raw(" ".repeat(indent)));
        }
        line
    }

    pub fn push_warning(&mut self, text: &str) {
        let line = Line::from(vec![
            Span::styled(
                "⚠ ",
                Style::default()
                    .fg(crate::theme::theme().error_soft)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                text.to_string(),
                Style::default().fg(crate::theme::theme().text_soft),
            ),
        ]);
        if self.home_toast(line.clone()) {
            return;
        }
        let line = self.indent_home_receipt(line);
        self.push_styled_lines_with_hyperlinks(vec![line], &[], 0);
    }

    pub fn replay_session_history_with_tools(
        &mut self,
        history: std::sync::Arc<Vec<gray_core::Message>>,
        cwd: &std::path::Path,
        display: crate::tool_fmt::ToolDisplay<'_>,
        tail: Vec<String>,
    ) {
        self.left_home = true;
        let estimate: usize = history.iter().map(crate::compact::estimate_tokens).sum();
        self.replay_queue = Some(Box::new(crate::composer::ReplayQueue {
            history,
            next: 0,
            tail,
            tool_calls: HashMap::new(),
            labels: display.labels.cloned(),
            previews: display.previews.cloned(),
            cwd: cwd.to_path_buf(),
            estimate,
        }));
        self.tick_replay();
    }

    pub(crate) fn tick_replay(&mut self) {
        let Some(mut q) = self.replay_queue.take() else {
            return;
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(4);
        self.begin_insert_batch();
        while q.next < q.history.len() {
            let display = crate::tool_fmt::ToolDisplay {
                labels: q.labels.as_ref(),
                previews: q.previews.as_ref(),
            };
            self.replay_entry(&q.history[q.next], &mut q.tool_calls, &q.cwd, display);
            q.next += 1;
            if std::time::Instant::now() >= deadline {
                break;
            }
        }
        let done = q.next >= q.history.len();
        if done {
            let display = crate::tool_fmt::ToolDisplay {
                labels: q.labels.as_ref(),
                previews: q.previews.as_ref(),
            };
            for (_id, (name, args)) in q.tool_calls.drain() {
                let header = crate::tool_fmt::format_tool_card_lines(
                    &name,
                    &args,
                    Some(&q.cwd),
                    display.label(&name),
                    display.preview(&name),
                );
                self.push_tool_box_lines(header, Vec::new());
            }
            self.seed_estimate_usage(q.estimate);
            let tail = std::mem::take(&mut q.tail);
            let trailer = !tail.is_empty();
            if trailer {
                self.ensure_gap();
            }
            for line in tail {
                self.push_dim(line);
            }
            if trailer {
                self.ensure_gap();
            }
        }
        self.end_insert_batch();
        if !done {
            self.replay_queue = Some(q);
        }
        let _ = self.draw();
    }

    fn replay_entry(
        &mut self,
        msg: &gray_core::Message,
        tool_calls: &mut HashMap<String, (String, serde_json::Value)>,
        cwd: &std::path::Path,
        display: crate::tool_fmt::ToolDisplay<'_>,
    ) {
        {
            match msg.role {
                gray_core::Role::User => {
                    let mut user_text = String::new();
                    let mut attached: Vec<std::path::PathBuf> = Vec::new();
                    let mut media_stubs: Vec<String> = Vec::new();
                    let has_tool_result = msg
                        .content
                        .iter()
                        .any(|b| matches!(b, gray_core::ContentBlock::ToolResult { .. }));
                    let mut tool_media_stubs: Vec<std::path::PathBuf> = msg
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            gray_core::ContentBlock::Text { text } => {
                                media_stub_saved_path(text).filter(|p| p.is_file())
                            }
                            _ => None,
                        })
                        .collect();
                    for block in &msg.content {
                        match block {
                            gray_core::ContentBlock::Text { text } => {
                                if is_media_stub_text(text) {
                                    if has_tool_result {
                                        continue;
                                    }
                                    match media_stub_saved_path(text).filter(|p| p.is_file()) {
                                        Some(saved) => attached.push(saved),
                                        None => media_stubs.push(text.trim().to_string()),
                                    }
                                    continue;
                                }
                                if !user_text.is_empty() {
                                    user_text.push('\n');
                                }
                                user_text.push_str(text);
                            }
                            gray_core::ContentBlock::StructuredInput { kind, .. } => {
                                if !user_text.is_empty() {
                                    user_text.push('\n');
                                }
                                user_text.push_str("[component event: ");
                                user_text.push_str(kind);
                                user_text.push(']');
                            }
                            gray_core::ContentBlock::Image { media_type, data }
                            | gray_core::ContentBlock::Media {
                                media_type, data, ..
                            } => {
                                if let Some(p) =
                                    gray_core::paths::materialize_media(media_type, data)
                                {
                                    attached.push(p);
                                }
                            }
                            gray_core::ContentBlock::ToolResult {
                                id,
                                content,
                                is_error,
                            } => replay_tool_result(
                                self,
                                tool_calls,
                                ReplayResult {
                                    id,
                                    content,
                                    is_error: *is_error,
                                },
                                cwd,
                                display,
                                &mut tool_media_stubs,
                            ),
                            _ => {}
                        }
                    }
                    let typed = !msg.injected
                        && msg
                            .content
                            .iter()
                            .any(|b| matches!(b, gray_core::ContentBlock::Text { .. }));
                    if !user_text.is_empty() || !attached.is_empty() || !media_stubs.is_empty() {
                        let (visible, notices) = super::split_synthetic_prompt(&user_text);
                        if !visible.is_empty() || !attached.is_empty() {
                            self.push_user_prompt(&visible, &attached, true);
                        }
                        for notice in notices {
                            self.push_dim(format!("\u{2b22} {notice}"));
                        }
                        for stub in &media_stubs {
                            self.push_dim(format!("\u{2b22} {stub}"));
                        }
                        if typed && !visible.is_empty() {
                            self.history.push(visible);
                            if self.history.len() > 100 {
                                self.history.remove(0);
                            }
                            self.history_idx = None;
                        }
                    }
                }
                gray_core::Role::Assistant => {
                    for block in &msg.content {
                        match block {
                            gray_core::ContentBlock::Thinking { text, .. } => {
                                if !text.trim().is_empty() {
                                    self.ensure_gap();
                                    let rows =
                                        thinking_run_rows(text, prose_width(self.width()), false);
                                    self.push_styled_lines_with_hyperlinks(rows, &[], 0);
                                }
                            }
                            gray_core::ContentBlock::Text { text } => {
                                let clean = strip_ansi(text);
                                // "(empty)" is agent-loop scaffolding for a
                                // textless turn, not real output — don't
                                // resurrect it on replay.
                                if clean.trim() == "(empty)" {
                                    continue;
                                }
                                if !clean.trim().is_empty() {
                                    self.ensure_gap();

                                    let tw = prose_width(self.width().max(10));
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
                            gray_core::ContentBlock::ToolUse { id, name, args, .. } => {
                                tool_calls.insert(id.clone(), (name.clone(), args.clone()));
                            }
                            _ => {}
                        }
                    }
                }
                gray_core::Role::System => {
                    for block in &msg.content {
                        if let gray_core::ContentBlock::ToolResult {
                            id,
                            content,
                            is_error,
                        } = block
                        {
                            replay_tool_result(
                                self,
                                tool_calls,
                                ReplayResult {
                                    id,
                                    content,
                                    is_error: *is_error,
                                },
                                cwd,
                                display,
                                &mut Vec::new(),
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Compaction swaps old image blocks for a stub text block before the
/// history is persisted; on replay, recover the saved copy so the preview
/// still renders instead of dumping the stub into the prompt.
fn media_stub_saved_path(text: &str) -> Option<std::path::PathBuf> {
    let t = text.trim();
    if !is_media_stub_text(t) {
        return None;
    }
    let rest = t.split("Saved to ").nth(1)?;
    let path = rest.split("; read it to re-view]").next()?.trim();
    (!path.is_empty()).then(|| std::path::PathBuf::from(path))
}

fn is_media_stub_text(text: &str) -> bool {
    let t = text.trim();
    t.starts_with('[') && t.ends_with(']') && t.contains(" elided from context (")
}

struct ReplayResult<'a> {
    id: &'a str,
    content: &'a str,
    is_error: bool,
}

fn replay_tool_result(
    tui: &mut Tui,
    tool_calls: &mut HashMap<String, (String, serde_json::Value)>,
    result: ReplayResult<'_>,
    cwd: &std::path::Path,
    display: crate::tool_fmt::ToolDisplay<'_>,
    stub_images: &mut Vec<std::path::PathBuf>,
) {
    let ReplayResult {
        id,
        content,
        is_error,
    } = result;
    let (name, args) = tool_calls
        .remove(id)
        .map(|(n, a)| (n, Some(a)))
        .unwrap_or_else(|| ("tool".to_string(), None));
    let header = match args.as_ref() {
        Some(a) => crate::tool_fmt::format_tool_card_lines(
            &name,
            a,
            Some(cwd),
            display.label(&name),
            display.preview(&name),
        ),
        None => crate::tool_fmt::format_live_tool_header(
            &name,
            "",
            Some(cwd),
            display.label(&name),
            display.preview(&name),
        ),
    };
    let mut lines = crate::tool_fmt::format_tool_result_lines_with_context(
        &name,
        args.as_ref(),
        content,
        is_error,
        Some(cwd),
    );
    if !is_error {
        let rows = crate::composer::transcript::rows::viewed_image_lines(
            &name,
            args.as_ref(),
            cwd,
            tui.width(),
        )
        .or_else(|| {
            let rows =
                crate::composer::transcript::rows::attached_image_lines(stub_images, tui.width());
            if rows.is_some() {
                stub_images.clear();
            }
            rows
        });
        if let Some(rows) = rows {
            lines.extend(rows);
        }
    }
    tui.push_tool_box_lines(header, lines);
}

pub(crate) fn rebase_hyperlinks_for_slice(
    hyperlinks: &[HyperlinkTarget],
    line_offset: usize,
    len: usize,
) -> Vec<HyperlinkTarget> {
    hyperlinks
        .iter()
        .filter_map(|h| {
            if h.line_index < line_offset || h.line_index >= line_offset.saturating_add(len) {
                return None;
            }
            let mut hc = h.clone();
            hc.line_index -= line_offset;
            Some(hc)
        })
        .collect()
}

pub(crate) fn home_receipt_indent(home: bool, width: usize) -> usize {
    if !home {
        return 0;
    }
    crate::composer::home_column(width).0.saturating_sub(GUTTER)
}

pub(crate) fn error_lines(msg: &str) -> Vec<Line<'static>> {
    let theme = crate::theme::theme();
    msg.trim_end()
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            let style = if i == 0 {
                Style::default().fg(theme.error)
            } else {
                Style::default().fg(theme.text_muted)
            };
            Line::from(Span::styled(l.to_string(), style))
        })
        .collect()
}

pub(crate) fn render_markdown_lines(
    text: &str,
    width: Option<usize>,
) -> (Vec<Line<'static>>, Vec<HyperlinkTarget>) {
    let clean = strip_ansi(text);
    if clean.trim().is_empty() {
        return (Vec::new(), Vec::new());
    }
    let tw = prose_width(width.unwrap_or(80).max(10));
    let mut buffers = gray_markdown::MarkdownBuffers::new();
    let (output, _) = gray_markdown::render_markdown_ratatui_with_buffers_width(
        &clean,
        gray_markdown::gray_markdown_style(),
        true,
        &mut buffers,
        Some(gray_markdown::get_syntect()),
        Some(tw),
    );
    (output.lines, output.hyperlinks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_stub_saved_path_extracts_path() {
        let text = "[image/png elided from context (711160 bytes). Saved to /home/u/.gray/attachments/ab12.png; read it to re-view]";
        assert_eq!(
            media_stub_saved_path(text),
            Some(std::path::PathBuf::from(
                "/home/u/.gray/attachments/ab12.png"
            ))
        );
    }

    #[test]
    fn media_stub_without_saved_path_is_still_a_stub() {
        let text = "[image/png elided from context (215264 bytes). The original survives in the session transcript and source file]";
        assert!(is_media_stub_text(text));
        assert_eq!(media_stub_saved_path(text), None);
    }

    #[test]
    fn plain_prompt_text_is_not_a_stub() {
        assert!(!is_media_stub_text("fix this please"));
        assert!(!is_media_stub_text("[not a stub] elided from context-ish"));
        assert_eq!(media_stub_saved_path("hello"), None);
    }
}
