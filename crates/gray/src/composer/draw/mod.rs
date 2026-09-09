use std::time::Duration;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{MIN_VIEWPORT_H, PANEL_ROWS, Tui, VIEWPORT_H};

mod widgets;

pub(crate) use widgets::{
    build_input_box, queued_preview_lines, shimmer_spans, status_dock_h, thinking_style,
    transcript_ends_blank,
};

/// Blank rows inserted between the status/queued rows and the input so the
/// input + panel + footer sit flush at the viewport bottom. Without this the
/// fixed-height inline viewport leaves the unused rows below the footer.
pub(crate) fn filler_rows(
    area_h: u16,
    status_h: u16,
    queued_h: u16,
    box_rows: u16,
    attach_h: u16,
    panel_h: u16,
) -> u16 {
    area_h
        .saturating_sub(status_h + queued_h + box_rows + attach_h + 1)
        .saturating_sub(panel_h)
}

/// Spare cleared row kept above the input so a 1-row estimate-vs-layout
/// mismatch can't leave ghost shell text below the box (the `/` corruption
/// that forced the fixed-14 revert). Idle viewport is ~5 rows, not 14.
pub(crate) const SPARE_ROWS: u16 = 1;

/// Exact-fit viewport height for the given content, clamped to
/// `MIN_VIEWPORT_H..=VIEWPORT_H`. The spare is skipped while a status dock
/// is up: the dock already ends in a bare breathing row, so adding SPARE on
/// top renders a second blank row between `Working…` and the input.
pub(crate) fn desired_viewport_h(
    status_h: u16,
    queued_h: u16,
    box_rows: u16,
    panel_h: u16,
    attach_h: u16,
) -> u16 {
    let spare = if status_h > 0 { 0 } else { SPARE_ROWS };
    (status_h + queued_h + box_rows + panel_h + attach_h + 1 + spare)
        .clamp(MIN_VIEWPORT_H, VIEWPORT_H)
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

    // Exact-fit viewport with in-place resizing (codex parity):
    // Grow and shrink are applied directly to the terminal's viewport area without
    // recreating the terminal or re-probing cursor position via CPR, preventing
    // prompt doubling and cursor drift.
    let n = tui.queued_inputs.len();
    let queued_est: u16 = if question_active || n == 0 {
        0
    } else {
        1 + n.min(3) as u16 + u16::from(n > 3)
    };
    let panel_est: u16 = if question_active {
        PANEL_ROWS as u16
    } else {
        tui.matches.len().min(PANEL_ROWS) as u16
    };
    let box_rows_est: u16 = if question_active { 0 } else { box_h };
    let desired = desired_viewport_h(status_h, queued_est, box_rows_est, panel_est, attach_h);

    // frankentui lesson: synchronized-output bracketing (DEC2026) — one atomic
    // present per frame so the compositor never shows a torn frame.
    // Terminals without support ignore the sequence.
    crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::BeginSynchronizedUpdate
    )?;
    let _ = tui.terminal.set_viewport_height(desired, screen_size);
    tui.viewport_h = desired;

    let res = tui
        .terminal
        .draw(|frame| {
                let area = frame.area();
                let w = area.width as usize;

                let status_y = area.y;
                // Queued preview sits between status and input (codex PendingInputPreview
                // parity). Hidden while a question owns the viewport.
                let queued_preview: Vec<Line<'static>> = if question_active {
                    Vec::new()
                } else {
                    queued_preview_lines(&tui.queued_inputs, w)
                };
                let queued_h = queued_preview.len() as u16;
                // Space left for the completion/question panel once the fixed rows
                // (status, queued, input, attachments, footer) are placed.
                let avail = area.height.saturating_sub(
                    status_h + queued_h + if question_active { 0 } else { box_h } + attach_h + 1,
                );
                // Grow viewport to fit full question; fall back to PANEL_ROWS min when short on space.
                // Two-pass (uncapped then capped), no new layout engine.
                let need = if question_active {
                    tui.active_question
                        .as_ref()
                        .map(|q| {
                            super::question::panel_lines(q, tui.textarea.text(), w, 100).len()
                                as u16
                        })
                        .unwrap_or(PANEL_ROWS as u16)
                } else {
                    PANEL_ROWS as u16
                };
                let panel_cap = need.min(avail).max((PANEL_ROWS as u16).min(avail));
                let question_lines: Option<Vec<Line<'static>>> = if question_active {
                    tui.active_question.as_ref().map(|q| {
                        super::question::panel_lines(
                            q,
                            tui.textarea.text(),
                            w,
                            panel_cap.max(1) as usize,
                        )
                    })
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
                // Bottom-anchor the composer: unused viewport rows go ABOVE the input
                // as a blank gap, so the input + footer sit flush at the viewport
                // bottom instead of floating with a huge cleared area below them.
                let box_rows = if question_active { 0 } else { box_h };
                let filler =
                    filler_rows(area.height, status_h, queued_h, box_rows, attach_h, panel_h);
                debug_assert_eq!(filler, avail.saturating_sub(panel_h));
                let box_y = status_y + status_h + queued_h + filler;
                // Codex parity: while a question is active the question surface REPLACES
                // the composer — no input box, the panel occupies its slot.
                let panel_y = if question_active {
                    box_y
                } else {
                    box_y + box_rows
                };
                let attach_y = panel_y + panel_h;
                let footer_y = attach_y + attach_h;
                if filler > 0 {
                    frame.render_widget(
                        ratatui::widgets::Clear,
                        Rect::new(area.x, status_y + status_h + queued_h, area.width, filler),
                    );
                }

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
                let rendered_box_h = box_h.min(area.bottom().saturating_sub(box_y));
                if rendered_box_h > 0 && !question_active {
                    let box_block =
                        Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)));
                    frame.render_widget(
                        Paragraph::new(ibox.lines.clone()).block(box_block),
                        Rect::new(area.x, box_y, area.width, rendered_box_h),
                    );
                }

                if let Some(qlines) = &question_lines
                    && visible_count > 0
                {
                    // ponytail: clipped option window shows a count, no extra row.
                    let q_hidden = (need as usize).saturating_sub(visible_count);
                    for (i, line) in qlines.iter().enumerate().take(visible_count) {
                        let item_y = panel_y + i as u16;
                        if item_y < area.y || item_y >= area.y + area.height {
                            continue;
                        }
                        let is_last = i + 1 == visible_count;
                        let rendered: Line<'static> = if is_last && q_hidden > 0 {
                            Line::from(vec![Span::styled(
                                format!(" … ↓ +{} more", q_hidden),
                                Style::default()
                                    .fg(Color::Rgb(140, 140, 140))
                                    .bg(Color::Rgb(22, 22, 22)),
                            )])
                        } else {
                            line.clone()
                        };
                        frame.render_widget(
                            Paragraph::new(rendered).block(
                                Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22))),
                            ),
                            Rect::new(area.x, item_y, area.width, 1),
                        );
                    }
                    // Notes editing owns the cursor: park it on the notes row
                    // (second-to-last content row: notes, tips, bottom margin).
                    // Without this, Tab opens notes but typing lands invisibly.
                    if let Some(q) = tui.active_question.as_ref()
                        && q.notes_focused()
                        && q.notes_editor_visible()
                    {
                        let notes_at = qlines.len().saturating_sub(3);
                        if notes_at < visible_count {
                            let y = panel_y + notes_at as u16;
                            if y < area.y + area.height {
                                let text = tui.textarea.text();
                                let cursor = tui.textarea.cursor().min(text.len());
                                let col = 2 + unicode_width::UnicodeWidthStr::width(
                                    text.get(..cursor).unwrap_or(""),
                                );
                                frame.set_cursor_position(Position::new(area.x + col as u16, y));
                            }
                        }
                    }
                } else if visible_count > 0 {
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
                        let used_len = cmd_str.chars().count() + desc_str.chars().count();
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
                        let pad_len = w.saturating_sub(used_len + marker.chars().count());
                        let line_bg = if is_sel {
                            Color::Rgb(246, 173, 126)
                        } else {
                            Color::Rgb(28, 28, 28)
                        };
                        let marker_style = if is_sel {
                            Style::default()
                                .fg(Color::Black)
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
                                        .fg(Color::Black)
                                        .bg(line_bg)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(
                                    desc_str,
                                    Style::default().fg(Color::Rgb(40, 40, 40)).bg(line_bg),
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
                                    Style::default().fg(Color::Rgb(140, 140, 140)).bg(line_bg),
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

                let (_, max_label) = crate::setup::model_context_info(&tui.model_name);
                let (used_tokens, hit_rate) =
                    if let Some(u) = tui.latest_usage.or(tui.cumulative_usage) {
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
                let right_parts = {
                    let perm_badge: Option<(String, Color)> = match tui.permission_mode.as_str() {
                        "full" => Some((" · full access".to_string(), Color::Rgb(200, 120, 120))),
                        "read-only" => {
                            Some((" · read-only".to_string(), Color::Rgb(130, 145, 160)))
                        }
                        _ => None,
                    };
                    let mut v = if model_display.is_empty() {
                        if effort_display.is_empty() {
                            Vec::new()
                        } else {
                            vec![Span::styled(
                                effort_display.clone(),
                                Style::default().fg(Color::Rgb(108, 108, 108)),
                            )]
                        }
                    } else if effort_display.is_empty() {
                        vec![Span::styled(
                            model_display.clone(),
                            Style::default().fg(Color::Rgb(140, 140, 140)),
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
                    if let Some((badge, color)) = &perm_badge {
                        v.push(Span::styled(badge.clone(), Style::default().fg(*color)));
                    }
                    let badge_len = perm_badge.map(|(b, _)| b.chars().count()).unwrap_or(0);
                    (v, badge_len)
                };
                let (right_parts, badge_len) = right_parts;
                let right_len = if model_display.is_empty() {
                    effort_display.chars().count() + badge_len
                } else if effort_display.is_empty() {
                    model_display.chars().count() + badge_len
                } else {
                    model_display.chars().count() + 3 + effort_display.chars().count() + badge_len
                };
                let left_len = 1 + ctx_display.chars().count() + 3 + cache_display.chars().count();
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

                // A parked question owns the cursor (notes row above); never park it
                // on the hidden composer box instead.
                if tui.status.is_none() && !tui.is_task_running && tui.active_question.is_none() {
                    let cur_x = (area.x + 3 + ibox.cur_col as u16)
                        .min(area.x + area.width.saturating_sub(1));
                    let cur_y = (box_y + 1 + ibox.cur_row as u16)
                        .min(area.y + area.height.saturating_sub(1));
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
    fn filler_bottom_anchors_footer() {
        // Screenshot repro: 14-row inline viewport, status dock 3 + input 3,
        // no panel/matches -> footer must sit on the last viewport row, with
        // the slack above the input instead of below the footer.
        let filler = filler_rows(14, 3, 0, 3, 0, 0);
        assert_eq!(filler, 7);
        // status(3) + filler(7) + box(3) + footer(1) fills the viewport.
        assert_eq!(3 + filler + 3 + 1, 14);
        // Full panel consumes the slack: no gap below the footer either.
        assert_eq!(filler_rows(14, 3, 0, 3, 0, 6), 1);
        // Overflow saturates instead of underflowing.
        assert_eq!(filler_rows(5, 3, 0, 3, 0, 0), 0);
    }

    #[test]
    fn desired_viewport_collapses_idle_gap() {
        // Idle: input 3 + footer 1 + 1 spare = 5 rows, not 14. Filler is 1.
        assert_eq!(desired_viewport_h(0, 0, 3, 0, 0), 5);
        assert_eq!(filler_rows(5, 0, 0, 3, 0, 0), 1);
        // Slash popup: input 3 + panel 6 + footer 1 + spare 1 = 11.
        assert_eq!(desired_viewport_h(0, 0, 3, 6, 0), 11);
        // Running: dock already ends in a breathing row, so no spare —
        // status 2 + input 3 + footer 1 = 6, filler 0, single separator.
        assert_eq!(desired_viewport_h(2, 0, 3, 0, 0), 6);
        assert_eq!(filler_rows(6, 2, 0, 3, 0, 0), 0);
        // Running + full panel still fits: 3 + 3 + 6 + 1 = 13.
        assert_eq!(desired_viewport_h(3, 0, 3, 6, 0), 13);
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
