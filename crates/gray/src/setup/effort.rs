//! Thinking-effort picker modal (split from `setup`).

use super::*;

/// Interactive "Thinking effort" GUI modal matching Pi / Prime-Agent.
pub fn run_effort_modal(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use crossterm::terminal::EnterAlternateScreen;
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    let _session = TuiSession::acquire()?;
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(
        stdout_handle,
        EnterAlternateScreen,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::Hide
    )?;
    let _ = crossterm::terminal::size();

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = crate::theme::theme().surface_bg;
    let accent_peach = crate::theme::theme().accent;
    let text_dim = crate::theme::theme().text_dim;

    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    // Provider-driven levels (opencode parity): only offer efforts the
    // current model's family accepts; non-reasoning models get just `off`.
    // `supported_thinking_levels` never returns an empty list.
    let levels: Vec<(&str, &str)> =
        super::context::supported_thinking_levels(&bg_snapshot.model_name);

    let current_level = config
        .thinking_effort
        .clone()
        .unwrap_or_else(|| "high".to_string());
    let mut sel = levels
        .iter()
        .position(|(l, _)| *l == current_level)
        .unwrap_or(levels.len().saturating_sub(1));
    // Trailing rows: a blank separator, then the reasoning-text display
    // toggle (not an effort level). The gap groups the levels so the toggle
    // reads as a separate action.
    let gap_idx = levels.len();
    let rows = levels.len() + 2;
    let max_sel = rows.saturating_sub(1);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

                render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 58.min(area.width.saturating_sub(4)).max(36).min(area.width);
                let modal_h = (rows as u16 + 5)
                    .min(area.height.saturating_sub(2))
                    .max(10)
                    .min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                frame.render_widget(Clear, modal_rect);

                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);

                let pad_x = 4u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(
                    modal_x + pad_x,
                    modal_y + 1,
                    inner_w,
                    modal_h.saturating_sub(2),
                );

                // Header
                let title_str = "Thinking effort";
                let esc_str = "esc";
                let pad_len = (inner.width as usize)
                    .saturating_sub(title_str.chars().count() + esc_str.chars().count());
                let header_line = Line::from(vec![
                    Span::styled(
                        title_str,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(
                    Paragraph::new(header_line),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );

                // List of levels + gap + display toggle
                let list_y = inner.y + 2;
                // rows = len+2: `gap_idx` renders a blank separator so the
                // display toggle reads as a separate action; the last
                // iteration renders the display-toggle row.
                #[allow(clippy::needless_range_loop)]
                for idx in 0..rows {
                    if idx == gap_idx {
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(
                                " ".repeat(inner.width as usize),
                                Style::default().bg(box_bg),
                            ))),
                            Rect::new(inner.x, list_y + idx as u16, inner.width, 1),
                        );
                        continue;
                    }
                    let (level, desc, is_current) = if idx < levels.len() {
                        let (l, d) = levels[idx];
                        (l, d, current_level == l)
                    } else {
                        let shown = config.show_reasoning.unwrap_or(true);
                        (
                            "display",
                            if shown {
                                "Reasoning text shown"
                            } else {
                                "Reasoning text hidden"
                            },
                            shown,
                        )
                    };
                    let is_selected = idx == sel;

                    let check_glyph = if is_current { "✓ " } else { "  " };
                    let raw_content = format!(" {check_glyph}{level:<8}  {desc}");
                    let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                    let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

                    let row_line = if is_selected {
                        Line::from(Span::styled(
                            full_row_str,
                            Style::default()
                                .fg(crate::theme::theme().on_selection)
                                .bg(accent_peach)
                                .add_modifier(Modifier::BOLD),
                        ))
                    } else {
                        let check_span = if is_current {
                            Span::styled(
                                " ✓ ",
                                Style::default()
                                    .fg(crate::theme::theme().success)
                                    .add_modifier(Modifier::BOLD)
                                    .bg(box_bg),
                            )
                        } else {
                            Span::styled("   ", Style::default().bg(box_bg))
                        };
                        let name_span = Span::styled(
                            format!("{level:<8}  "),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        );
                        let desc_span = Span::styled(
                            desc,
                            Style::default()
                                .fg(crate::theme::theme().text_muted)
                                .bg(box_bg),
                        );
                        let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                        Line::from(vec![check_span, name_span, desc_span, pad_span])
                    };

                    frame.render_widget(
                        Paragraph::new(row_line),
                        Rect::new(inner.x, list_y + idx as u16, inner.width, 1),
                    );
                }

                // Footer
                let footer_line = Line::from(vec![
                    Span::styled(
                        "↑↓ ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled(
                        "enter ",
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(
                    Paragraph::new(footer_line),
                    Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
                );
            })?;

            if !poll(Duration::from_millis(100))? {
                continue;
            }

            match read()? {
                Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind: KeyEventKind::Press,
                    ..
                }) if modifiers.contains(KeyModifiers::CONTROL) => match code {
                    KeyCode::Char('p') => {
                        sel = sel.saturating_sub(1);
                        if sel == gap_idx {
                            sel = sel.saturating_sub(1);
                        }
                    }
                    KeyCode::Char('n') => {
                        sel = (sel + 1).min(max_sel);
                        if sel == gap_idx {
                            sel = (sel + 1).min(max_sel);
                        }
                    }
                    _ => {}
                },
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
                    KeyCode::Up => {
                        sel = sel.saturating_sub(1);
                        if sel == gap_idx {
                            sel = sel.saturating_sub(1);
                        }
                    }
                    KeyCode::Down => {
                        sel = (sel + 1).min(max_sel);
                        if sel == gap_idx {
                            sel = (sel + 1).min(max_sel);
                        }
                    }
                    KeyCode::Esc => return Ok(false),
                    KeyCode::Enter => {
                        if sel == gap_idx {
                            continue;
                        }
                        if sel == max_sel {
                            // Display toggle: flip, persist, stay open.
                            let shown = !config.show_reasoning.unwrap_or(true);
                            config.show_reasoning = Some(shown);
                            let path = saved_config_path()?;
                            let mut saved = load_saved_config_at(&path);
                            saved.show_reasoning = Some(shown);
                            save_saved_config_at(&path, &saved)?;
                            continue;
                        }
                        let (chosen, _) = levels[sel];
                        config.thinking_effort = Some(chosen.to_string());

                        let path = saved_config_path()?;
                        let mut saved = load_saved_config_at(&path);
                        saved.thinking_effort = config.thinking_effort.clone();
                        save_saved_config_at(&path, &saved)?;

                        return Ok(true);
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    })();

    let _ = terminal.clear();

    result
}
