use std::time::Duration;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{PANEL_ROWS, Tui};

mod widgets;

pub(crate) use widgets::{
    build_input_box, queued_preview_lines, shimmer_spans, status_dock_h, thinking_style,
    transcript_ends_blank,
};

pub(crate) fn draw(tui: &mut Tui) -> anyhow::Result<()> {
    if tui.modal_open {
        return Ok(());
    }
    let (cols, _rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let w = cols as usize;

    // Snapshot the live boot panel before the draw closure borrows the terminal.
    // Same card content as the committed final card (bg + margins), so the
    // `starting` view never looks different from `autostarted`.
    let boot_panel_lines: Vec<Line<'static>> = if tui.active_question.is_some() {
        Vec::new()
    } else {
        tui.gateway_panel_lines(w)
    };

    let text = tui.textarea.text().to_string();
    let cursor = tui.textarea.cursor().min(text.len());
    let ibox = build_input_box(&text, cursor, w);
    let box_h = ibox.lines.len().max(1) as u16;
    let question_active = tui.active_question.is_some();
    // While a question is up it IS the status: hide the shimmer and the
    // attachments row so the panel gets the whole inline viewport.
    let attach_h: u16 = u16::from(!tui.attachments.is_empty() && !question_active);
    // Seam (only if scrollback didn't already end blank) + shimmer status
    // text + one bare breathing row below it.
    let needs_seam = !transcript_ends_blank(&tui.transcript);
    let status_h: u16 = status_dock_h(tui.status.is_some(), question_active, needs_seam);
    // Row offset of the status text inside its dock: below the seam when
    // one was reserved, else the very top of the viewport.
    let seam_h: u16 = if status_h > 0 {
        u16::from(needs_seam)
    } else {
        0
    };

    // frankentui lesson: synchronized-output bracketing (DEC2026) — one atomic
    // present per frame so the compositor never shows a torn frame.
    // Terminals without support ignore the sequence.
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::BeginSynchronizedUpdate
    )?;
    let res = tui.terminal.draw(|frame| {
        let area = frame.area();
        let w = area.width as usize;

        let status_y = area.y;
        // Queued preview sits between status and input (codex PendingInputPreview
        // parity). Hidden while a question owns the viewport.
        // Live gateway boot panel rides the same slot (above the input box):
        // card rows with the committed card's bg, zero transcript lines.
        let queued_preview: Vec<Line<'static>> = if question_active {
            Vec::new()
        } else {
            queued_preview_lines(&tui.queued_inputs, w)
        };
        // Live gateway boot panel rides the slot above the input box. Its rows
        // come pre-padded with the card bg (transcript::format_gateway_boot_card)
        // and ONE bare row separates the card from the input band so the two
        // gray blocks never fuse — the same `ensure_gap(1)` the committed card
        // gets, so the commit never shifts the input by a row.
        let boot_lines: &[Line<'static>] = if question_active {
            &[]
        } else {
            &boot_panel_lines
        };
        let boot_gap_h: u16 = u16::from(!boot_lines.is_empty());
        let queued_h = (queued_preview.len() + boot_lines.len()) as u16 + boot_gap_h;
        let box_y = status_y + status_h + queued_h;
        // Bare breathing row between the band (or panel/attachments below it)
        // and the footer, so the input never melts into it. The box's own pads
        // share the band bg and can't do this job. Question panels carry their
        // own bottom margin.
        let gap_h: u16 = if question_active { 0 } else { 1 };
        let avail = area.height.saturating_sub(
            status_h + queued_h + if question_active { 0 } else { box_h } + attach_h + gap_h + 1,
        );
        // Grow viewport to fit full question; fall back to PANEL_ROWS min when short on space.
        // Two-pass (uncapped then capped), no new layout engine.
        let need = if question_active {
            tui.active_question
                .as_ref()
                .map(|q| super::question::panel_lines(q, w, 100).len() as u16)
                .unwrap_or(PANEL_ROWS as u16)
        } else {
            PANEL_ROWS as u16
        };
        let panel_cap = need.min(avail).max((PANEL_ROWS as u16).min(avail));
        let question_lines: Option<Vec<Line<'static>>> = if question_active {
            tui.active_question
                .as_ref()
                .map(|q| super::question::panel_lines(q, w, panel_cap.max(1) as usize))
        } else {
            None
        };
        let visible_count = if let Some(qlines) = &question_lines {
            qlines.len().min(panel_cap as usize)
        } else if tui.matches.is_empty() {
            0
        } else {
            tui.matches.len().min(panel_cap as usize)
        };
        let panel_h = visible_count as u16;
        // Codex parity: while a question is active the question surface REPLACES
        // the composer — no input box, the panel occupies its slot.
        let panel_y = if question_active {
            box_y
        } else {
            box_y + box_h
        };
        let attach_y = panel_y + panel_h;
        let footer_y = attach_y + attach_h + gap_h;

        if let Some((started, label)) = &tui.status
            && !question_active
        {
            let label_text = format!(" ⬡ {label}\u{2026}");
            let mut spans = shimmer_spans(&label_text, started.elapsed(), tui.truecolor);
            let elapsed = started.elapsed();
            let elapsed_str = format!("{:.1}s", elapsed.as_secs_f64());
            let tok_suffix = if tui.is_task_running {
                let base = tui
                    .latest_usage
                    .or(tui.cumulative_usage)
                    .map(|u| u.total())
                    .unwrap_or(0);
                let live = base + tui.live_streamed_tokens;
                if live > 0 {
                    format!(" · {} tok", crate::repl::fmt_usage(live))
                } else {
                    String::new()
                }
            } else if let Some(u) = tui.latest_usage {
                format!(" · {} tok", crate::repl::fmt_usage(u.total()))
            } else {
                String::new()
            };
            let suffix = format!(" {elapsed_str}{tok_suffix} (esc to interrupt)");
            spans.push(Span::styled(
                suffix,
                Style::default().fg(Color::Rgb(108, 108, 108)),
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
        // Boot panel painted by the SAME helper the committed card uses
        // (paint_card floods the rect with the card bg, then lays the rows), so
        // live `validating token…` is pixel-identical to committed
        // `connected as …`. The bare gap row below it is simply left unpainted.
        if !boot_lines.is_empty() {
            let boot_y = status_y + status_h + queued_preview.len() as u16;
            if boot_y < area.bottom() {
                let boot_h = (boot_lines.len() as u16).min(area.bottom() - boot_y);
                if boot_h > 0 {
                    super::transcript::paint_card(
                        boot_lines,
                        Rect::new(area.x, boot_y, area.width, boot_h),
                        frame.buffer_mut(),
                    );
                }
            }
        }

        let rendered_box_h = box_h.min(area.bottom().saturating_sub(box_y));
        if rendered_box_h > 0 && !question_active {
            let box_block = Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)));
            frame.render_widget(
                Paragraph::new(ibox.lines).block(box_block),
                Rect::new(area.x, box_y, area.width, rendered_box_h),
            );
        }

        if let Some(qlines) = &question_lines
            && visible_count > 0
        {
            for (i, line) in qlines.iter().enumerate().take(visible_count) {
                let item_y = panel_y + i as u16;
                if item_y < area.y || item_y >= area.y + area.height {
                    continue;
                }
                frame.render_widget(
                    Paragraph::new(line.clone())
                        .block(Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)))),
                    Rect::new(area.x, item_y, area.width, 1),
                );
            }
        } else if visible_count > 0 {
            let start = tui
                .sel
                .saturating_sub(visible_count.saturating_sub(1))
                .min(tui.sel);
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
                let used_len = cmd_str.chars().count() + desc_str.chars().count();
                let pad_len = w.saturating_sub(used_len);
                let line_bg = if is_sel {
                    Color::Rgb(246, 173, 126)
                } else {
                    Color::Rgb(28, 28, 28)
                };
                let line = if is_sel {
                    Line::from(vec![
                        Span::styled(
                            cmd_str,
                            Style::default()
                                .fg(Color::Black)
                                .bg(line_bg)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            desc_str,
                            Style::default().fg(Color::Rgb(40, 40, 40)).bg(line_bg),
                        ),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(line_bg)),
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
                            Style::default().fg(Color::Rgb(140, 140, 140)).bg(line_bg),
                        ),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(line_bg)),
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
                        .bg(Color::Rgb(59, 130, 246))
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    fname,
                    Style::default()
                        .fg(Color::Rgb(180, 180, 180))
                        .bg(Color::Rgb(38, 38, 38)),
                ));
                spans.push(Span::raw("  "));
            }
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(area.x, attach_y, area.width, 1),
            );
        }

        if gap_h > 0 {
            let gap_y = attach_y + attach_h;
            if gap_y >= area.y && gap_y < area.y + area.height {
                frame.render_widget(
                    Paragraph::new(Line::from("")),
                    Rect::new(area.x, gap_y, area.width, 1),
                );
            }
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
        let effort_display = if tui.hide_thinking {
            if tui.thinking_effort.is_empty() {
                "hidden".to_string()
            } else {
                format!("{} · hidden", tui.thinking_effort)
            }
        } else {
            tui.thinking_effort.clone()
        };
        let right_parts = if model_display.is_empty() {
            vec![Span::styled(
                effort_display.clone(),
                Style::default().fg(Color::Rgb(108, 108, 108)),
            )]
        } else {
            vec![
                Span::styled(
                    model_display.clone(),
                    Style::default().fg(Color::Rgb(140, 140, 140)),
                ),
                Span::styled(" \u{b7} ", Style::default().fg(Color::Rgb(80, 80, 80))),
                Span::styled(
                    effort_display.clone(),
                    Style::default().fg(Color::Rgb(108, 108, 108)),
                ),
            ]
        };
        // Cron ticking clock — next due countdown, ticks via tick_status
        let cron_display: Option<(String, Color)> =
            tui.next_cron.as_ref().and_then(|(name, next)| {
                let now = chrono::Utc::now();
                let secs = (*next - now).num_seconds();
                if secs < -120 {
                    None // past grace, stale
                } else if secs <= 0 {
                    Some((format!("⏰ {name} due!"), Color::Rgb(246, 173, 126)))
                } else if secs < 60 {
                    Some((format!("⏰ {name} {secs}s"), Color::Rgb(246, 173, 126)))
                } else if secs < 3600 {
                    let m = secs / 60;
                    let s = secs % 60;
                    Some((format!("⏰ {name} {m}m {s}s"), Color::Rgb(180, 160, 130)))
                } else if secs < 86400 {
                    let h = secs / 3600;
                    let m = (secs % 3600) / 60;
                    Some((format!("⏰ {name} {h}h {m}m"), Color::Rgb(140, 140, 140)))
                } else {
                    None // far future, don't clutter footer
                }
            });
        let cron_len = cron_display
            .as_ref()
            .map(|(s, _)| s.chars().count() + 3)
            .unwrap_or(0);
        let right_len = if model_display.is_empty() {
            effort_display.chars().count()
        } else {
            model_display.chars().count() + 3 + effort_display.chars().count()
        };
        let left_len =
            1 + ctx_display.chars().count() + 3 + cache_display.chars().count() + cron_len;
        let pad_len = w.saturating_sub(left_len + right_len);

        let cache_color = if hit_rate > 0.0 {
            Color::Rgb(130, 145, 130)
        } else {
            Color::Rgb(80, 80, 80)
        };

        let mut footer_spans = vec![
            Span::raw(" "),
            Span::styled(ctx_display, Style::default().fg(Color::Rgb(108, 108, 108))),
            Span::styled(" \u{b7} ", Style::default().fg(Color::Rgb(65, 65, 65))),
            Span::styled(cache_display, Style::default().fg(cache_color)),
        ];
        if let Some((cron_str, cron_color)) = cron_display {
            footer_spans.push(Span::styled(
                " \u{b7} ",
                Style::default().fg(Color::Rgb(65, 65, 65)),
            ));
            footer_spans.push(Span::styled(
                cron_str,
                Style::default().fg(cron_color).add_modifier(Modifier::BOLD),
            ));
        }
        footer_spans.push(Span::raw(" ".repeat(pad_len)));
        footer_spans.extend(right_parts);
        if footer_y < area.y + area.height {
            // Same full-bleed band as the input box above: without it the pad
            // cells keep the terminal default bg (dark gap on the right).
            let footer_block = Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)));
            frame.render_widget(
                Paragraph::new(Line::from(footer_spans)).block(footer_block),
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
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::EndSynchronizedUpdate
    );
    res?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_dock_seam_is_dynamic() {
        assert_eq!(status_dock_h(false, false, true), 0);
        assert_eq!(status_dock_h(true, true, true), 0); // question owns the viewport
        assert_eq!(status_dock_h(true, false, false), 2); // scrollback already blank: status + breath
        assert_eq!(status_dock_h(true, false, true), 3); // seam + status + breath
    }

    #[test]
    fn transcript_ends_blank_matches_ensure_gap() {
        use ratatui::style::{Color, Style};
        assert!(!transcript_ends_blank(&[]));
        assert!(transcript_ends_blank(&[Line::from("")]));
        assert!(transcript_ends_blank(&[Line::from(" ")])); // left_pad-only row
        assert!(!transcript_ends_blank(&[Line::from("text")]));
        // card / code padding rows carry a bg: they are edges, not gaps
        let bg = Style::default().bg(Color::Rgb(22, 22, 22));
        assert!(!transcript_ends_blank(&[Line::from("").style(bg)]));
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
