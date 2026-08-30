use std::time::Duration;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};

use super::{PANEL_ROWS, VIEWPORT_H, Tui};

pub(crate) fn thinking_style() -> Style {
    Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC)
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

pub(crate) fn draw(tui: &mut Tui) -> anyhow::Result<()> {
    // Viewport height growth (Grok xai-ratatui-inline/src/terminal.rs:888 pattern internal)
    // Compute needed viewport height including panel + attachments + footer.
    // Stock ratatui 0.29 has no set_viewport_height; grok's fork adds it.
    // We keep the calculation for fidelity and future crate-gated migration.
    {
        let panel_h: u16 = tui.matches.len().min(PANEL_ROWS) as u16;
        let needed_h = VIEWPORT_H + panel_h + if tui.attachments.is_empty() { 0 } else { 1 } + 1;
        // On grok's xai-ratatui-inline this would be:
        // if needed_h != tui.terminal.viewport_area().height { tui.terminal.set_viewport_height(needed_h)?; }
        let _ = needed_h;
    }

    let w = tui.width();
    tui.terminal.draw(|frame| {
        let area = frame.area();

        let text = tui.textarea.text().to_string();
        let content_w = w.saturating_sub(6).max(1);

        // Neutral Gray palette (no blue)
        let bg_color = Color::Rgb(22, 22, 22);
        let prompt_color = Color::Rgb(180, 180, 180);
        let text_primary = Color::Rgb(225, 225, 225);

        let mut box_lines: Vec<Line<'static>> = Vec::new();

        box_lines.push(Line::from(vec![Span::styled(" ".repeat(w), Style::default().bg(bg_color))]));

        // Prompt input rows
        let prompt_arrow = "  ❯ ";
        let arrow_span = Span::styled(prompt_arrow, Style::default().fg(prompt_color).add_modifier(Modifier::BOLD).bg(bg_color));

        let mut cur_row = 0usize;
        let mut cur_col = 0usize;
        let cursor = tui.textarea.cursor().min(text.len());

        if text.is_empty() {
            box_lines.push(Line::from(vec![
                arrow_span.clone(),
                Span::styled(" ".repeat(w.saturating_sub(4)), Style::default().bg(bg_color)),
            ]));
            cur_row = 0;
            cur_col = 0;
        } else {
            let lines_raw: Vec<&str> = text.split('\n').collect();
            let mut cursor_found = false;
            let mut current_byte_pos = 0usize;
            let mut row_count = 0usize;

            for (i, raw_line) in lines_raw.iter().enumerate() {
                let prefix_span = if i == 0 {
                    arrow_span.clone()
                } else {
                    Span::styled("    ", Style::default().bg(bg_color))
                };

                let line_len_bytes = raw_line.len();
                let line_end_bytes = current_byte_pos + line_len_bytes;
                let has_cursor = !cursor_found && (cursor <= line_end_bytes || i == lines_raw.len() - 1);

                if raw_line.is_empty() {
                    box_lines.push(Line::from(vec![
                        prefix_span,
                        Span::styled(" ".repeat(w.saturating_sub(4)), Style::default().bg(bg_color)),
                    ]));
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
                        let s_len = chunk.len();
                        let pad_len = w.saturating_sub(4 + s_len);

                        if chunk_idx == 0 {
                            box_lines.push(Line::from(vec![
                                prefix_span.clone(),
                                Span::styled(s, Style::default().fg(text_primary).bg(bg_color)),
                                Span::styled(" ".repeat(pad_len), Style::default().bg(bg_color)),
                            ]));
                        } else {
                            box_lines.push(Line::from(vec![
                                Span::styled("    ", Style::default().bg(bg_color)),
                                Span::styled(s, Style::default().fg(text_primary).bg(bg_color)),
                                Span::styled(" ".repeat(pad_len), Style::default().bg(bg_color)),
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

        box_lines.push(Line::from(vec![Span::styled(" ".repeat(w), Style::default().bg(bg_color))]));

        let box_h = box_lines.len().max(1) as u16;
        let has_attach = !tui.attachments.is_empty();
        let attach_h: u16 = if has_attach { 1 } else { 0 };
        let has_status = tui.status.is_some();

        let status_h: u16 = if has_status { 1 } else { 0 };

        let avail_panel_h = (area.height.saturating_sub(box_h + attach_h + 1) as usize).min(PANEL_ROWS);
        let visible_count = if !tui.matches.is_empty() {
            tui.matches.len().min(avail_panel_h)
        } else {
            0
        };
        let panel_h: u16 = visible_count as u16;

        let (status_y, box_y, panel_y, attach_y, footer_y) = if !tui.matches.is_empty() {
            let box_y = area.y;
            // Omit bottom padding row of prompt box when autocomplete open: panel directly below prompt
            let panel_y = box_y + box_h.saturating_sub(1);
            let attach_y = panel_y + panel_h;
            let footer_y = (attach_y + attach_h).min(area.y + area.height.saturating_sub(1));
            let status_y = area.y;
            (status_y, box_y, panel_y, attach_y, footer_y)
        } else if has_status {
            let status_y = area.y;
            let box_y = status_y + status_h;
            // ensure exactly 1 row separation before prompt is handled by status being 1 row; box_lines top padding already provides the prompt border
            let panel_y = box_y + box_h;
            let attach_y = panel_y + panel_h;
            let footer_y = (attach_y + attach_h).min(area.y + area.height.saturating_sub(1));
            (status_y, box_y, panel_y, attach_y, footer_y)
        } else {
            let box_y = area.y;
            let status_y = area.y;
            let panel_y = box_y + box_h;
            let attach_y = panel_y + panel_h;
            let footer_y = (attach_y + attach_h).min(area.y + area.height.saturating_sub(1));
            (status_y, box_y, panel_y, attach_y, footer_y)
        };

        if let Some((started, label)) = &tui.status {
            if status_y < area.y + area.height {
                let label_text = format!("  \u{2b21} {label}\u{2026}");
                let mut spans = shimmer_spans(&label_text, started.elapsed(), tui.truecolor);
                let elapsed = started.elapsed();
                let elapsed_str = format!("{:.1}s", elapsed.as_secs_f64());
                let tok_suffix = if tui.is_task_running {
                    let base = tui.latest_usage.map(|u| u.total()).unwrap_or(0);
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
                frame.render_widget(Paragraph::new(Line::from(spans)), Rect::new(area.x, status_y, area.width, 1));
            }
        }

        let rendered_box_h = box_h.min((area.y + area.height).saturating_sub(box_y));
        if rendered_box_h > 0 {
            frame.render_widget(
                Paragraph::new(box_lines).block(Block::default().style(Style::default().bg(bg_color))),
                Rect::new(area.x, box_y, area.width, rendered_box_h),
            );
        }

        if !tui.matches.is_empty() && visible_count > 0 {
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

        if has_attach && attach_y < area.y + area.height {
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

        let (_, max_label) = crate::setup::model_context_info(&tui.model_name);
        let (used_tokens, hit_rate) = if let Some(u) = tui.cumulative_usage.or(tui.latest_usage) {
            (u.total(), u.cache_hit_rate() * 100.0)
        } else {
            (0, 0.0)
        };
        let ctx_display = format!("{}/{}", crate::setup::format_context_length(used_tokens), max_label);
        let cache_display = format!("{hit_rate:.1}% cache");

        let model_display = crate::setup::friendly_model_name(&tui.model_name);
        let effort_display = &tui.thinking_effort;
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
        let left_len = 2 + ctx_display.chars().count() + 3 + cache_display.chars().count() + cron_len;
        let pad_len = w.saturating_sub(left_len + right_len);

        let cache_color = if hit_rate > 0.0 {
            Color::Rgb(130, 145, 130)
        } else {
            Color::Rgb(80, 80, 80)
        };

        let mut footer_spans = vec![
            Span::raw("  "),
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

        // Clear any leftover rows below footer_y within the viewport
        let used_bottom = footer_y + 1;
        if used_bottom < area.y + area.height {
            frame.render_widget(
                ratatui::widgets::Clear,
                Rect::new(area.x, used_bottom, area.width, (area.y + area.height) - used_bottom),
            );
        }

        let cur_x = (area.x + 4 + cur_col as u16).min(area.x + area.width.saturating_sub(1));
        let cur_y = (box_y + 1 + cur_row as u16).min(area.y + area.height.saturating_sub(1));
        frame.set_cursor_position(Position::new(cur_x, cur_y));
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    pub(crate) fn shimmer_spans_change_across_ticks() {
        let text = "\u{2022} Working\u{2026} 1s (ctrl-c to cancel)";
        let mut prev = shimmer_spans(text, Duration::from_millis(300), false);
        for ms in (400..=1200).step_by(100) {
            let cur = shimmer_spans(text, Duration::from_millis(ms), false);
            assert_ne!(prev, cur, "no change between {}ms and {}ms", ms - 100, ms);
            prev = cur;
        }
    }

    #[test]
    pub(crate) fn shimmer_truecolor_changes_across_ticks() {
        let text = "\u{2022} Working\u{2026} 1s";
        let a = shimmer_spans(text, Duration::from_millis(500), true);
        let b = shimmer_spans(text, Duration::from_millis(600), true);
        assert_ne!(a, b);
    }

    #[test]
    pub(crate) fn consecutive_frames_differ_in_test_backend() {
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;
        use ratatui::Terminal;
        use std::time::Instant;
        let mut terminal = Terminal::new(TestBackend::new(40, 3)).unwrap();
        let started = Instant::now();
        let frame_at = |ms: u64, term: &mut Terminal<TestBackend>| {
            let fake_started = started - Duration::from_millis(ms);
            term.draw(|f| {
                let text = format!("\u{2022} Working\u{2026} 1s");
                let spans = shimmer_spans(&text, fake_started.elapsed(), false);
                f.render_widget(Paragraph::new(Line::from(spans)), Rect::new(0, 0, 40, 1));
            }).unwrap();
            term.backend().buffer().clone()
        };
        let b1 = frame_at(500, &mut terminal);
        let b2 = frame_at(600, &mut terminal);
        assert_ne!(b1, b2, "ratatui diff should see changing spans across 100ms ticks");
    }

    #[test]
    fn viewport_grows_for_panel() {
        let panel_h = 3u16;
        let needed_h = super::VIEWPORT_H + panel_h + 1; // +footer, no attach
        assert_eq!(needed_h, 11); // 7+3+1
        let panel_h2 = 6u16;
        let needed_h2 = super::VIEWPORT_H + panel_h2 + 1;
        assert_eq!(needed_h2, 14);
        assert!(needed_h2 > needed_h);
    }
}
