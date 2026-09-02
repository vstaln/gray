pub mod catalog;
pub use catalog::{
    build_connect_items, gray_home, load_catalog, load_saved_config_at, mask_key_pretty,
    save_saved_config_at, saved_config_path, Catalog, CatalogModel, CatalogProvider,
    ConnectItem, SavedConfig, PROVIDERS_JSON,
};
pub(crate) use catalog::{load_auth_keys, save_auth_key};

pub mod context;
pub use context::{
    cache_model_context, extract_context_length_from_json, fetch_live_provider_models,
    format_context_length, friendly_model_name, get_cached_model_context, get_provider_models,
    get_provider_models_with_live, get_user_context_window, model_context_info,
    parse_context_window, resolve_model_context_length, set_user_context_window,
};

pub mod ui;
pub use ui::{BackgroundSnapshot, dim_color, dim_line, dim_style, render_dimmed_background};

use crate::{config::Config, tui::print_wrapped};


/// Interactive "Connect a provider" GUI modal with clean colored box styling.
/// Floating container block matching the composer prompt text box, live search filter,
/// peach selection highlight, and in-modal API key entry.
pub fn run_connect_modal(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use ratatui::Terminal;
    use std::io::Write as _;
    use std::time::Duration;

    let catalog = load_catalog()?;
    let all_items = build_connect_items(&catalog);
    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;

    enum ModalState {
        Selecting,
        EnteringKey {
            item: ConnectItem,
            key_buf: String,
            existing_key: Option<String>,
            status_msg: Option<String>,
        },
        SelectingModel {
            item: ConnectItem,
            models: Vec<(String, String)>,
            filter: String,
            sel: usize,
            scroll_top: usize,
        },
    }

    let mut state = ModalState::Selecting;
    let mut connected_name: Option<(String, String)> = None;

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let input_bg = Color::Rgb(32, 32, 32);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            let auth_keys = load_auth_keys();

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

                render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 68.min(area.width.saturating_sub(4)).max(42).min(area.width);
                let modal_h = 18.min(area.height.saturating_sub(2)).max(10).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                match &mut state {
                    ModalState::Selecting => {
                        let filtered: Vec<&ConnectItem> = all_items
                            .iter()
                            .filter(|item| {
                                let f = filter.to_lowercase();
                                f.is_empty()
                                    || item.name.to_lowercase().contains(&f)
                                    || item.id.to_lowercase().contains(&f)
                                    || item.sublabel.to_lowercase().contains(&f)
                            })
                            .collect();

                        // Clear popup background
                        frame.render_widget(Clear, modal_rect);

                        // Container Box (pure colored block matching text box, no border characters)
                        let box_block = Block::default()
                            .style(Style::default().bg(box_bg));
                        frame.render_widget(box_block, modal_rect);

                        let pad_x = 3u16;
                        let inner_w = modal_w.saturating_sub(pad_x * 2);
                        let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));

                        // 1. Header Line (with esc at top right)
                        let title_str = "Connect a provider";
                        let esc_str = "esc";
                        let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                        let header_line = Line::from(vec![
                            Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                            Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));

                        // 2. Search Bar
                        let search_line = if filter.is_empty() {
                            Line::from(vec![
                                Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled("Type to filter providers...", Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg)),
                            ])
                        } else {
                            Line::from(vec![
                                Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled(&filter, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                            ])
                        };
                        frame.render_widget(Paragraph::new(search_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));

                        // 3. Provider List
                        let list_y = inner.y + 3;
                        let list_h = inner.height.saturating_sub(4) as usize;

                        if filtered.is_empty() {
                            let empty_msg = Paragraph::new(Line::from(vec![
                                Span::styled("  No matching providers found", Style::default().fg(text_dim).bg(box_bg)),
                            ]));
                            frame.render_widget(empty_msg, Rect::new(inner.x, list_y + 1, inner.width, 1));
                        } else {
                            let safe_sel = sel.min(filtered.len().saturating_sub(1));
                            if safe_sel < scroll_top {
                                scroll_top = safe_sel;
                            } else if safe_sel >= scroll_top + list_h {
                                scroll_top = safe_sel.saturating_sub(list_h.saturating_sub(1));
                            }

                            for r in 0..list_h {
                                let idx = scroll_top + r;
                                if idx >= filtered.len() {
                                    break;
                                }

                                let item = filtered[idx];
                                let is_selected = idx == safe_sel;

                                let is_connected = auth_keys.contains_key(&item.id)
                                    || (config.base_url == item.base_url && config.api_key.is_some());

                                let check_glyph = if is_connected { "✓ " } else { "  " };

                                let sub = if item.sublabel.is_empty() {
                                    String::new()
                                } else {
                                    format!(" {}", item.sublabel)
                                };

                                let raw_content = format!(" {check_glyph}{}{sub}", item.name);
                                let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                                let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

                                let row_line = if is_selected {
                                    Line::from(Span::styled(
                                        full_row_str,
                                        Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD),
                                    ))
                                } else {
                                    let check_span = if is_connected {
                                        Span::styled(" ✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD).bg(box_bg))
                                    } else {
                                        Span::styled("   ", Style::default().bg(box_bg))
                                    };
                                    let name_span = Span::styled(&item.name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                                    let sub_span = Span::styled(sub, Style::default().fg(Color::Rgb(130, 130, 130)).bg(box_bg));
                                    let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                                    Line::from(vec![check_span, name_span, sub_span, pad_span])
                                };

                                frame.render_widget(
                                    Paragraph::new(row_line),
                                    Rect::new(inner.x, list_y + r as u16, inner.width, 1),
                                );
                            }
                        }

                        // 4. Footer Help Line (no brackets)
                        let footer_line = Line::from(vec![
                            Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                            Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(
                            Paragraph::new(footer_line),
                            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
                        );
                    }
                    ModalState::EnteringKey {
                        item,
                        key_buf,
                        existing_key,
                        status_msg,
                    } => {
                        let dialog_w = 64.min(area.width.saturating_sub(4)).max(40).min(area.width);
                        let dialog_h = 10.min(area.height.saturating_sub(2)).max(8).min(area.height);
                        let dialog_x = (area.width.saturating_sub(dialog_w)) / 2;
                        let dialog_y = (area.height.saturating_sub(dialog_h)) / 3;
                        let dialog_rect = Rect::new(dialog_x, dialog_y, dialog_w, dialog_h);

                        frame.render_widget(Clear, dialog_rect);

                        let box_block = Block::default()
                            .style(Style::default().bg(box_bg));
                        frame.render_widget(box_block, dialog_rect);

                        let pad_x = 3u16;
                        let inner_w = dialog_w.saturating_sub(pad_x * 2);
                        let inner = Rect::new(dialog_x + pad_x, dialog_y + 1, inner_w, dialog_h.saturating_sub(2));

                        // Header (with esc at top right)
                        let title_str = "API Key Configuration";
                        let esc_str = "esc";
                        let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                        let line0 = Line::from(vec![
                            Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                            Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        let line1 = Line::from(vec![
                            Span::styled(format!("Provider: {}", item.name), Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(Paragraph::new(line0), Rect::new(inner.x, inner.y, inner.width, 1));
                        frame.render_widget(Paragraph::new(line1), Rect::new(inner.x, inner.y + 1, inner.width, 1));

                        // Input Box (inset colored block)
                        let input_content = if key_buf.is_empty() {
                            if let Some(existing) = existing_key.as_ref() {
                                let masked = mask_key_pretty(existing);
                                Line::from(vec![
                                    Span::styled(
                                        format!(" {masked}"),
                                        Style::default().fg(Color::Rgb(210, 210, 210)).bg(input_bg),
                                    ),
                                    Span::styled(
                                        "  \u{00b7} Enter to keep, paste to replace",
                                        Style::default().fg(Color::Rgb(110, 110, 110)).bg(input_bg),
                                    ),
                                ])
                            } else {
                                Line::from(vec![
                                    Span::styled(" Paste or type API key...", Style::default().fg(Color::Rgb(110, 110, 110)).bg(input_bg)),
                                ])
                            }
                        } else {
                            let masked = "•".repeat(key_buf.chars().count());
                            Line::from(vec![
                                Span::styled(format!(" {masked}"), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(input_bg)),
                            ])
                        };

                        let input_rect = Rect::new(inner.x, inner.y + 3, inner.width, 1);
                        frame.render_widget(Clear, input_rect);
                        frame.render_widget(Paragraph::new(input_content).style(Style::default().bg(input_bg)), input_rect);

                        // Status or note
                        let note_line = if let Some(msg) = status_msg {
                            Line::from(Span::styled(format!(" \u{2022} {msg}"), Style::default().fg(Color::Rgb(239, 68, 68)).bg(box_bg)))
                        } else {
                            Line::from(Span::styled(" (Key stored securely in ~/.gray/auth.json)", Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg)))
                        };
                        frame.render_widget(Paragraph::new(note_line), Rect::new(inner.x, inner.y + 5, inner.width, 1));

                        // Footer buttons (enter update / submit - no brackets)
                        let action_label = if existing_key.is_some() { "update" } else { "submit" };
                        let footer = Line::from(vec![
                            Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled(action_label, Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(Paragraph::new(footer), Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1));
                    }
                    ModalState::SelectingModel {
                        item,
                        models,
                        filter: m_filter,
                        sel: m_sel,
                        scroll_top: m_scroll_top,
                    } => {
                        let filtered_models: Vec<&(String, String)> = models
                            .iter()
                            .filter(|(m_id, m_name)| {
                                let f = m_filter.to_lowercase();
                                f.is_empty() || m_id.to_lowercase().contains(&f) || m_name.to_lowercase().contains(&f)
                            })
                            .collect();

                        let modal_w = 68.min(area.width.saturating_sub(4)).max(42).min(area.width);
                        let modal_h = 20.min(area.height.saturating_sub(2)).max(12).min(area.height);
                        let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                        let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                        let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                        frame.render_widget(Clear, modal_rect);

                        let box_block = Block::default().style(Style::default().bg(box_bg));
                        frame.render_widget(box_block, modal_rect);

                        let pad_x = 3u16;
                        let inner_w = modal_w.saturating_sub(pad_x * 2);
                        let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));

                        // Header: Select Model — Provider ... esc
                        let title_str = format!("Select model \u{2014} {}", item.name);
                        let esc_str = "esc";
                        let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                        let header_line = Line::from(vec![
                            Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                            Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));

                        // Search Bar
                        let search_line = if m_filter.is_empty() {
                            Line::from(vec![
                                Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled("Type to filter models...", Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg)),
                            ])
                        } else {
                            Line::from(vec![
                                Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled(&*m_filter, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                            ])
                        };
                        frame.render_widget(Paragraph::new(search_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));

                        // Model list
                        let list_y = inner.y + 3;
                        let list_h = inner.height.saturating_sub(4) as usize;

                        if filtered_models.is_empty() {
                            let empty_msg = if m_filter.is_empty() {
                                Paragraph::new(Line::from(vec![
                                    Span::styled("  No models listed — press Enter to continue", Style::default().fg(text_dim).bg(box_bg)),
                                ]))
                            } else {
                                Paragraph::new(Line::from(vec![
                                    Span::styled("  Use custom model: ", Style::default().fg(text_dim).bg(box_bg)),
                                    Span::styled(&*m_filter, Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                                ]))
                            };
                            frame.render_widget(empty_msg, Rect::new(inner.x, list_y + 1, inner.width, 1));
                        } else {
                            let safe_sel = (*m_sel).min(filtered_models.len().saturating_sub(1));
                            if safe_sel < *m_scroll_top {
                                *m_scroll_top = safe_sel;
                            } else if safe_sel >= *m_scroll_top + list_h {
                                *m_scroll_top = safe_sel.saturating_sub(list_h.saturating_sub(1));
                            }

                            for r in 0..list_h {
                                let idx = *m_scroll_top + r;
                                if idx >= filtered_models.len() {
                                    break;
                                }

                                let (m_id, m_name) = filtered_models[idx];
                                let is_selected = idx == safe_sel;
                                let is_current = config.model.as_deref() == Some(m_id.as_str());

                                let check_glyph = if is_current { "✓ " } else { "  " };

                                let display_name = if m_name.is_empty() { m_id.as_str() } else { m_name.as_str() };
                                let sub = if m_name.is_empty() || m_name == m_id {
                                    String::new()
                                } else {
                                    format!(" {}", m_id)
                                };

                                let raw_content = format!(" {check_glyph}{}{sub}", display_name);
                                let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                                let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

                                let row_line = if is_selected {
                                    Line::from(Span::styled(
                                        full_row_str,
                                        Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD),
                                    ))
                                } else {
                                    let check_span = if is_current {
                                        Span::styled(" ✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD).bg(box_bg))
                                    } else {
                                        Span::styled("   ", Style::default().bg(box_bg))
                                    };
                                    let name_span = Span::styled(display_name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                                    let sub_span = Span::styled(sub, Style::default().fg(Color::Rgb(130, 130, 130)).bg(box_bg));
                                    let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
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
                            Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                            Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
                        ]);
                        frame.render_widget(
                            Paragraph::new(footer_line),
                            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
                        );
                    }
                }
            })?;

            if !poll(Duration::from_millis(100))? {
                continue;
            }

            match &mut state {
                ModalState::Selecting => {
                    let filtered: Vec<&ConnectItem> = all_items
                        .iter()
                        .filter(|item| {
                            let f = filter.to_lowercase();
                            f.is_empty()
                                || item.name.to_lowercase().contains(&f)
                                || item.id.to_lowercase().contains(&f)
                                || item.sublabel.to_lowercase().contains(&f)
                        })
                        .collect();

                    if filtered.is_empty() {
                        sel = 0;
                    } else if sel >= filtered.len() {
                        sel = filtered.len().saturating_sub(1);
                    }

                    match read()? {
                        Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                            if modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
                        Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                            match code {
                                KeyCode::Char('p') => sel = sel.saturating_sub(1),
                                KeyCode::Char('n') => {
                                    if !filtered.is_empty() {
                                        sel = (sel + 1).min(filtered.len() - 1);
                                    }
                                }
                                _ => {}
                            }
                        }
                        Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                            KeyCode::Up => sel = sel.saturating_sub(1),
                            KeyCode::Down => {
                                if !filtered.is_empty() {
                                    sel = (sel + 1).min(filtered.len() - 1);
                                }
                            }
                            KeyCode::PageUp => sel = sel.saturating_sub(8),
                            KeyCode::PageDown => {
                                if !filtered.is_empty() {
                                    sel = (sel + 8).min(filtered.len() - 1);
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
                                if let Some(&item) = filtered.get(sel) {
                                    if item.no_auth {
                                        config.base_url = item.base_url.clone();
                                        config.api_key = None;
                                        let models = get_provider_models_with_live(&item.id, &item.base_url, None, &catalog);
                                        state = ModalState::SelectingModel {
                                            item: item.clone(),
                                            models,
                                            filter: String::new(),
                                            sel: 0,
                                            scroll_top: 0,
                                        };
                                    } else {
                                        let existing = load_auth_keys()
                                            .get(&item.id)
                                            .cloned()
                                            .or_else(|| if config.base_url == item.base_url { config.api_key.clone() } else { None });
                                        state = ModalState::EnteringKey {
                                            item: item.clone(),
                                            key_buf: String::new(),
                                            existing_key: existing,
                                            status_msg: None,
                                        };
                                    }
                                }
                            }
                            _ => {}
                        },
                        Event::Resize(_, _) => {}
                        _ => {}
                    }
                }
                ModalState::EnteringKey {
                    item,
                    key_buf,
                    existing_key,
                    status_msg,
                } => match read()? {
                    Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                        if modifiers.contains(KeyModifiers::CONTROL) => {
                        state = ModalState::Selecting;
                    }
                    Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                        KeyCode::Esc => {
                            state = ModalState::Selecting;
                        }
                        KeyCode::Char(ch) => {
                            key_buf.push(ch);
                            *status_msg = None;
                        }
                        KeyCode::Backspace => {
                            key_buf.pop();
                            *status_msg = None;
                        }
                        KeyCode::Enter => {
                            let final_key = if key_buf.is_empty() {
                                existing_key.clone().unwrap_or_default()
                            } else {
                                key_buf.clone()
                            };

                            if final_key.is_empty() {
                                *status_msg = Some("No API key entered — please enter a valid key".into());
                            } else if existing_key.is_some() {
                                save_auth_key(&item.id, &final_key)?;
                                let path = saved_config_path()?;
                                let mut saved = load_saved_config_at(&path);
                                let is_switching = saved.base_url.as_deref() != Some(item.base_url.as_str());
                                config.base_url = item.base_url.clone();
                                config.api_key = Some(final_key.clone());
                                saved.base_url = Some(config.base_url.clone());
                                saved.api_key = config.api_key.clone();
                                saved.auth_mode = Some("api_key".into());
                                if is_switching {
                                    let models = get_provider_models_with_live(&item.id, &item.base_url, Some(&final_key), &catalog);
                                    saved.model = models.first().map(|(id, _)| id.clone());
                                    config.model = saved.model.clone();
                                } else if saved.model.is_none() {
                                    if let Some(m) = &config.model {
                                        saved.model = Some(m.clone());
                                    } else {
                                        let models = get_provider_models_with_live(&item.id, &item.base_url, Some(&final_key), &catalog);
                                        saved.model = models.first().map(|(id, _)| id.clone());
                                        config.model = saved.model.clone();
                                    }
                                } else {
                                    config.model = saved.model.clone();
                                }
                                save_saved_config_at(&path, &saved)?;
                                return Ok(true);
                            } else {
                                save_auth_key(&item.id, &final_key)?;
                                config.base_url = item.base_url.clone();
                                config.api_key = Some(final_key.clone());
                                let models = get_provider_models_with_live(&item.id, &item.base_url, Some(&final_key), &catalog);
                                state = ModalState::SelectingModel {
                                    item: item.clone(),
                                    models,
                                    filter: String::new(),
                                    sel: 0,
                                    scroll_top: 0,
                                };
                            }
                        }
                        _ => {}
                    },
                    Event::Resize(_, _) => {}
                    _ => {}
                },
                ModalState::SelectingModel {
                    item,
                    models,
                    filter: m_filter,
                    sel: m_sel,
                    scroll_top: _,
                } => {
                    let filtered_models: Vec<&(String, String)> = models
                        .iter()
                        .filter(|(m_id, m_name)| {
                            let f = m_filter.to_lowercase();
                            f.is_empty() || m_id.to_lowercase().contains(&f) || m_name.to_lowercase().contains(&f)
                        })
                        .collect();

                    if filtered_models.is_empty() {
                        *m_sel = 0;
                    } else if *m_sel >= filtered_models.len() {
                        *m_sel = filtered_models.len().saturating_sub(1);
                    }

                    match read()? {
                        Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                            if modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
                        Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                            match code {
                                KeyCode::Char('p') => *m_sel = m_sel.saturating_sub(1),
                                KeyCode::Char('n') => {
                                    if !filtered_models.is_empty() {
                                        *m_sel = (*m_sel + 1).min(filtered_models.len() - 1);
                                    }
                                }
                                _ => {}
                            }
                        }
                        Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                            KeyCode::Up => *m_sel = m_sel.saturating_sub(1),
                            KeyCode::Down => {
                                if !filtered_models.is_empty() {
                                    *m_sel = (*m_sel + 1).min(filtered_models.len() - 1);
                                }
                            }
                            KeyCode::PageUp => *m_sel = m_sel.saturating_sub(8),
                            KeyCode::PageDown => {
                                if !filtered_models.is_empty() {
                                    *m_sel = (*m_sel + 8).min(filtered_models.len() - 1);
                                }
                            }
                            KeyCode::Char(ch) => {
                                m_filter.push(ch);
                                *m_sel = 0;
                            }
                            KeyCode::Backspace => {
                                m_filter.pop();
                                *m_sel = 0;
                            }
                            KeyCode::Esc => {
                                state = ModalState::Selecting;
                            }
                            KeyCode::Enter => {
                                let chosen_model = if let Some(&(m_id, _)) = filtered_models.get(*m_sel) {
                                    m_id.clone()
                                } else if !m_filter.is_empty() {
                                    m_filter.trim().to_string()
                                } else {
                                    "default".to_string()
                                };

                                config.model = Some(chosen_model.clone());

                                let path = saved_config_path()?;
                                let mut saved = load_saved_config_at(&path);
                                saved.base_url = Some(config.base_url.clone());
                                saved.api_key = config.api_key.clone();
                                saved.model = config.model.clone();
                                saved.auth_mode = Some(if item.no_auth { "none".into() } else { "api_key".into() });
                                save_saved_config_at(&path, &saved)?;

                                connected_name = Some((item.name.clone(), chosen_model));
                                return Ok(true);
                            }
                            _ => {}
                        },
                        Event::Resize(_, _) => {}
                        _ => {}
                    }
                }
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

pub fn run_model_modal(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use ratatui::Terminal;
    use std::collections::BTreeMap;
    use std::io::Write as _;
    use std::time::Duration;

    let catalog = match load_catalog() {
        Ok(c) => c,
        Err(_) => BTreeMap::new(),
    };

    let (item_id, item_name) = if let Some((pid, p)) = catalog.iter().find(|(_, p)| p.base_url == config.base_url) {
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
    };

    let models = get_provider_models_with_live(&item_id, &config.base_url, config.api_key.as_deref(), &catalog);

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;

    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            let filtered_models: Vec<&(String, String)> = models
                .iter()
                .filter(|(m_id, m_name)| {
                    let f = filter.to_lowercase();
                    f.is_empty() || m_id.to_lowercase().contains(&f) || m_name.to_lowercase().contains(&f)
                })
                .collect();

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

                render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 68.min(area.width.saturating_sub(4)).max(42).min(area.width);
                let modal_h = 16.min(area.height.saturating_sub(2)).max(10).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                frame.render_widget(Clear, modal_rect);

                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);

                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));

                // Header
                let title_str = format!("Select model \u{2014} {}", item.name);
                let esc_str = "esc";
                let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                let header_line = Line::from(vec![
                    Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));

                // Search Bar
                let search_line = if filter.is_empty() {
                    Line::from(vec![
                        Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled("Type to filter models...", Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled(&filter, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                    ])
                };
                frame.render_widget(Paragraph::new(search_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));

                // List
                let list_y = inner.y + 3;
                let list_h = inner.height.saturating_sub(4) as usize;

                if filtered_models.is_empty() {
                    let empty_msg = if filter.is_empty() {
                        Paragraph::new(Line::from(vec![
                            Span::styled("  No models listed — press Enter to continue", Style::default().fg(text_dim).bg(box_bg)),
                        ]))
                    } else {
                        Paragraph::new(Line::from(vec![
                            Span::styled("  Use custom model: ", Style::default().fg(text_dim).bg(box_bg)),
                            Span::styled(&filter, Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
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

                        let display_name = if m_name.is_empty() { m_id.as_str() } else { m_name.as_str() };
                        let sub = if m_name.is_empty() || m_name == m_id {
                            String::new()
                        } else {
                            format!(" {}", m_id)
                        };

                        let raw_content = format!(" {check_glyph}{}{sub}", display_name);
                        let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                        let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

                        let row_line = if is_selected {
                            Line::from(Span::styled(
                                full_row_str,
                                Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD),
                            ))
                        } else {
                            let check_span = if is_current {
                                Span::styled(" ✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD).bg(box_bg))
                            } else {
                                Span::styled("   ", Style::default().bg(box_bg))
                            };
                            let name_span = Span::styled(display_name, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                            let sub_span = Span::styled(sub, Style::default().fg(Color::Rgb(130, 130, 130)).bg(box_bg));
                            let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
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
                    Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
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
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                    match code {
                        KeyCode::Char('p') => sel = sel.saturating_sub(1),
                        KeyCode::Char('n') => {
                            if !filtered_models.is_empty() {
                                sel = (sel + 1).min(filtered_models.len() - 1);
                            }
                        }
                        _ => {}
                    }
                }
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
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

/// Thinking levels and descriptions matching Pi / Prime-Agent.
pub const THINKING_LEVELS: &[(&str, &str)] = &[
    ("off", "No reasoning"),
    ("minimal", "Very brief reasoning"),
    ("low", "Light reasoning"),
    ("medium", "Moderate reasoning"),
    ("high", "Deep reasoning"),
    ("xhigh", "Very deep reasoning"),
    ("max", "Maximum reasoning"),
];

/// Interactive "Thinking effort" GUI modal matching Pi / Prime-Agent.
pub fn run_effort_modal(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use ratatui::Terminal;
    use std::io::Write as _;
    use std::time::Duration;

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let current_level = config.thinking_effort.clone().unwrap_or_else(|| "high".to_string());
    let mut sel = THINKING_LEVELS.iter().position(|(l, _)| *l == current_level).unwrap_or(4);

    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

                render_dimmed_background(frame, &bg_snapshot);

                let modal_w = 58.min(area.width.saturating_sub(4)).max(36).min(area.width);
                let modal_h = (THINKING_LEVELS.len() as u16 + 5).min(area.height.saturating_sub(2)).max(10).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);

                frame.render_widget(Clear, modal_rect);

                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);

                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));

                // Header
                let title_str = "Thinking effort";
                let esc_str = "esc";
                let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                let header_line = Line::from(vec![
                    Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));

                // List of levels
                let list_y = inner.y + 2;
                for (idx, (level, desc)) in THINKING_LEVELS.iter().enumerate() {
                    let is_selected = idx == sel;
                    let is_current = current_level == *level;

                    let check_glyph = if is_current { "✓ " } else { "  " };
                    let raw_content = format!(" {check_glyph}{level:<8}  {desc}");
                    let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                    let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));

                    let row_line = if is_selected {
                        Line::from(Span::styled(
                            full_row_str,
                            Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD),
                        ))
                    } else {
                        let check_span = if is_current {
                            Span::styled(" ✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD).bg(box_bg))
                        } else {
                            Span::styled("   ", Style::default().bg(box_bg))
                        };
                        let name_span = Span::styled(format!("{level:<8}  "), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                        let desc_span = Span::styled(*desc, Style::default().fg(Color::Rgb(140, 140, 140)).bg(box_bg));
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
                    Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
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
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) => return Ok(false),
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                    match code {
                        KeyCode::Char('p') => sel = sel.saturating_sub(1),
                        KeyCode::Char('n') => sel = (sel + 1).min(THINKING_LEVELS.len().saturating_sub(1)),
                        _ => {}
                    }
                }
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => sel = (sel + 1).min(THINKING_LEVELS.len().saturating_sub(1)),
                    KeyCode::Esc => return Ok(false),
                    KeyCode::Enter => {
                        let (chosen, _) = THINKING_LEVELS[sel];
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

pub async fn run_effort_menu(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    run_effort_modal(config, bg)
}

pub async fn run_model_menu(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    run_model_modal(config, bg)
}

pub async fn run_provider_menu(config: &mut Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<bool> {
    run_connect_modal(config, bg)
}

/// Interactive proxy picker — like /model but for `gray proxy`.
/// Returns Some(provider) when user picks one, None on cancel.
pub fn run_proxy_modal(_config: &Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<Option<String>> {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use ratatui::Terminal;
    use std::io::Write as _;
    use std::time::Duration;

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();
    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    // Fixed proxy providers — matches crates/gray/src/proxy.rs get_adapter keys
    let providers: &[(&str, &str)] = &[
        ("openrouter", "OpenRouter"),
        ("xai", "xAI Grok"),
        ("codex", "Codex (OpenAI)"),
    ];
    let mut sel = 0usize;
    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<Option<String>> {
        loop {
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = 62.min(area.width.saturating_sub(4)).max(40).min(area.width);
                let modal_h = (providers.len() as u16 + 6).min(area.height.saturating_sub(2)).max(10).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);
                frame.render_widget(Clear, modal_rect);
                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);
                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));
                let title_str = "Proxy upstream";
                let esc_str = "esc";
                let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                let header_line = Line::from(vec![
                    Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));
                let sub_line = Line::from(Span::styled(
                    "Share auth via http://127.0.0.1:8645/v1  •  any bearer forwarded",
                    Style::default().fg(Color::Rgb(100, 100, 100)).bg(box_bg),
                ));
                frame.render_widget(Paragraph::new(sub_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));
                let list_y = inner.y + 3;
                for (idx, (id, display)) in providers.iter().enumerate() {
                    let is_selected = idx == sel;
                    let is_auth = crate::proxy::get_adapter(id).map(|a| a.is_authenticated()).unwrap_or(false);
                    let status = if is_auth { "ready" } else { "not logged in — /connect" };
                    let status_color = if is_auth { Color::Rgb(74, 222, 128) } else { Color::Rgb(140, 120, 120) };
                    let check = if is_auth { "✓ " } else { "  " };
                    let raw_content = format!(" {check}{:<10}  {status}", display);
                    let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                    let full_row_str = format!("{}{}", raw_content, " ".repeat(fill));
                    let row_line = if is_selected {
                        Line::from(Span::styled(full_row_str, Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD)))
                    } else {
                        let check_span = if is_auth {
                            Span::styled(" ✓ ", Style::default().fg(Color::Rgb(74, 222, 128)).add_modifier(Modifier::BOLD).bg(box_bg))
                        } else {
                            Span::styled("   ", Style::default().bg(box_bg))
                        };
                        let name_span = Span::styled(format!("{display:<10}  "), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                        let status_span = Span::styled(status, Style::default().fg(status_color).bg(box_bg));
                        let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                        Line::from(vec![check_span, name_span, status_span, pad_span])
                    };
                    frame.render_widget(Paragraph::new(row_line), Rect::new(inner.x, list_y + idx as u16, inner.width, 1));
                }
                let footer_line = Line::from(vec![
                    Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("start", Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(footer_line), Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1));
            })?;
            if !poll(Duration::from_millis(100))? {
                continue;
            }
            match read()? {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => match code {
                    KeyCode::Char('p') => sel = sel.saturating_sub(1),
                    KeyCode::Char('n') => sel = (sel + 1).min(providers.len().saturating_sub(1)),
                    _ => {}
                },
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => sel = (sel + 1).min(providers.len().saturating_sub(1)),
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Enter => {
                        let (id, _) = providers[sel];
                        return Ok(Some(id.to_string()));
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    })();
    let _ = terminal.clear();
    let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    if !was_raw {
        let _ = disable_raw_mode();
    } else {
        let _ = enable_raw_mode();
    }
    let _ = std::io::stdout().flush();
    result
}

pub async fn run_proxy_menu(config: &Config, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<Option<String>> {
    run_proxy_modal(config, bg)
}

/// Interactive skills picker — searchable list like /resume, peach highlight.
/// Returns the selected skill plus optional args (text after first space in query).
/// Bare `/skills` opens this; `/skills:name` bypasses it via expand_skill_command.
pub fn run_skills_modal(cwd: &std::path::Path, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<Option<(crate::skills::Skill, String)>> {
    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
    use ratatui::backend::CrosstermBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use ratatui::Terminal;
    use std::io::Write as _;
    use std::time::Duration;

    let discovered = crate::skills::discover_skills(cwd);
    let skills = discovered.skills;
    if skills.is_empty() {
        return Ok(None);
    }

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();
    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let mut query = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;
    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);

    let result: anyhow::Result<Option<(crate::skills::Skill, String)>> = (|| {
        loop {
            // split query into filter part (before first space) and args part (after)
            let (filter_part, args_part) = if let Some(pos) = query.find(char::is_whitespace) {
                (query[..pos].trim().to_string(), query[pos..].trim().to_string())
            } else {
                (query.trim().to_string(), String::new())
            };
            let filter_lower = filter_part.to_lowercase();
            let filtered: Vec<&crate::skills::Skill> = skills
                .iter()
                .filter(|s| {
                    if filter_lower.is_empty() {
                        true
                    } else {
                        s.name.to_lowercase().contains(&filter_lower)
                            || s.description.to_lowercase().contains(&filter_lower)
                    }
                })
                .collect();

            if sel >= filtered.len() && !filtered.is_empty() {
                sel = filtered.len() - 1;
            }
            if filtered.is_empty() {
                sel = 0;
            }
            if sel < scroll_top {
                scroll_top = sel;
            }

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 30 || area.height < 8 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = 78.min(area.width.saturating_sub(2)).max(44).min(area.width);
                let modal_h = 20.min(area.height.saturating_sub(2)).max(12).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);
                frame.render_widget(Clear, modal_rect);
                let box_block = Block::default().style(Style::default().bg(box_bg));
                frame.render_widget(box_block, modal_rect);
                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));
                let title = format!("Skills — {} available", skills.len());
                let esc_str = "esc";
                let pad_len = (inner.width as usize).saturating_sub(title.chars().count() + esc_str.chars().count());
                let header = Line::from(vec![
                    Span::styled(title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(header), Rect::new(inner.x, inner.y, inner.width, 1));
                let search_line = if query.is_empty() {
                    Line::from(vec![
                        Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled("Type to filter — select to run (args after space)", Style::default().fg(Color::Rgb(90, 90, 90)).bg(box_bg)),
                    ])
                } else {
                    let args_hint = if !args_part.is_empty() {
                        format!("  · args: {args_part}")
                    } else {
                        String::new()
                    };
                    Line::from(vec![
                        Span::styled("Search: ", Style::default().fg(accent_peach).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled(query.clone(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled("▎", Style::default().fg(accent_peach).bg(box_bg)),
                        Span::styled(args_hint, Style::default().fg(Color::Rgb(100, 100, 100)).bg(box_bg)),
                    ])
                };
                frame.render_widget(Paragraph::new(search_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));
                let list_y = inner.y + 3;
                let list_h = inner.height.saturating_sub(4) as usize;
                if filtered.is_empty() {
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled("  No matching skills", Style::default().fg(text_dim).bg(box_bg)))),
                        Rect::new(inner.x, list_y, inner.width, 1),
                    );
                } else {
                    let visible = list_h.max(1);
                    if sel >= scroll_top + visible {
                        scroll_top = sel + 1 - visible;
                    }
                    for r in 0..visible {
                        let idx = scroll_top + r;
                        if idx >= filtered.len() {
                            break;
                        }
                        let s = filtered[idx];
                        let is_sel = idx == sel;
                        // truncate description to fit
                        let name_w = 22usize;
                        let desc_avail = (inner.width as usize).saturating_sub(4 + name_w + 3).max(10);
                        let desc = if s.description.chars().count() > desc_avail {
                            let mut t: String = s.description.chars().take(desc_avail - 1).collect();
                            t.push('…');
                            t
                        } else {
                            s.description.clone()
                        };
                        let raw = format!(" {name:<name_w$}  {desc}", name = s.name, desc = desc, name_w = name_w);
                        let fill = (inner.width as usize).saturating_sub(raw.chars().count());
                        let row_str = format!("{}{}", raw, " ".repeat(fill));
                        let line = if is_sel {
                            Line::from(Span::styled(row_str, Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD)))
                        } else {
                            let name_span = Span::styled(format!(" {name:<name_w$}  ", name = s.name, name_w = name_w), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                            let desc_span = Span::styled(desc, Style::default().fg(Color::Rgb(140, 140, 140)).bg(box_bg));
                            let pad_span = Span::styled(" ".repeat(fill), Style::default().bg(box_bg));
                            Line::from(vec![name_span, desc_span, pad_span])
                        };
                        frame.render_widget(Paragraph::new(line), Rect::new(inner.x, list_y + r as u16, inner.width, 1));
                    }
                }
                let footer = Line::from(vec![
                    Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("navigate  ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("run  ", Style::default().fg(text_dim).bg(box_bg)),
                    Span::styled("esc ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                    Span::styled("cancel", Style::default().fg(text_dim).bg(box_bg)),
                ]);
                frame.render_widget(Paragraph::new(footer), Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1));
            })?;
            if !poll(Duration::from_millis(80))? {
                continue;
            }
            match read()? {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => match code {
                    KeyCode::Char('p') => sel = sel.saturating_sub(1),
                    KeyCode::Char('n') => {
                        let count = filtered.len();
                        if count > 0 { sel = (sel + 1).min(count - 1); }
                    }
                    _ => {}
                },
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => {
                        let c = filtered.len();
                        if c > 0 { sel = (sel + 1).min(c - 1); }
                    }
                    KeyCode::Enter => {
                        if let Some(s) = filtered.get(sel) {
                            return Ok(Some(((*s).clone(), args_part.clone())));
                        }
                    }
                    KeyCode::Backspace => { query.pop(); sel = 0; }
                    KeyCode::Char(ch) => { query.push(ch); sel = 0; }
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    })();
    let _ = terminal.clear();
    let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    if !was_raw { let _ = disable_raw_mode(); } else { let _ = enable_raw_mode(); }
    let _ = std::io::stdout().flush();
    result
}

pub async fn run_skills_picker(cwd: &std::path::Path, bg: Option<&BackgroundSnapshot>) -> anyhow::Result<Option<(crate::skills::Skill, String)>> {
    run_skills_modal(cwd, bg)
}

pub async fn run_onboarding(config: &mut Config) -> anyhow::Result<bool> {
    let _ = crossterm::terminal::disable_raw_mode();
    crate::tui::clear_screen();
    print!("\r\n");
    crate::tui::print_logo();
    print!("\r\n");
    print_wrapped("\x1b[2mWelcome to gray by alignment\x1b[0m", 2);
    print_wrapped(
        "\x1b[2mgray is a minimal agent that runs tools, edits code, and works with any model provider.\x1b[0m",
        2,
    );
    print!("\r\n");
    run_provider_menu(config, None).await
}

