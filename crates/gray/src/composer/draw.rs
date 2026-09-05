use std::time::Duration;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{PANEL_ROWS, Tui};

pub(crate) fn thinking_style() -> Style {
    Style::default()
        .fg(Color::Rgb(140, 140, 140))
        .add_modifier(Modifier::ITALIC)
}

pub(crate) fn shimmer_spans(text: &str, elapsed: Duration, truecolor: bool) -> Vec<Span<'static>> {
    use ratatui::style::Color;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() { return Vec::new(); }
    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let sweep_seconds = 2.0f32;
    let pos = ((elapsed.as_secs_f32() % sweep_seconds) / sweep_seconds * (period as f32)) as usize;
    let band_half_width = 5.0f32;
    const BASE: (u8, u8, u8) = (150, 148, 144);
    const HIGHLIGHT: (u8, u8, u8) = (255, 255, 255);
    chars.iter().enumerate().map(|(i, ch)| {
        let dist = ((i as isize + padding as isize) - pos as isize).abs() as f32;
        let t = if dist <= band_half_width { 0.5 * (1.0 + (std::f32::consts::PI * (dist / band_half_width)).cos()) } else { 0.0 };
        let style = if truecolor {
            let k = t.clamp(0.0, 1.0) * 0.9;
            let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k) as u8;
            Style::default().fg(Color::Rgb(lerp(BASE.0, HIGHLIGHT.0), lerp(BASE.1, HIGHLIGHT.1), lerp(BASE.2, HIGHLIGHT.2))).add_modifier(if t > 0.3 { Modifier::BOLD } else { Modifier::empty() })
        } else if t < 0.2 { Style::default().add_modifier(Modifier::DIM) } else if t < 0.6 { Style::default() } else { Style::default().add_modifier(Modifier::BOLD) };
        Span::styled(ch.to_string(), style)
    }).collect()
}

/// Input box render state: styled lines (own internal top/bottom margin rows,
/// `❯` prompt) plus the cursor position within them.
pub(crate) struct InputBox {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) cur_row: usize,
    pub(crate) cur_col: usize,
}

/// Builds the prompt box lines from textarea state. Hoisted out of the draw
/// closure so the dock height (and thus the viewport) can be sized before
/// `terminal.draw` runs.
pub(crate) fn build_input_box(text: &str, cursor: usize, w: usize) -> InputBox {
    let content_w = w.saturating_sub(4).max(1);

    // Neutral Gray palette (no blue)
    let bg_color = Color::Rgb(22, 22, 22);
    let prompt_color = Color::Rgb(180, 180, 180);
    let text_primary = Color::Rgb(225, 225, 225);

    let mut box_lines: Vec<Line<'static>> = Vec::new();

    // Top padding inside the box
    box_lines.push(Line::from(""));

    // Prompt input rows
    let prompt_arrow = " ❯ ";
    let arrow_span = Span::styled(prompt_arrow, Style::default().fg(prompt_color).add_modifier(Modifier::BOLD).bg(bg_color));

    let mut cur_row = 0usize;
    let mut cur_col = 0usize;

    if text.is_empty() {
        box_lines.push(Line::from(vec![arrow_span.clone()]));
    } else {
        let lines_raw: Vec<&str> = text.split('\n').collect();
        let mut cursor_found = false;
        let mut current_byte_pos = 0usize;
        let mut row_count = 0usize;

        for (i, raw_line) in lines_raw.iter().enumerate() {
            let prefix_span = if i == 0 {
                arrow_span.clone()
            } else {
                Span::styled("   ", Style::default().bg(bg_color))
            };

            let line_len_bytes = raw_line.len();
            let line_end_bytes = current_byte_pos + line_len_bytes;
            let has_cursor = !cursor_found && (cursor <= line_end_bytes || i == lines_raw.len() - 1);

            if raw_line.is_empty() {
                box_lines.push(Line::from(vec![prefix_span]));
                if has_cursor {
                    cur_row = row_count;
                    cur_col = 0;
                    cursor_found = true;
                }
                row_count += 1;
            } else {
                let chars: Vec<char> = raw_line.chars().collect();
                let mut line_byte_offset = 0usize;

                for (chunk_idx, chunk) in chars.chunks(content_w).enumerate() {
                    let s: String = chunk.iter().collect();
                    let chunk_byte_len: usize = chunk.iter().map(|c| c.len_utf8()).sum();

                    if chunk_idx == 0 {
                        box_lines.push(Line::from(vec![
                            prefix_span.clone(),
                            Span::styled(s, Style::default().fg(text_primary).bg(bg_color)),
                        ]));
                    } else {
                        box_lines.push(Line::from(vec![
                            Span::styled("   ", Style::default().bg(bg_color)),
                            Span::styled(s, Style::default().fg(text_primary).bg(bg_color)),
                        ]));
                    }

                    if has_cursor && !cursor_found {
                        let cursor_in_line_bytes = cursor.saturating_sub(current_byte_pos);
                        if cursor_in_line_bytes <= line_byte_offset + chunk_byte_len || chunk_idx == chars.chunks(content_w).count() - 1 {
                            cur_row = row_count;
                            let bytes_into_chunk = cursor_in_line_bytes.saturating_sub(line_byte_offset);
                            let mut col = 0usize;
                            let mut b = 0usize;
                            for ch in chunk {
                                if b >= bytes_into_chunk { break; }
                                b += ch.len_utf8();
                                col += 1;
                            }
                            cur_col = col;
                            cursor_found = true;
                        }
                    }

                    line_byte_offset += chunk_byte_len;
                    row_count += 1;
                }
            }

            current_byte_pos = line_end_bytes + 1;
        }
    }

    // Bottom padding inside the box
    box_lines.push(Line::from(""));

    InputBox { lines: box_lines, cur_row, cur_col }
}

/// True when the last scrollback row is a bare blank (no bg, no glyphs):
/// the transcript already left breathing room above the viewport (a
/// paragraph separator, `ensure_gap`, …). Same predicate `ensure_gap` uses.
pub(crate) fn transcript_ends_blank(transcript: &[Line<'static>]) -> bool {
    transcript.last().is_some_and(|l| {
        l.style.bg.is_none()
            && l.spans.iter().all(|s| s.style.bg.is_none() && s.content.trim().is_empty())
    })
}

/// Height reserved above the input box for the live status:
///   seam    1 row — ONLY when the transcript's last row is not already blank
///   status  1 row — the plain shimmer text
///   breath  1 row — bare space below, so it never melts into the input box
///
/// The seam is dynamic on purpose. A fixed seam (b377846) stacked with the
/// blank a paragraph checkpoint leaves behind (2-row gap, hence 8ec9af0);
/// no seam (current) jams thinking rows, list items, code fences and
/// partial paragraphs flush against `⬡ Working…`. Deciding per frame gives
/// exactly one blank row above the status — `ensure_gap(1)` for the dock.
pub(crate) fn status_dock_h(has_status: bool, question_active: bool, needs_seam: bool) -> u16 {
    if !has_status || question_active {
        return 0;
    }
    2 + u16::from(needs_seam)
}

/// Queued follow-up inputs held while a turn is in flight (codex
/// `PendingInputPreview` parity, minimal): header + `↳` dim-italic rows.
/// One row per queued message, first line only, truncated to `w`.
pub(crate) fn queued_preview_lines(
    queued: &std::collections::VecDeque<(String, Vec<std::path::PathBuf>)>,
    w: usize,
) -> Vec<Line<'static>> {
    if queued.is_empty() {
        return Vec::new();
    }
    let dim = Style::default().fg(Color::Rgb(140, 140, 140));
    let dim_italic = Style::default()
        .fg(Color::Rgb(140, 140, 140))
        .add_modifier(Modifier::DIM)
        .add_modifier(Modifier::ITALIC);
    let mut lines = vec![Line::from(vec![
        Span::styled("• ", dim),
        Span::styled(format!("Queued follow-up inputs ({})", queued.len()), dim),
    ])];
    // Viewport is only 10 rows — cap preview so input + footer stay visible.
    let max_show = 3usize;
    for (text, attached) in queued.iter().take(max_show) {
        let first = text.lines().next().unwrap_or("").trim();
        let mut preview: String = first.chars().take(w.saturating_sub(6).max(8)).collect();
        if first.chars().count() > preview.chars().count() {
            preview.push('…');
        }
        if !attached.is_empty() {
            preview.push_str(&format!(" [+{} image]", attached.len()));
        }
        lines.push(Line::from(vec![
            Span::styled("  ↳ ", dim),
            Span::styled(preview, dim_italic),
        ]));
    }
    if queued.len() > max_show {
        lines.push(Line::from(vec![Span::styled(
            format!("    … +{} more", queued.len() - max_show),
            dim_italic,
        )]));
    }
    lines
}

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
    let seam_h: u16 = if status_h > 0 { u16::from(needs_seam) } else { 0 };

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
        let boot_lines: &[Line<'static>] = if question_active { &[] } else { &boot_panel_lines };
        let boot_gap_h: u16 = u16::from(!boot_lines.is_empty());
        let queued_h = (queued_preview.len() + boot_lines.len()) as u16 + boot_gap_h;
        let box_y = status_y + status_h + queued_h;
    // Bare breathing row between the band (or panel/attachments below it)
    // and the footer, so the input never melts into it. The box's own pads
    // share the band bg and can't do this job. Question panels carry their
    // own bottom margin.
    let gap_h: u16 = if question_active { 0 } else { 1 };
    let avail = area.height.saturating_sub(status_h + queued_h + if question_active { 0 } else { box_h } + attach_h + gap_h + 1);
        // Grow viewport to fit full question; fall back to PANEL_ROWS min when short on space.
        // ponytail: two-pass (uncapped then capped), no new layout engine.
        let need = if question_active {
            tui.active_question.as_ref().map(|q| super::question::panel_lines(q, w, 100).len() as u16).unwrap_or(PANEL_ROWS as u16)
        } else { PANEL_ROWS as u16 };
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
        let panel_y = if question_active { box_y } else { box_y + box_h };
    let attach_y = panel_y + panel_h;
    let footer_y = attach_y + attach_h + gap_h;

        if let Some((started, label)) = &tui.status && !question_active {
            let label_text = format!(" ⬡ {label}\u{2026}");
            let mut spans = shimmer_spans(&label_text, started.elapsed(), tui.truecolor);
            let elapsed = started.elapsed();
            let elapsed_str = format!("{:.1}s", elapsed.as_secs_f64());
            let tok_suffix = if tui.is_task_running {
                let base = tui.latest_usage.or(tui.cumulative_usage).map(|u| u.total()).unwrap_or(0);
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
            spans.push(Span::styled(suffix, Style::default().fg(Color::Rgb(108, 108, 108))));
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
            frame.render_widget(Paragraph::new(line.clone()), Rect::new(area.x, y, area.width, 1));
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
            frame.render_widget(Paragraph::new(ibox.lines).block(box_block), Rect::new(area.x, box_y, area.width, rendered_box_h));
        }

        if let Some(qlines) = &question_lines && visible_count > 0 {
            for (i, line) in qlines.iter().enumerate().take(visible_count) {
                let item_y = panel_y + i as u16;
                if item_y < area.y || item_y >= area.y + area.height {
                    continue;
                }
                frame.render_widget(
                    Paragraph::new(line.clone()).block(Block::default().style(Style::default().bg(Color::Rgb(22, 22, 22)))),
                    Rect::new(area.x, item_y, area.width, 1),
                );
            }
        } else if visible_count > 0 {
            let start = tui.sel.saturating_sub(visible_count.saturating_sub(1)).min(tui.sel);
            for (i, (name, desc)) in tui.matches.iter().enumerate().skip(start).take(visible_count) {
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
                        Span::styled(cmd_str, Style::default().fg(Color::Black).bg(line_bg).add_modifier(Modifier::BOLD)),
                        Span::styled(desc_str, Style::default().fg(Color::Rgb(40, 40, 40)).bg(line_bg)),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(line_bg)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(cmd_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(line_bg)),
                        Span::styled(desc_str, Style::default().fg(Color::Rgb(140, 140, 140)).bg(line_bg)),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(line_bg)),
                    ])
                };
                frame.render_widget(
                    Paragraph::new(line).block(Block::default().style(Style::default().bg(line_bg))),
                    Rect::new(area.x, item_y, area.width, 1),
                );
            }
        }

        if attach_h > 0 && attach_y < area.y + area.height {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (_ph, p) in &tui.attachments {
                let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("clipboard").to_string();
                // blue File badge like opencode
                spans.push(Span::styled(
                    " File ",
                    Style::default().fg(Color::White).bg(Color::Rgb(59, 130, 246)).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    fname,
                    Style::default().fg(Color::Rgb(180, 180, 180)).bg(Color::Rgb(38, 38, 38)),
                ));
                spans.push(Span::raw("  "));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), Rect::new(area.x, attach_y, area.width, 1));
        }

        if gap_h > 0 {
        let gap_y = attach_y + attach_h;
        if gap_y >= area.y && gap_y < area.y + area.height {
            frame.render_widget(Paragraph::new(Line::from("")), Rect::new(area.x, gap_y, area.width, 1));
        }
    }

    let (_, max_label) = crate::setup::model_context_info(&tui.model_name);
        let (used_tokens, hit_rate) = if let Some(u) = tui.latest_usage.or(tui.cumulative_usage) {
            (u.total(), u.cache_hit_rate() * 100.0)
        } else {
            (0, 0.0)
        };
        let ctx_display = format!("{}/{}", crate::setup::format_context_length(used_tokens), max_label);
        let cache_display = format!("{hit_rate:.1}% cache");

        let model_display = crate::setup::friendly_model_name(&tui.model_name);
        let effort_display = if tui.hide_thinking {
            if tui.thinking_effort.is_empty() { "hidden".to_string() } else { format!("{} · hidden", tui.thinking_effort) }
        } else {
            tui.thinking_effort.clone()
        };
        let right_parts = if model_display.is_empty() {
            vec![Span::styled(effort_display.clone(), Style::default().fg(Color::Rgb(108, 108, 108)))]
        } else {
            vec![
                Span::styled(model_display.clone(), Style::default().fg(Color::Rgb(140, 140, 140))),
                Span::styled(" \u{b7} ", Style::default().fg(Color::Rgb(80, 80, 80))),
                Span::styled(effort_display.clone(), Style::default().fg(Color::Rgb(108, 108, 108))),
            ]
        };
        // Cron ticking clock — hermes-style next due countdown, ticks via tick_status
        let cron_display: Option<(String, Color)> = tui.next_cron.as_ref().and_then(|(name, next)| {
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
        let cron_len = cron_display.as_ref().map(|(s,_)| s.chars().count() + 3).unwrap_or(0);
        let right_len = if model_display.is_empty() { effort_display.chars().count() } else { model_display.chars().count() + 3 + effort_display.chars().count() };
        let left_len = 1 + ctx_display.chars().count() + 3 + cache_display.chars().count() + cron_len;
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
            footer_spans.push(Span::styled(" \u{b7} ", Style::default().fg(Color::Rgb(65, 65, 65))));
            footer_spans.push(Span::styled(cron_str, Style::default().fg(cron_color).add_modifier(Modifier::BOLD)));
        }
        footer_spans.push(Span::raw(" ".repeat(pad_len)));
        footer_spans.extend(right_parts);
        if footer_y < area.y + area.height {
            frame.render_widget(Paragraph::new(Line::from(footer_spans)), Rect::new(area.x, footer_y, area.width, 1));
        }

        let used_bottom = footer_y + 1;
        if used_bottom < area.y + area.height {
            frame.render_widget(
                ratatui::widgets::Clear,
                Rect::new(area.x, used_bottom, area.width, (area.y + area.height) - used_bottom),
            );
        }

        if tui.status.is_none() && !tui.is_task_running {
            let cur_x = (area.x + 3 + ibox.cur_col as u16).min(area.x + area.width.saturating_sub(1));
            let cur_y = (box_y + 1 + ibox.cur_row as u16).min(area.y + area.height.saturating_sub(1));
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
        assert_eq!(status_dock_h(true, true, true), 0);   // question owns the viewport
        assert_eq!(status_dock_h(true, false, false), 2); // scrollback already blank: status + breath
        assert_eq!(status_dock_h(true, false, true), 3);  // seam + status + breath
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
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Queued follow-up inputs (2)"), "got: {text}");
        assert!(text.contains("↳ hello"), "got: {text}");
        assert!(text.contains("↳ second"), "got: {text}");
    }
}
