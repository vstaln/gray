//! Provider picker modal (split from `setup`).

use super::*;

pub fn run_model_modal(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::io::Write as _;
    use std::time::Duration;

    let catalog = load_catalog().unwrap_or_default();

    let (item_id, item_name) =
        if let Some((pid, p)) = catalog.iter().find(|(_, p)| p.base_url == config.base_url) {
            (pid.clone(), p.name.clone())
        } else {
            ("custom".to_string(), "Custom".to_string())
        };

    let item = ConnectItem {
        id: item_id.clone(),
        name: item_name.clone(),
        sublabel: String::new(),
        base_url: config.base_url.clone(),
        category: "Providers",
        env_key: String::new(),
        no_auth: false,
        oauth_capable: crate::setup::catalog::OAUTH_CAPABLE.contains(&item_id.as_str()),
    };

    let models = get_provider_models_with_live(
        &item_id,
        &config.base_url,
        config.api_key.as_deref(),
        &catalog,
    );

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
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

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;

    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            let filtered_models: Vec<&(String, String)> = models
                .iter()
                .filter(|(m_id, m_name)| {
                    let f = filter.to_lowercase();
                    f.is_empty()
                        || m_id.to_lowercase().contains(&f)
                        || m_name.to_lowercase().contains(&f)
                })
                .collect();

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

                render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 68.min(area.width.saturating_sub(4)).max(42).min(area.width);
                let modal_h = 16
                    .min(area.height.saturating_sub(2))
                    .max(10)
                    .min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                frame.render_widget(Clear, modal_rect);

                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);

                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(
                    modal_x + pad_x,
                    modal_y + 1,
                    inner_w,
                    modal_h.saturating_sub(2),
                );

                // Header
                let title_str = format!("Select model \u{2014} {}", item.name);
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

                // Search Bar
                let search_line = if filter.is_empty() {
                    Line::from(vec![
                        Span::styled(
                            "Search: ",
                            Style::default()
                                .fg(accent_peach)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(
                            "Type to filter models...",
                            Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg),
                        ),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(
                            "Search: ",
                            Style::default()
                                .fg(accent_peach)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled(
                            &filter,
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD)
                                .bg(box_bg),
                        ),
                        Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                    ])
                };
                frame.render_widget(
                    Paragraph::new(search_line),
                    Rect::new(inner.x, inner.y + 1, inner.width, 1),
                );

                // List
                let list_y = inner.y + 3;
                let list_h = inner.height.saturating_sub(4) as usize;

                if filtered_models.is_empty() {
                    let empty_msg = if filter.is_empty() {
                        Paragraph::new(Line::from(vec![Span::styled(
                            "  No models listed — press Enter to continue",
                            Style::default().fg(text_dim).bg(box_bg),
                        )]))
                    } else {
                        Paragraph::new(Line::from(vec![
                            Span::styled(
                                "  Use custom model: ",
                                Style::default().fg(text_dim).bg(box_bg),
                            ),
                            Span::styled(
                                &filter,
                                Style::default()
                                    .fg(accent_peach)
                                    .add_modifier(Modifier::BOLD)
                                    .bg(box_bg),
                            ),
                        ]))
                    };
                    frame.render_widget(empty_msg, Rect::new(inner.x, list_y + 1, inner.width, 1));
                } else {
                    let safe_sel = sel.min(filtered_models.len().saturating_sub(1));
                    if safe_sel < scroll_top {
                        scroll_top = safe_sel;
                    } else if safe_sel >= scroll_top + list_h {
                        scroll_top = safe_sel.saturating_sub(list_h.saturating_sub(1));
                    }

                    for r in 0..list_h {
                        let idx = scroll_top + r;
                        if idx >= filtered_models.len() {
                            break;
                        }

                        let (m_id, m_name) = filtered_models[idx];
                        let is_selected = idx == safe_sel;
                        let is_current = config.model.as_deref() == Some(m_id.as_str());

                        let check_glyph = if is_current { "✓ " } else { "  " };

                        let display_name = if m_name.is_empty() {
                            m_id.as_str()
                        } else {
                            m_name.as_str()
                        };
                        let sub = if m_name.is_empty() || m_name == m_id {
                            String::new()
                        } else {
                            format!(" {}", m_id)
                        };

                        let raw_content = format!(" {check_glyph}{}{sub}", display_name);
                        let fill =
                            (inner.width as usize).saturating_sub(raw_content.chars().count());
                        let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

                        let row_line = if is_selected {
                            Line::from(Span::styled(
                                full_row_str,
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(accent_peach)
                                    .add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            let check_span = if is_current {
                                Span::styled(
                                    " ✓ ",
                                    Style::default()
                                        .fg(Color::Rgb(74, 222, 128))
                                        .add_modifier(Modifier::BOLD)
                                        .bg(box_bg),
                                )
                            } else {
                                Span::styled("   ", Style::default().bg(box_bg))
                            };
                            let name_span = Span::styled(
                                display_name,
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD)
                                    .bg(box_bg),
                            );
                            let sub_span = Span::styled(
                                sub,
                                Style::default().fg(Color::Rgb(130, 130, 130)).bg(box_bg),
                            );
                            let pad_span =
                                Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                            Line::from(vec![check_span, name_span, sub_span, pad_span])
                        };

                        frame.render_widget(
                            Paragraph::new(row_line),
                            Rect::new(inner.x, list_y + r as u16, inner.width, 1),
                        );
                    }
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

            if filtered_models.is_empty() {
                sel = 0;
            } else if sel >= filtered_models.len() {
                sel = filtered_models.len().saturating_sub(1);
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
                    KeyCode::Char('p') => sel = sel.saturating_sub(1),
                    KeyCode::Char('n') if !filtered_models.is_empty() => {
                        sel = (sel + 1).min(filtered_models.len() - 1);
                    }
                    _ => {}
                },
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => {
                        if !filtered_models.is_empty() {
                            sel = (sel + 1).min(filtered_models.len() - 1);
                        }
                    }
                    KeyCode::PageUp => sel = sel.saturating_sub(8),
                    KeyCode::PageDown => {
                        if !filtered_models.is_empty() {
                            sel = (sel + 8).min(filtered_models.len() - 1);
                        }
                    }
                    KeyCode::Char(ch) => {
                        filter.push(ch);
                        sel = 0;
                    }
                    KeyCode::Backspace => {
                        filter.pop();
                        sel = 0;
                    }
                    KeyCode::Esc => return Ok(false),
                    KeyCode::Enter => {
                        let chosen_model = if let Some(&(m_id, _)) = filtered_models.get(sel) {
                            m_id.clone()
                        } else if !filter.is_empty() {
                            filter.trim().to_string()
                        } else {
                            "default".to_string()
                        };

                        config.model = Some(chosen_model.clone());

                        let path = saved_config_path()?;
                        let mut saved = load_saved_config_at(&path);
                        saved.model = config.model.clone();
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
    let _ = crossterm::execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        crossterm::cursor::Show,
    );
    if !was_raw {
        let _ = disable_raw_mode();
    } else {
        let _ = enable_raw_mode();
    }
    let _ = std::io::stdout().flush();

    result
}
