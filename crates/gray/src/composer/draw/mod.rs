use std::time::Duration;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{MIN_VIEWPORT_H, PANEL_ROWS, Tui};
use crate::text_width::display_width;

pub(crate) mod widgets;

pub(crate) use widgets::{
    HomeRows, InputCell, build_input_box, chip_cells, home_band_h, needs_seam,
    queued_preview_lines, shimmer_spans, status_dock_h, tail_block_h, tail_gap_h,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn desired_viewport_h(
    status_h: u16,
    queued_h: u16,
    live_h: u16,
    box_rows: u16,
    panel_h: u16,
    attach_h: u16,
    footer_h: u16,
    max_h: u16,
) -> u16 {
    (status_h + queued_h + live_h + box_rows + panel_h + attach_h + footer_h)
        .clamp(MIN_VIEWPORT_H, max_h)
}

pub(crate) fn latched_viewport_floor(
    status_h: u16,
    queued_h: u16,
    live_h: u16,
    box_rows: u16,
    attach_h: u16,
    footer_h: u16,
    max_h: u16,
) -> u16 {
    let with_dock = status_h + queued_h + live_h + box_rows + attach_h + footer_h;
    let no_status = queued_h + live_h + box_rows + attach_h + footer_h;

    let live_overrun = live_h + box_rows + attach_h + footer_h;
    with_dock.max(no_status).max(live_overrun).min(max_h)
}

pub(crate) fn attachment_cells(attachments: &[std::path::PathBuf], text: &str) -> Vec<InputCell> {
    let t = crate::theme::theme();
    let mut open = std::collections::HashMap::<String, usize>::new();
    for (_, name) in crate::repl::attachments::attachment_markers(text) {
        *open.entry(name).or_default() += 1;
    }
    let parts: Vec<(&str, String)> = attachments
        .iter()
        .filter(
            |path| match open.get_mut(&crate::repl::attachments::attachment_label(path)) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    false
                }
                _ => true,
            },
        )
        .map(|path| {
            let kind = crate::repl::attachments::attachment_kind(path).label();
            (kind, crate::repl::attachments::attachment_label(path))
        })
        .collect();
    let chips: Vec<(&str, &str)> = parts.iter().map(|(k, n)| (*k, n.as_str())).collect();
    chip_cells(
        &chips,
        Style::default().fg(t.text_bright).bg(t.input_bg),
        Style::default().fg(t.text_muted).bg(t.surface_bg),
        Style::default().bg(t.surface_bg),
    )
}

pub(crate) fn band_overlay(desired: u16, box_h: u16) -> u16 {
    box_h.saturating_sub(1).min(desired)
}

/// Completion rows that fit in `room` band rows: the panel needs one spacer
/// row above the box, and a panel that cannot fit at least one row is hidden
/// rather than drawn over the box.
pub(crate) fn popup_rows(matches: usize, room: u16) -> usize {
    matches
        .min(PANEL_ROWS)
        .min(usize::from(room.saturating_sub(1)))
}

pub(crate) fn viewport_cap(rows: u16) -> u16 {
    rows.saturating_sub(2).max(MIN_VIEWPORT_H)
}

pub(crate) fn widget_row_cap(screen_h: u16) -> u16 {
    (screen_h.saturating_mul(2) / 5).max(4)
}

pub(crate) fn footer_paint_row(footer_y: u16, area: Rect) -> Option<u16> {
    (area.height > 0).then(|| footer_y.min(area.y + area.height - 1))
}

pub(crate) fn band_budget(screen_h: u16, reserved_rows: u16) -> u16 {
    screen_h.saturating_sub(reserved_rows)
}

pub(crate) fn trim_to_allowance(counts: &mut [u16], allowance: u16) {
    let mut left = allowance;
    for count in counts.iter_mut() {
        let keep = left.min(*count);
        *count = keep;
        left = left.saturating_sub(keep);
    }
}

/// The footer's `effort` segment. A composite (Fusion) label already names
/// the lead's effort, so the segment keeps only the `hidden` marker there.
fn effort_segment_text(effort_display: &str, composite: bool, hide_thinking: bool) -> String {
    match (composite, hide_thinking) {
        (false, _) => effort_display.to_string(),
        (true, true) if !effort_display.is_empty() => "hidden".to_string(),
        (true, _) => String::new(),
    }
}

pub(crate) fn continue_ghost(
    allow_empty_submit: bool,
    is_task_running: bool,
    text: &str,
) -> Option<&'static str> {
    (allow_empty_submit && !is_task_running && text.trim().is_empty())
        .then_some(crate::repl::CONTINUE_GHOST)
}

pub(crate) fn footer_badge_visible(model: &str, snapshot: Option<bool>) -> bool {
    snapshot
        .unwrap_or_else(|| crate::setup::context::model_supports_reasoning(model) != Some(false))
}

pub(crate) fn draw(tui: &mut Tui) -> anyhow::Result<()> {
    tui.status_line_seen = crate::statusline::version();

    if tui.batch_depth > 0 {
        return Ok(());
    }
    frame(tui, true)
}

pub(crate) fn settle_band(tui: &mut Tui) -> anyhow::Result<()> {
    frame(tui, false)
}

fn frame(tui: &mut Tui, paint: bool) -> anyhow::Result<()> {
    if tui.modal_open {
        return Ok(());
    }
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let screen_size = ratatui::layout::Size::new(cols, rows);
    let w = cols as usize;

    let text = tui.textarea.text().to_string();
    let cursor = tui.textarea.cursor().min(text.len());

    let ghost = continue_ghost(tui.allow_empty_submit, tui.is_task_running, &text);
    let model_display = tui
        .model_label
        .clone()
        .unwrap_or_else(|| crate::setup::friendly_model_name(&tui.model_name));
    let line_cfg = crate::statusline::config();
    let effort_seg = line_cfg
        .left()
        .iter()
        .chain(line_cfg.right().iter())
        .any(|s| s == "effort");
    let show_effort = footer_badge_visible(&tui.model_name, tui.turn_show_effort);
    let effort_display = if !show_effort || tui.thinking_effort == "off" {
        String::new()
    } else if tui.hide_thinking {
        if tui.thinking_effort.is_empty() {
            "hidden".to_string()
        } else {
            format!("{} · hidden", tui.thinking_effort)
        }
    } else {
        tui.thinking_effort.clone()
    };
    let model_text = match effort_display.chars().next() {
        Some(c)
            if !effort_seg
                && tui.model_label_parts.is_none()
                && !model_display
                    .to_lowercase()
                    .contains(&tui.thinking_effort.to_lowercase()) =>
        {
            let rest = &effort_display[c.len_utf8()..];
            format!("{model_display} {}{rest}", c.to_uppercase())
        }
        _ => model_display.clone(),
    };
    let (home_model_name, home_model_tail) = tui
        .model_label_parts
        .as_ref()
        .map(|parts| {
            (
                parts.iter().map(|(t, _)| t.as_str()).collect::<String>(),
                String::new(),
            )
        })
        .unwrap_or_else(|| {
            if effort_display.is_empty()
                || model_display
                    .to_lowercase()
                    .contains(&tui.thinking_effort.to_lowercase())
            {
                (model_display.clone(), String::new())
            } else {
                (model_display.clone(), format!(" {effort_display}"))
            }
        });
    let home = tui.is_home().then_some(HomeRows {
        notice: tui.home_notice.as_deref(),
        stats: tui.plugin_stats.as_deref(),
        model: Some(home_model_name.as_str()),
        model_tail: (!home_model_tail.is_empty()).then_some(home_model_tail.as_str()),
        above: tui.home_update.as_deref(),
        quality_of_life: tui.home_quality_of_life.as_deref(),
        session: tui.home_session.as_deref(),
    });
    let home_mode = home.is_some();

    let attach_h: u16 = 0;
    let footer_h: u16 = u16::from(!home_mode);

    let needs_seam = needs_seam(&tui.transcript);
    let dock_live = tui.status.is_some() || tui.hint_active().is_some();
    // Home keeps one fixed toast row above the box, so a hint appearing or
    // expiring never shifts the centered box.
    let status_h: u16 = if home_mode {
        1
    } else {
        status_dock_h(dock_live, needs_seam)
    };

    let seam_h: u16 = u16::from(needs_seam);
    let n = tui.queued_inputs.len() + tui.queued_sends.len();
    let queued_est: u16 = if n == 0 {
        0
    } else {
        1 + n.min(3) as u16 + u16::from(n > 3) + 1
    };
    let search_h: u16 = u16::from(tui.search.is_some());

    let ask_est: u16 = tui
        .ask_modal
        .as_ref()
        .map(|m| m.rows.len().min(24) as u16)
        .unwrap_or(0);

    let live_est: u16 = tui
        .live_tool_rows(w)
        .iter()
        .map(|l| {
            crate::composer::transcript::wrap_styled_line(l.clone(), w.saturating_sub(4).max(1))
                .len()
                .max(1) as u16
        })
        .sum::<u16>()
        .saturating_add(u16::from(tui.live_tool_overflow() > 0));
    let stream_tail: Vec<Line<'static>> = tui.stream_tail_rows(w);

    let tail_gap = tail_gap_h(dock_live, &stream_tail);
    let tail_est = stream_tail.len() as u16 + tail_gap;

    let chips = attachment_cells(&tui.attachments, &text);
    let ibox = build_input_box(&text, cursor, w, ghost, home, &chips);
    // A consent/question prompt owns the bottom band: the input box (and
    // its cursor) step aside until it closes.
    let asking = tui.ask_modal.is_some();
    let ask_gap = u16::from(asking);
    let box_h = if asking {
        0
    } else {
        ibox.lines.len().max(1) as u16
    };

    // The completion popup shows only as many rows as the band can hold
    // next to the box and footer, so the whole panel always paints and the
    // band grows by exactly the panel it draws.
    // On home the popup drops down from the input band over the model and
    // hint rows instead of opening above it: the box stays put and the
    // welcome banner above is never scrolled away by a transient menu.
    let below_band = (ibox.lines.len().saturating_sub(ibox.lead_h + ibox.band_h)) as u16;
    let home_room = rows
        .saturating_sub(tui.terminal.viewport_area.y + status_h + box_h.saturating_sub(below_band));
    let home_drop = home_mode && !asking && home_room >= tui.matches.len().min(3) as u16;
    let panel_rows = if home_drop {
        popup_rows(tui.matches.len(), home_room + 1)
    } else {
        popup_rows(
            tui.matches.len(),
            viewport_cap(rows).saturating_sub(status_h + box_h + attach_h + footer_h),
        )
    };
    let panel_est: u16 = if panel_rows == 0 {
        search_h
    } else if home_drop {
        (panel_rows as u16).saturating_sub(below_band)
    } else {
        panel_rows as u16 + 1
    };

    let box_rows_est: u16 = box_h;
    let widget_budget = rows
        .saturating_sub(
            box_h + status_h + queued_est + live_est + tail_est + panel_est + attach_h + 2,
        )
        .min(widget_row_cap(rows));
    let mut widget_rows = tui.plugin_widget.rows(widget_budget as usize, w);
    let max_viewport_h = viewport_cap(rows);

    let mut band_allowance =
        band_budget(max_viewport_h, status_h + box_h + attach_h + panel_est + 1)
            .saturating_sub(2 * ask_gap)
            .max(if asking { ask_est.min(6) } else { 0 });

    let reserve_overflow_row = tui.live_tool_overflow() > 0 && band_allowance > 0;
    if reserve_overflow_row {
        band_allowance -= 1;
    }
    let mut reserved = [
        ask_est,
        queued_est,
        widget_rows.len() as u16,
        live_est,
        tail_est,
    ];
    trim_to_allowance(&mut reserved, band_allowance);
    let (ask_est, queued_est, widget_h, live_est, tail_est) = (
        reserved[0],
        reserved[1],
        reserved[2],
        reserved[3],
        tail_block_h(reserved[4], tail_gap),
    );
    let desired = desired_viewport_h(
        status_h,
        queued_est,
        live_est + tail_est + widget_h + ask_est + 2 * ask_gap,
        box_rows_est,
        panel_est,
        attach_h,
        footer_h,
        max_viewport_h,
    );

    let floor = latched_viewport_floor(
        status_h,
        queued_est,
        live_est + tail_est + widget_h + ask_est + 2 * ask_gap,
        box_rows_est,
        attach_h,
        footer_h,
        max_viewport_h,
    );
    let desired = desired.max(floor);
    // Only draft rows past the first are transient: the box's padding rows
    // and its single text row are always there, so they never cover
    // transcript rows.
    let draft_rows = if asking {
        0
    } else {
        ibox.band_h.saturating_sub(2).max(1) as u16
    };
    let overlay = band_overlay(desired, draft_rows);

    tui.begin_sync();

    if let Err(e) = tui
        .terminal
        .set_viewport_height(desired, overlay, screen_size)
    {
        tui.end_sync();
        return Err(e.into());
    }
    tui.viewport_h = desired;

    if !paint {
        tui.end_sync();
        return Ok(());
    }

    let live_headers: Vec<Line<'static>> = tui.live_tool_rows(w);
    let live_tail_counts: Vec<usize> = tui
        .live_tools
        .iter()
        .take(super::MAX_LIVE_TOOLS)
        .map(|t| t.tail.rows().len())
        .collect();
    let live_overflow = tui.live_tool_overflow();
    let ask_modal: Option<super::AskModal> = tui.ask_modal.clone();
    let compaction_elapsed = tui.compaction_elapsed();
    let turn_started = tui.turn_started;
    let is_task_running = tui.is_task_running;

    let pill_turn_output = tui.turn_output_accum;
    let pill_streamed = tui.streamed_bytes;
    let pill_tok_suffix = super::live_pill_suffix(pill_turn_output, pill_streamed);

    let cache_timer = tui.cache_remaining().map(|left| {
        (
            format!(
                "{} {}",
                crate::tui::glyph("\u{25f7}", "*"),
                crate::cache::format_remaining(left)
            ),
            if left < Duration::from_secs(60) {
                crate::theme::theme().text_dim
            } else {
                crate::theme::theme().cache_hit
            },
        )
    });

    let reveal = tui.home_reveal_alpha();
    let res = tui.terminal.draw(|frame| {
        let area = frame.area();

        frame.render_widget(
            Block::default().style(Style::default().bg(crate::theme::theme().window())),
            area,
        );
        let w = area.width as usize;

        let status_y = area.y;

        let mut queued_preview: Vec<Line<'static>> =
            queued_preview_lines(&tui.queued_inputs, &tui.queued_sends, w);

        let mut live_rows: Vec<Line<'static>> = Vec::new();
        let mut live_is_tail: Vec<bool> = Vec::new();
        {
            let mut card = 0usize;
            let mut pos_in_card = 0usize;
            for l in live_headers {
                let is_tail = pos_in_card > 0;
                for r in
                    crate::composer::transcript::wrap_styled_line(l, w.saturating_sub(4).max(1))
                {
                    live_rows.push(r);
                    live_is_tail.push(is_tail);
                }
                pos_in_card += 1;
                if pos_in_card > live_tail_counts.get(card).copied().unwrap_or(0) {
                    card += 1;
                    pos_in_card = 0;
                }
            }
        }
        let mut tail_rows = stream_tail;

        let mut ask_rows: Vec<Line<'static>> = ask_modal
            .as_ref()
            .map(|m| {
                m.rows
                    .iter()
                    .take(24)
                    .enumerate()
                    .flat_map(|(ri, (glyph, label, desc))| {
                        let theme = crate::theme::theme();
                        let (raw, style) = match glyph.as_str() {
                            "?" => (
                                label.clone(),
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            "\u{2502}" => (
                                format!("  \u{2502} {desc}"),
                                Style::default().fg(theme.text_dim),
                            ),
                            "\u{b7}" => {
                                (format!("    {desc}"), Style::default().fg(theme.text_dim))
                            }
                            _ => {
                                let marker = if ri == m.cursor {
                                    crate::tui::arrow_glyph()
                                } else {
                                    " "
                                };
                                let body = if glyph == "*" {
                                    format!("{label}: {desc}")
                                } else if (glyph.is_empty() || glyph == "~") && desc.is_empty() {
                                    label.clone()
                                } else if glyph.is_empty() || glyph == "~" {
                                    format!("{label}   {desc}")
                                } else {
                                    format!("{glyph} {label} — {desc}")
                                };
                                (
                                    format!("  {marker} {body}"),
                                    if ri == m.cursor {
                                        Style::default()
                                            .fg(theme.accent)
                                            .add_modifier(Modifier::BOLD)
                                    } else if glyph == "~" {
                                        Style::default().fg(theme.text_dim)
                                    } else {
                                        Style::default().fg(theme.text_muted)
                                    },
                                )
                            }
                        };
                        crate::composer::transcript::wrap_styled_line(
                            Line::from(vec![Span::styled(raw, style)]),
                            w.saturating_sub(4).max(1),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut rows_h = [
            ask_rows.len() as u16,
            queued_preview.len() as u16,
            widget_rows.len() as u16,
            live_rows.len() as u16,
            tail_rows.len() as u16 + tail_gap,
        ];
        trim_to_allowance(&mut rows_h, band_allowance);
        ask_rows.truncate(rows_h[0] as usize);
        queued_preview.truncate(rows_h[1] as usize);
        widget_rows.truncate(rows_h[2] as usize);
        while live_rows.len() > rows_h[3] as usize {
            if let Some(idx) = live_is_tail.iter().rposition(|&t| t) {
                live_rows.remove(idx);
                live_is_tail.remove(idx);
            } else {
                live_rows.pop();
                live_is_tail.pop();
            }
        }
        let tail_h = tail_block_h(rows_h[4], tail_gap);

        let tail_text = tail_h.saturating_sub(tail_gap);
        tail_rows.drain(..tail_rows.len() - tail_text as usize);
        let show_live_overflow = reserve_overflow_row;
        let live_h = rows_h[3] + u16::from(show_live_overflow);
        let queued_h = rows_h[1];
        let ask_h = rows_h[0];
        let widget_h = rows_h[2];

        let visible_count = panel_rows;
        let panel_gap: u16 = if visible_count > 0 && !home_drop {
            visible_count as u16 + 1
        } else if visible_count > 0 {
            0
        } else {
            search_h
        };
        let panel_h: u16 = 0;
        let box_rows = box_h;
        let tail_y = status_y + seam_h;
        let queued_y = status_y + status_h + tail_h;
        let live_y = queued_y + queued_h;
        let ask_y = live_y + live_h + ask_gap;
        let widget_y = ask_y + ask_h + ask_gap;
        let box_y = widget_y + widget_h + panel_gap;
        let panel_y = if home_mode {
            (area.y + area.height.saturating_sub(1)).saturating_sub(panel_h)
        } else {
            box_y + box_rows
        };
        let attach_y = panel_y + panel_h;
        let footer_y = attach_y + attach_h;

        if let Some((started, label)) = &tui.status {
            let label_text = format!(" {} {label}\u{2026}", crate::tui::glyph("\u{2b21}", "o"));

            let elapsed = match compaction_elapsed {
                Some(d) => d,
                None => super::pill_elapsed(turn_started, *started, is_task_running),
            };
            let mut spans = shimmer_spans(&label_text, elapsed);

            let tps_suffix = match compaction_elapsed {
                Some(_) => String::new(),
                None => super::live_tps_suffix(pill_turn_output, pill_streamed, tui.turn_stream_ms),
            };
            let elapsed_str = format!("{:.1}s", elapsed.as_secs_f64());
            let suffix = format!(" {elapsed_str}{pill_tok_suffix}{tps_suffix} (esc to interrupt)");
            spans.push(Span::styled(
                suffix,
                Style::default().fg(crate::theme::theme().tool_dim),
            ));
            frame.render_widget(
                Paragraph::new(Line::from(spans)),
                Rect::new(area.x, status_y + seam_h + tail_h, area.width, 1),
            );
        } else if let Some((shown, hint)) = &tui.hint
            && shown.elapsed() < super::HINT_TTL
        {
            let mut line = tui.hint_line.clone().unwrap_or_else(|| {
                Line::from(Span::styled(
                    hint.clone(),
                    Style::default().fg(crate::theme::theme().tool_dim),
                ))
            });
            line.spans.insert(0, Span::raw(" "));
            // On home the toast sits over the centered box, not at column 0.
            let x = if home_mode {
                (area.x + ibox.x as u16).min(area.right())
            } else {
                area.x
            };
            frame.render_widget(
                Paragraph::new(line),
                Rect::new(x, status_y + seam_h + tail_h, area.right() - x, 1),
            );
        }

        for (i, line) in queued_preview.iter().enumerate() {
            let y = queued_y + i as u16;
            if y < area.y || y >= area.y + area.height {
                continue;
            }
            frame.render_widget(
                chrome_row(line.clone()),
                Rect::new(area.x, y, area.width, 1),
            );
        }
        for (i, line) in tail_rows.iter().enumerate() {
            let y = tail_y + i as u16;
            if y < area.y || y >= area.y + area.height {
                continue;
            }
            frame.render_widget(
                Paragraph::new(line.clone()),
                Rect::new(area.x, y, area.width, 1),
            );
        }
        for (i, line) in live_rows.iter().enumerate() {
            let y = live_y + i as u16;
            if y < area.y || y >= area.y + area.height {
                continue;
            }
            frame.render_widget(
                live_tool_row(line.clone()),
                Rect::new(area.x, y, area.width, 1),
            );
        }
        if show_live_overflow {
            let y = live_y + live_rows.len() as u16;
            if y >= area.y && y < area.y + area.height {
                frame.render_widget(
                    chrome_row(Line::from(vec![Span::styled(
                        format!("    … +{live_overflow} more"),
                        Style::default()
                            .fg(crate::theme::theme().text_muted)
                            .add_modifier(Modifier::DIM)
                            .add_modifier(Modifier::ITALIC),
                    )])),
                    Rect::new(area.x, y, area.width, 1),
                );
            }
        }
        for (i, line) in ask_rows.iter().enumerate() {
            let y = ask_y + i as u16;
            if y < area.y || y >= area.y + area.height {
                continue;
            }
            frame.render_widget(
                chrome_row(line.clone()),
                Rect::new(area.x, y, area.width, 1),
            );
        }
        for (i, line) in widget_rows.iter().enumerate() {
            let y = widget_y + i as u16;
            if y < area.bottom() {
                frame.render_widget(
                    chrome_row(line.clone()),
                    Rect::new(area.x, y, area.width, 1),
                );
            }
        }
        let rendered_box_h = box_h.min(area.bottom().saturating_sub(box_y));
        if rendered_box_h > 0 {
            let lead = (ibox.lead_h as u16).min(rendered_box_h);
            let band_h = (ibox.band_h as u16).min(rendered_box_h.saturating_sub(lead));
            let box_x = area.x + (ibox.x as u16).min(area.width);
            let box_w = (ibox.width as u16).min(area.right().saturating_sub(box_x));
            frame.render_widget(
                Block::default().style(Style::default().bg(crate::theme::theme().surface_bg)),
                Rect::new(box_x, box_y + lead, box_w, band_h),
            );

            frame.render_widget(
                Paragraph::new(ibox.lines.clone()),
                Rect::new(box_x, box_y, area.right() - box_x, rendered_box_h),
            );
        }

        if let Some(search) = &tui.search {
            let q = search.query.clone();
            let no_match = search.idx.is_none();
            let mut spans = vec![Span::styled(
                " search: ".to_string(),
                Style::default()
                    .fg(crate::theme::theme().accent)
                    .add_modifier(Modifier::BOLD),
            )];
            spans.push(Span::styled(
                q,
                Style::default().fg(crate::theme::theme().text_body),
            ));
            if no_match {
                spans.push(Span::styled(
                    "  (no match)".to_string(),
                    Style::default()
                        .fg(crate::theme::theme().text_muted)
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            let y = box_y.saturating_sub(1);
            if y < area.bottom() {
                frame.render_widget(
                    chrome_row(Line::from(spans)),
                    Rect::new(area.x, y, area.width, 1),
                );
            }
        }

        if visible_count > 0 {
            let start = tui
                .sel
                .saturating_sub(visible_count.saturating_sub(1))
                .min(tui.sel);

            let total = tui.matches.len();
            let end = (start + visible_count).min(total);
            let hidden_above = start;
            let hidden_below = total.saturating_sub(end);

            let pad_x: u16 = 2;
            let mut item_w = 24usize;
            for (name, desc) in tui.matches.iter().skip(start).take(visible_count) {
                let sw = theme_row_swatch(name).map_or(0, |sw| {
                    sw.iter()
                        .map(|sp| display_width(sp.content.as_ref()))
                        .sum::<usize>()
                });
                item_w = item_w.max(
                    display_width(&format!(" /{name} "))
                        + display_width(&format!(" {desc} "))
                        + sw
                        + 1,
                );
            }
            let pop_w = (item_w as u16 + pad_x * 2)
                .min(area.width.saturating_sub(4))
                .max(30);
            let inner_w = (pop_w - pad_x * 2) as usize;
            let pop_h = visible_count as u16;
            let pop_x = (area.x + ibox.x as u16).min(area.right().saturating_sub(pop_w));
            let pop_y = if home_drop {
                (box_y + (ibox.lead_h + ibox.band_h) as u16)
                    .min(area.bottom().saturating_sub(pop_h))
            } else {
                box_y.saturating_sub(pop_h).max(area.y)
            };
            let rect = Rect::new(pop_x, pop_y, pop_w, pop_h);
            if home_drop {
                // Blank the home rows the dropdown covers, edge to edge, so
                // no half-hidden model/hint text pokes out beside it.
                let covered = Rect::new(area.x, pop_y, area.width, pop_h).intersection(area);
                frame.render_widget(ratatui::widgets::Clear, covered);
                frame.render_widget(
                    Block::default().style(Style::default().bg(crate::theme::theme().window())),
                    covered,
                );
            }
            frame.render_widget(ratatui::widgets::Clear, rect);
            frame.render_widget(
                Block::default().style(Style::default().bg(crate::theme::theme().surface_bg)),
                rect,
            );

            for (i, (name, desc)) in tui
                .matches
                .iter()
                .enumerate()
                .skip(start)
                .take(visible_count)
            {
                let item_y = pop_y + (i - start) as u16;
                let is_sel = i == tui.sel;
                let cmd_str = format!(" /{name} ");
                let desc_str = format!(" {desc} ");
                let swatch = theme_row_swatch(name);
                let used_len = display_width(&cmd_str)
                    + display_width(&desc_str)
                    + swatch.as_ref().map_or(0, |s| {
                        s.iter()
                            .map(|sp| display_width(sp.content.as_ref()))
                            .sum::<usize>()
                    });

                let marker = match (
                    i == start && hidden_above > 0,
                    i + 1 == end && hidden_below > 0,
                ) {
                    (true, true) => "↕",
                    (true, false) => "↑",
                    (false, true) => "↓",
                    (false, false) => "",
                };
                let pad_len = inner_w.saturating_sub(used_len + display_width(marker));
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
                        .fg(crate::theme::theme().text_bright)
                        .bg(line_bg)
                        .add_modifier(Modifier::BOLD)
                };
                let line = if is_sel {
                    let mut spans = vec![
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
                    ];
                    if let Some(sw) = swatch {
                        spans.extend(sw);
                    }
                    spans.push(Span::styled(marker.to_string(), marker_style));
                    Line::from(spans)
                } else {
                    let mut spans = vec![
                        Span::styled(
                            cmd_str,
                            Style::default()
                                .fg(crate::theme::theme().text_bright)
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
                    ];
                    if let Some(sw) = swatch {
                        spans.extend(sw);
                    }
                    spans.push(Span::styled(marker.to_string(), marker_style));
                    Line::from(spans)
                };
                frame.render_widget(
                    Paragraph::new(line)
                        .block(Block::default().style(Style::default().bg(line_bg))),
                    Rect::new(pop_x + pad_x, item_y, inner_w as u16, 1),
                );
            }
        }

        if !home_mode {
            let (max_tokens, max_label) = crate::setup::model_context_info(&tui.model_name);

            let has_usage = tui.latest_usage.or(tui.cumulative_usage).is_some();
            let (used_tokens, hit_rate) = if let Some(u) = tui.latest_usage.or(tui.cumulative_usage)
            {
                (
                    super::live_context_total(Some(u), tui.streamed_bytes),
                    u.cache_hit_rate() * 100.0,
                )
            } else if tui.streamed_bytes > 0 {
                (super::live_context_total(None, tui.streamed_bytes), 0.0)
            } else {
                (0, 0.0)
            };
            let ctx_pct = (used_tokens * 100).checked_div(max_tokens).unwrap_or(0);
            let ctx_display = format!(
                "{} / {}",
                crate::setup::format_context_length(used_tokens),
                max_label
            );
            let ctx_compact = format!("ctx {ctx_pct}%");
            let cache_display = format!("{hit_rate:.1}% cache");

            let cache_color = if hit_rate > 0.0 {
                crate::theme::theme().cache_hit
            } else {
                crate::theme::theme().text_faint
            };

            let sep = line_cfg.separator();
            let muted = crate::theme::theme().text_muted;

            let level = std::cell::Cell::new(0u8);
            let dir_name = std::path::Path::new(&tui.cwd)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let compute_builtin = |name: &str| -> Option<crate::statusline::Seg> {
                use crate::statusline::Seg;
                let t = crate::theme::theme();
                let pieces = |parts: Vec<(String, Color)>| Seg {
                    text: parts.iter().map(|(s, _)| s.as_str()).collect(),
                    color: muted,
                    parts: Some(parts),
                };
                Some(match name {
                    "cache" | "timer" if level.get() > 3 => Seg::new("", muted),
                    "location" => {
                        let mut parts = vec![("\u{28ff} ".to_string(), t.text_bright)];
                        let dir = if level.get() > 1 {
                            dir_name.clone()
                        } else {
                            crate::statusline::short_cwd(&tui.cwd)
                        };
                        parts.push((dir, t.text_body));
                        let branch = crate::statusline::git_branch(std::path::Path::new(&tui.cwd));
                        if level.get() == 0 && !branch.is_empty() {
                            parts.push((format!(" ({branch})"), muted));
                        }
                        pieces(parts)
                    }
                    "state" => match turn_started.filter(|_| is_task_running) {
                        Some(start) => {
                            let frame = (start.elapsed().as_millis() / 80) as usize % 10;
                            let spin = if crate::tui::unicode_safe() {
                                "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".chars().nth(frame).unwrap_or('⠋')
                            } else {
                                "|/-\\".chars().nth(frame % 4).unwrap_or('|')
                            };
                            pieces(vec![
                                (spin.to_string(), muted),
                                (" working".to_string(), t.text_bright),
                            ])
                        }
                        None => Seg::new(
                            format!("{} idle", crate::tui::glyph("\u{25cb}", "o")),
                            muted,
                        ),
                    },
                    "context" if level.get() > 2 => Seg::new(ctx_compact.clone(), muted),
                    "context" => Seg::new(ctx_display.clone(), muted),
                    "cache" if !has_usage => Seg::new("", muted),
                    "cache" => Seg::new(cache_display.clone(), cache_color),
                    "timer" => cache_timer
                        .as_ref()
                        .map(|(text, color)| Seg::new(text.clone(), *color))
                        .unwrap_or_else(|| Seg::new("", muted)),
                    "work" => Seg::new(tui.background_work.clone().unwrap_or_default(), t.accent),
                    "model" => {
                        let mut seg = Seg::new(model_text.clone(), t.text_bright);
                        if let Some(parts) = &tui.model_label_parts {
                            seg.parts = Some(
                                parts
                                    .iter()
                                    .map(|(txt, slot)| {
                                        (
                                            txt.clone(),
                                            if slot.is_some() { t.text_bright } else { muted },
                                        )
                                    })
                                    .collect(),
                            );
                        }
                        seg
                    }
                    "effort" => Seg::new(
                        effort_segment_text(
                            &effort_display,
                            tui.model_label_parts.is_some(),
                            tui.hide_thinking,
                        ),
                        muted,
                    ),
                    "cwd" => Seg::new(crate::statusline::short_cwd(&tui.cwd), muted),
                    "dir" => Seg::new(dir_name.clone(), muted),
                    "branch" => Seg::new(
                        crate::statusline::git_branch(std::path::Path::new(&tui.cwd)),
                        t.tool_dim,
                    ),
                    "command" => Seg::new(crate::statusline::command_output(), muted),
                    "status" => Seg::new(crate::statusline::all_statuses(&sep), muted),
                    other => match other.strip_prefix("status:") {
                        Some(key) => {
                            Seg::new(crate::statusline::status(key).unwrap_or_default(), muted)
                        }
                        None => return None,
                    },
                })
            };

            let seg_memo: std::cell::RefCell<
                std::collections::HashMap<String, Vec<Option<crate::statusline::Seg>>>,
            > = Default::default();
            let builtin = |name: &str| -> Option<crate::statusline::Seg> {
                let l = level.get() as usize;
                if let Some(hit) = seg_memo
                    .borrow()
                    .get(name)
                    .and_then(|slots| slots.get(l))
                    .and_then(|slot| slot.as_ref())
                {
                    return Some(hit.clone());
                }
                let seg = compute_builtin(name);
                if let Some(seg) = &seg {
                    let mut memo = seg_memo.borrow_mut();
                    if let Some(slot) = memo
                        .entry(name.to_string())
                        .or_insert_with(|| vec![None; 5])
                        .get_mut(l)
                    {
                        *slot = Some(seg.clone());
                    }
                }
                seg
            };
            let sep_color = crate::theme::theme().text_faint;
            let right_sep = line_cfg.right_separator();
            let seg_width = |segs: &[crate::statusline::Seg]| -> usize {
                segs.iter().map(|s| display_width(&s.text)).sum()
            };

            let (mut left_segs, mut right_segs) = (Vec::new(), Vec::new());
            for l in 0..=4 {
                level.set(l);
                left_segs =
                    crate::statusline::compose(&line_cfg.left(), &builtin, &sep, sep_color, muted);
                right_segs = crate::statusline::compose(
                    &line_cfg.right(),
                    &builtin,
                    &right_sep,
                    sep_color,
                    muted,
                );
                if seg_width(&left_segs) + seg_width(&right_segs) + 5 <= w {
                    break;
                }
            }
            let left_segs =
                crate::statusline::clip(left_segs, w.saturating_sub(seg_width(&right_segs) + 5));
            if line_cfg.command.is_some() {
                static LAST_FACTS_KEY: std::sync::Mutex<Option<String>> =
                    std::sync::Mutex::new(None);
                let branch = crate::statusline::git_branch(std::path::Path::new(&tui.cwd));
                let key = format!(
                    "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                    tui.cwd,
                    tui.model_name,
                    model_display,
                    tui.thinking_effort,
                    used_tokens,
                    max_label,
                    hit_rate.to_bits(),
                    branch,
                );
                let fresh = match LAST_FACTS_KEY.lock() {
                    Ok(mut g) if g.as_deref() != Some(key.as_str()) => {
                        *g = Some(key);
                        true
                    }
                    _ => false,
                };
                if fresh {
                    crate::statusline::publish_facts(serde_json::json!({
                        "cwd": tui.cwd,
                        "workspace": { "current_dir": tui.cwd },
                        "model": { "id": tui.model_name, "display_name": model_display },
                        "effort": tui.thinking_effort,
                        "context": { "used_tokens": used_tokens, "max": max_label },
                        "cache_hit_rate": hit_rate,
                        "branch": branch,
                        "version": env!("CARGO_PKG_VERSION"),
                    }));
                }
            }
            let left_len = 1 + seg_width(&left_segs);
            let right_len = seg_width(&right_segs) + 1;
            let pad_len = w.saturating_sub(left_len + right_len);
            let to_spans = |s: crate::statusline::Seg| -> Vec<Span<'static>> {
                match s.parts {
                    Some(parts) => parts
                        .into_iter()
                        .map(|(text, color)| Span::styled(text, Style::default().fg(color)))
                        .collect(),
                    None => vec![Span::styled(s.text, Style::default().fg(s.color))],
                }
            };
            let mut footer_spans = vec![Span::raw(" ")];
            footer_spans.extend(left_segs.into_iter().flat_map(to_spans));
            footer_spans.push(Span::raw(" ".repeat(pad_len)));
            footer_spans.extend(right_segs.into_iter().flat_map(to_spans));

            if let Some(footer_y) = footer_paint_row(footer_y, area) {
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
        }

        let (cur_x, cur_y) = if let Some(search) = &tui.search {
            let x = area.x + 9 + display_width(&search.query) as u16;
            (
                x.min(area.x + area.width.saturating_sub(1)),
                box_y.saturating_sub(1),
            )
        } else {
            (
                (area.x + ibox.x as u16 + 3 + ibox.cur_col as u16)
                    .min(area.x + area.width.saturating_sub(1)),
                (box_y + ibox.cur_row as u16).min(area.y + area.height.saturating_sub(1)),
            )
        };
        if !asking {
            frame.set_cursor_position(Position::new(cur_x, cur_y));
        }
        if let Some(alpha) = reveal {
            crate::composer::fade_buffer(frame.buffer, alpha);
        }
    });
    let background_result = if res.is_ok() {
        if let Some(bg) = &mut tui.background {
            bg.draw(&mut std::io::stdout().lock(), cols, rows)
        } else {
            Ok(())
        }
    } else {
        Ok(())
    };
    tui.end_sync();
    res?;
    background_result?;
    Ok(())
}

fn live_tool_row(line: Line<'static>) -> Paragraph<'static> {
    Paragraph::new(line).block(
        Block::default()
            .padding(ratatui::widgets::Padding::left(1))
            .style(Style::default().bg(crate::theme::theme().surface_bg)),
    )
}

fn chrome_row(line: Line<'static>) -> Paragraph<'static> {
    Paragraph::new(line)
        .block(Block::default().style(Style::default().bg(crate::theme::theme().surface_bg)))
}

fn theme_row_swatch(name: &str) -> Option<Vec<Span<'static>>> {
    let theme_name = name
        .strip_prefix("theme ")
        .or_else(|| name.strip_prefix("themes "))?;
    let t = if crate::theme::is_builtin_name(theme_name) {
        crate::theme::GRAY_UI_THEME
    } else {
        let dir = crate::theme::themes_dir().ok()?;
        crate::theme::load_theme_in(&dir, theme_name).ok()?.0
    };
    let mut spans = vec![Span::raw(" ")];
    spans.extend(crate::theme::swatch_spans(&t));
    Some(spans)
}
