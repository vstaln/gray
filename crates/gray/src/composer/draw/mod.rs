use std::time::Duration;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{MIN_VIEWPORT_H, PANEL_ROWS, Tui, VIEWPORT_H};
use crate::text_width::display_width;

mod widgets;

pub(crate) use widgets::{
    build_input_box, queued_preview_lines, shimmer_spans, status_dock_h, transcript_ends_blank,
};

/// Exact-fit viewport height for the given content, clamped to
/// `MIN_VIEWPORT_H..=max_h`.
pub(crate) fn desired_viewport_h(
    status_h: u16,
    queued_h: u16,
    box_rows: u16,
    panel_h: u16,
    attach_h: u16,
    max_h: u16,
) -> u16 {
    (status_h + queued_h + box_rows + panel_h + attach_h + 1).clamp(MIN_VIEWPORT_H, max_h)
}

pub(crate) fn draw(tui: &mut Tui) -> anyhow::Result<()> {
    if tui.modal_open {
        return Ok(());
    }
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let screen_size = ratatui::layout::Size::new(cols, rows);
    let w = cols as usize;

    let text = tui.textarea.text().to_string();
    let cursor = tui.textarea.cursor().min(text.len());
    let ibox = build_input_box(&text, cursor, w);
    let box_h = ibox.lines.len().max(1) as u16;
    // Attachments row.
    let attach_h: u16 = u16::from(!tui.attachments.is_empty());
    // Seam (only if scrollback didn't already end blank) + shimmer status
    // text + one bare breathing row below it.
    let needs_seam = !transcript_ends_blank(&tui.transcript);
    let status_h: u16 = status_dock_h(tui.status.is_some(), needs_seam);
    // Row offset of the status text inside its dock: below the seam when
    // one was reserved, else the very top of the viewport.
    let seam_h: u16 = if status_h > 0 {
        u16::from(needs_seam)
    } else {
        0
    };

    // Exact-fit viewport with in-place resizing (codex parity):
    // Grow and shrink are applied directly to the terminal's viewport area without
    // recreating the terminal or re-probing cursor position via CPR, preventing
    // prompt doubling and cursor drift.
    let n = tui.queued_inputs.len();
    let queued_est: u16 = if n == 0 {
        0
    } else {
        1 + n.min(3) as u16 + u16::from(n > 3)
    };
    let panel_est: u16 = tui.matches.len().min(PANEL_ROWS) as u16;
    let box_rows_est: u16 = box_h;
    let max_viewport_h = VIEWPORT_H;
    let desired = desired_viewport_h(
        status_h,
        queued_est,
        box_rows_est,
        panel_est,
        attach_h,
        max_viewport_h,
    );

    // frankentui lesson: synchronized-output bracketing (DEC2026) — one atomic
    // present per frame so the compositor never shows a torn frame.
    // Terminals without support ignore the sequence.
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::BeginSynchronizedUpdate
    )?;
    // Viewport geometry is a precondition for the frame: a swallowed failure
    // would commit a frame against stale geometry (audit 24.01). Close the
    // synchronized-update bracket before bailing out.
    if let Err(e) = tui.terminal.set_viewport_height(desired, screen_size) {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EndSynchronizedUpdate
        );
        return Err(e.into());
    }
    tui.viewport_h = desired;

    let res = tui.terminal.draw(|frame| {
        let area = frame.area();
        let w = area.width as usize;

        let status_y = area.y;
        // Queued preview sits between status and input.
        let queued_preview: Vec<Line<'static>> = queued_preview_lines(&tui.queued_inputs, w);
        let queued_h = queued_preview.len() as u16;
        // Space left for the completion panel once the fixed rows
        // (status, queued, input, attachments, footer) are placed.
        let avail = area
            .height
            .saturating_sub(status_h + queued_h + box_h + attach_h + 1);
        let need = PANEL_ROWS as u16;
        let panel_cap = need.min(avail).max((PANEL_ROWS as u16).min(avail));
        let visible_count = if tui.matches.is_empty() {
            0
        } else {
            tui.matches.len().min(panel_cap as usize)
        };
        let panel_h = visible_count as u16;
        let box_rows = box_h;
        let box_y = status_y + status_h + queued_h;
        let panel_y = box_y + box_rows;
        let attach_y = panel_y + panel_h;
        let footer_y = attach_y + attach_h;

        if let Some((started, label)) = &tui.status {
            let label_text = format!(" ⬡ {label}\u{2026}");
            let mut spans = shimmer_spans(&label_text, started.elapsed());
            let elapsed = started.elapsed();
            let elapsed_str = format!("{:.1}s", elapsed.as_secs_f64());
            let tok_suffix = if tui.is_task_running {
                // Per-turn counter: starts at 0 on `begin_turn`, grows with
                // streamed output, and `end_turn` prints the same value as
                // the final `Thought for` total. Session context lives in
                // the footer gauge, never here.
                let live = tui.live_streamed_tokens;
                format!(" · {} tok", crate::repl::fmt_usage(live))
            } else if let Some(u) = tui.latest_usage {
                format!(" · {} tok", crate::repl::fmt_usage(u.total()))
            } else {
                String::new()
            };
            let suffix = format!(" {elapsed_str}{tok_suffix} (esc to interrupt)");
            spans.push(Span::styled(
                suffix,
                Style::default().fg(crate::theme::theme().tool_dim),
            ));
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(area.x, status_y + seam_h, area.width, 1),
            );
        }

        for (i, line) in queued_preview.iter().enumerate() {
            let y = status_y + status_h + i as u16;
            if y < area.y || y >= area.y + area.height {
                continue;
            }
            frame.render_widget(
                Paragraph::new(line.clone()),
                Rect::new(area.x, y, area.width, 1),
            );
        }
        let rendered_box_h = box_h.min(area.bottom().saturating_sub(box_y));
        if rendered_box_h > 0 {
            let box_block =
                Block::default().style(Style::default().bg(crate::theme::theme().surface_bg));
            frame.render_widget(
                Paragraph::new(ibox.lines.clone()).block(box_block),
                Rect::new(area.x, box_y, area.width, rendered_box_h),
            );
        }

        if visible_count > 0 {
            let start = tui
                .sel
                .saturating_sub(visible_count.saturating_sub(1))
                .min(tui.sel);
            // Scroll indicator: the window clips when the list is longer
            // than the cap — arrows at the clipped edge(s).
            let total = tui.matches.len();
            let end = (start + visible_count).min(total);
            let hidden_above = start;
            let hidden_below = total.saturating_sub(end);
            for (i, (name, desc)) in tui
                .matches
                .iter()
                .enumerate()
                .skip(start)
                .take(visible_count)
            {
                let y = (i - start) as u16;
                let item_y = panel_y + y;
                if item_y < area.y || item_y >= area.y + area.height {
                    continue;
                }
                let is_sel = i == tui.sel;
                let cmd_str = format!(" /{name} ");
                let desc_str = format!(" {desc} ");
                let used_len = display_width(&cmd_str) + display_width(&desc_str);
                // ponytail: single-char edge arrows, no extra row or layout.
                let marker = match (
                    i == start && hidden_above > 0,
                    i + 1 == end && hidden_below > 0,
                ) {
                    (true, true) => "↕",
                    (true, false) => "↑",
                    (false, true) => "↓",
                    (false, false) => "",
                };
                let pad_len = w.saturating_sub(used_len + display_width(marker));
                let line_bg = if is_sel {
                    crate::theme::theme().accent
                } else {
                    crate::theme::theme().raised_bg
                };
                let marker_style = if is_sel {
                    Style::default()
                        .fg(crate::theme::theme().on_selection)
                        .bg(line_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::White)
                        .bg(line_bg)
                        .add_modifier(Modifier::BOLD)
                };
                let line = if is_sel {
                    Line::from(vec![
                        Span::styled(
                            cmd_str,
                            Style::default()
                                .fg(crate::theme::theme().on_selection)
                                .bg(line_bg)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            desc_str,
                            Style::default()
                                .fg(crate::theme::theme().text_faint)
                                .bg(line_bg),
                        ),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(line_bg)),
                        Span::styled(marker.to_string(), marker_style),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(
                            cmd_str,
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(line_bg),
                        ),
                        Span::styled(
                            desc_str,
                            Style::default()
                                .fg(crate::theme::theme().text_muted)
                                .bg(line_bg),
                        ),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(line_bg)),
                        Span::styled(marker.to_string(), marker_style),
                    ])
                };
                frame.render_widget(
                    Paragraph::new(line)
                        .block(Block::default().style(Style::default().bg(line_bg))),
                    Rect::new(area.x, item_y, area.width, 1),
                );
            }
        }

        if attach_h > 0 && attach_y < area.y + area.height {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (_ph, p) in &tui.attachments {
                let fname = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("clipboard")
                    .to_string();
                // blue File badge like opencode
                spans.push(Span::styled(
                    " File ",
                    Style::default()
                        .fg(Color::White)
                        .bg(crate::theme::theme().info)
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    fname,
                    Style::default()
                        .fg(crate::theme::theme().text_soft)
                        .bg(crate::theme::theme().input_bg),
                ));
                spans.push(Span::raw("  "));
            }
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(area.x, attach_y, area.width, 1),
            );
        }

        let (_, max_label) = crate::setup::model_context_info(&tui.model_name);
        let (used_tokens, hit_rate) = if let Some(u) = tui.latest_usage.or(tui.cumulative_usage) {
            (u.total(), u.cache_hit_rate() * 100.0)
        } else {
            (0, 0.0)
        };
        let ctx_display = format!(
            "{}/{}",
            crate::setup::format_context_length(used_tokens),
            max_label
        );
        let cache_display = format!("{hit_rate:.1}% cache");

        let model_display = crate::setup::friendly_model_name(&tui.model_name);
        // Provider-driven (opencode parity): no effort badge when the provider
        // says this model doesn't reason. Unknown → show, as before.
        let show_effort =
            crate::setup::context::model_supports_reasoning(&tui.model_name) != Some(false);
        // `off` already implies hidden — don't render "off · hidden".
        // Non-reasoning models (show_effort false) render no badge; the
        // separator is omitted with it so the footer never trails " · ".
        let effort_display = if !show_effort {
            String::new()
        } else if tui.thinking_effort == "off" {
            "off".to_string()
        } else if tui.hide_thinking {
            if tui.thinking_effort.is_empty() {
                "hidden".to_string()
            } else {
                format!("{} · hidden", tui.thinking_effort)
            }
        } else {
            tui.thinking_effort.clone()
        };
        let right_parts = if model_display.is_empty() {
            if effort_display.is_empty() {
                Vec::new()
            } else {
                vec![Span::styled(
                    effort_display.clone(),
                    Style::default().fg(crate::theme::theme().tool_dim),
                )]
            }
        } else if effort_display.is_empty() {
            vec![Span::styled(
                model_display.clone(),
                Style::default().fg(crate::theme::theme().text_muted),
            )]
        } else {
            vec![
                Span::styled(
                    model_display.clone(),
                    Style::default().fg(crate::theme::theme().text_muted),
                ),
                Span::styled(
                    " \u{b7} ",
                    Style::default().fg(crate::theme::theme().text_faint),
                ),
                Span::styled(
                    effort_display.clone(),
                    Style::default().fg(crate::theme::theme().tool_dim),
                ),
            ]
        };
        let right_len = if model_display.is_empty() {
            display_width(&effort_display)
        } else if effort_display.is_empty() {
            display_width(&model_display)
        } else {
            display_width(&model_display) + 3 + display_width(&effort_display)
        };
        let left_len = 1 + display_width(&ctx_display) + 3 + display_width(&cache_display);
        let pad_len = w.saturating_sub(left_len + right_len);

        let cache_color = if hit_rate > 0.0 {
            crate::theme::theme().cache_hit
        } else {
            crate::theme::theme().text_faint
        };

        let mut footer_spans = vec![
            Span::raw(" "),
            Span::styled(
                ctx_display,
                Style::default().fg(crate::theme::theme().tool_dim),
            ),
            Span::styled(
                " \u{b7} ",
                Style::default().fg(crate::theme::theme().text_faint),
            ),
            Span::styled(cache_display, Style::default().fg(cache_color)),
        ];
        footer_spans.push(Span::raw(" ".repeat(pad_len)));
        footer_spans.extend(right_parts);
        if footer_y < area.y + area.height {
            // Transparent footer: text only, no full-bleed band.
            frame.render_widget(
                Paragraph::new(Line::from(footer_spans)),
                Rect::new(area.x, footer_y, area.width, 1),
            );
        }

        let used_bottom = footer_y + 1;
        if used_bottom < area.y + area.height {
            frame.render_widget(
                ratatui::widgets::Clear,
                Rect::new(
                    area.x,
                    used_bottom,
                    area.width,
                    (area.y + area.height) - used_bottom,
                ),
            );
        }

        if tui.status.is_none() && !tui.is_task_running {
            let cur_x =
                (area.x + 3 + ibox.cur_col as u16).min(area.x + area.width.saturating_sub(1));
            let cur_y =
                (box_y + 1 + ibox.cur_row as u16).min(area.y + area.height.saturating_sub(1));
            frame.set_cursor_position(Position::new(cur_x, cur_y));
        }
    });
    let ended = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EndSynchronizedUpdate
    );
    res?;
    ended?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_dock_seam_is_dynamic() {
        assert_eq!(status_dock_h(false, true), 0);
        assert_eq!(status_dock_h(true, false), 2); // scrollback already blank: status + breath
        assert_eq!(status_dock_h(true, true), 3); // seam + status + breath
    }

    #[test]
    fn transcript_ends_blank_matches_ensure_gap() {
        use ratatui::style::Style;
        assert!(!transcript_ends_blank(&[]));
        assert!(transcript_ends_blank(&[Line::from("")]));
        assert!(transcript_ends_blank(&[Line::from(" ")])); // left_pad-only row
        assert!(!transcript_ends_blank(&[Line::from("text")]));
        // card / code padding rows carry a bg: they are edges, not gaps
        let bg = Style::default().bg(crate::theme::GRAY_UI_THEME.surface_bg);
        assert!(!transcript_ends_blank(&[Line::from("").style(bg)]));
    }

    #[test]
    fn desired_viewport_exact_fit() {
        // Idle: input 3 + footer 1 = 4 rows (MIN_VIEWPORT_H).
        assert_eq!(desired_viewport_h(0, 0, 3, 0, 0, VIEWPORT_H), 4);
        // Slash popup: input 3 + panel 6 + footer 1 = 10.
        assert_eq!(desired_viewport_h(0, 0, 3, 6, 0, VIEWPORT_H), 10);
        // Running: status 2 + input 3 + footer 1 = 6.
        assert_eq!(desired_viewport_h(2, 0, 3, 0, 0, VIEWPORT_H), 6);
        // Running + full panel: 3 + 3 + 6 + 1 = 13.
        assert_eq!(desired_viewport_h(3, 0, 3, 6, 0, VIEWPORT_H), 13);
        // Question panel: expands up to available screen height to show all options.
        assert_eq!(desired_viewport_h(0, 0, 0, 15, 0, 23), 16);
    }

    #[test]
    fn queued_preview_renders_header_and_entries() {
        let mut q: std::collections::VecDeque<(String, Vec<std::path::PathBuf>)> =
            std::collections::VecDeque::new();
        assert!(queued_preview_lines(&q, 80).is_empty());
        q.push_back(("hello".to_string(), vec![]));
        q.push_back(("second\nline".to_string(), vec![]));
        let lines = queued_preview_lines(&q, 80);
        assert_eq!(lines.len(), 3); // header + 2 entries
        let text: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Queued follow-up inputs (2)"), "got: {text}");
        assert!(text.contains("↳ hello"), "got: {text}");
        assert!(text.contains("↳ second"), "got: {text}");
    }
}
