//! Connect-a-provider modal: shell + event loop (render arms live in `connect_draw`/`connect_models`).

use super::*;
use ratatui::style::Color;

/// Shared palette for the connect-modal render arms.
pub(crate) struct ConnectColors {
    pub box_bg: Color,
    pub input_bg: Color,
    pub accent_peach: Color,
    pub text_dim: Color,
}

/// Interactive "Connect a provider" GUI modal with clean colored box styling.
/// Floating container block matching the composer prompt text box, live search filter,
/// peach selection highlight, and in-modal API key entry.
pub fn run_connect_modal(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use crossterm::terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    };
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
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
    crossterm::execute!(
        stdout_handle,
        EnterAlternateScreen,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        crossterm::cursor::Hide
    )?;
    let _ = crossterm::terminal::size();

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let colors = ConnectColors {
        box_bg: Color::Rgb(22, 22, 22),
        input_bg: Color::Rgb(32, 32, 32),
        accent_peach: Color::Rgb(246, 173, 126),
        text_dim: Color::Rgb(120, 120, 120),
    };

    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        loop {
            let auth_keys = load_auth_keys();

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }

                render_dimmed_background(frame, &bg_snapshot);

                match &mut state {
                    ModalState::Selecting => super::connect_draw::render_selecting(
                        frame,
                        area,
                        &all_items,
                        filter.as_str(),
                        sel,
                        &mut scroll_top,
                        config,
                        &auth_keys,
                        &colors,
                    ),
                    ModalState::EnteringKey {
                        item,
                        key_buf,
                        existing_key,
                        status_msg,
                    } => super::connect_draw::render_entering_key(
                        frame,
                        area,
                        item,
                        key_buf,
                        existing_key,
                        status_msg,
                        &colors,
                    ),
                    ModalState::SelectingModel {
                        item,
                        models,
                        filter: m_filter,
                        sel: m_sel,
                        scroll_top: m_scroll_top,
                    } => super::connect_models::render_selecting_model(
                        frame,
                        area,
                        item,
                        models,
                        m_filter,
                        *m_sel,
                        m_scroll_top,
                        config,
                        &colors,
                    ),
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
                            KeyCode::Char('n') if !filtered.is_empty() => {
                                sel = (sel + 1).min(filtered.len() - 1);
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
                                        let models = get_provider_models_with_live(
                                            &item.id,
                                            &item.base_url,
                                            None,
                                            &catalog,
                                        );
                                        state = ModalState::SelectingModel {
                                            item: item.clone(),
                                            models,
                                            filter: String::new(),
                                            sel: 0,
                                            scroll_top: 0,
                                        };
                                    } else {
                                        let existing =
                                            load_auth_keys().get(&item.id).cloned().or_else(|| {
                                                if config.base_url == item.base_url {
                                                    config.api_key.clone()
                                                } else {
                                                    None
                                                }
                                            });
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
                    Event::Key(KeyEvent {
                        code: KeyCode::Char('c'),
                        modifiers,
                        kind: KeyEventKind::Press,
                        ..
                    }) if modifiers.contains(KeyModifiers::CONTROL) => {
                        state = ModalState::Selecting;
                    }
                    Event::Key(KeyEvent {
                        code,
                        kind: KeyEventKind::Press,
                        ..
                    }) => match code {
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
                                *status_msg =
                                    Some("No API key entered — please enter a valid key".into());
                            } else if existing_key.is_some() {
                                save_auth_key(&item.id, &final_key)?;
                                let path = saved_config_path()?;
                                let mut saved = load_saved_config_at(&path);
                                let is_switching =
                                    saved.base_url.as_deref() != Some(item.base_url.as_str());
                                config.base_url = item.base_url.clone();
                                config.api_key = Some(final_key.clone());
                                saved.base_url = Some(config.base_url.clone());
                                saved.api_key = config.api_key.clone();
                                saved.auth_mode = Some(AUTH_MODE_API_KEY.into());
                                if is_switching {
                                    let models = get_provider_models_with_live(
                                        &item.id,
                                        &item.base_url,
                                        Some(&final_key),
                                        &catalog,
                                    );
                                    saved.model = models.first().map(|(id, _)| id.clone());
                                    config.model = saved.model.clone();
                                } else if saved.model.is_none() {
                                    if let Some(m) = &config.model {
                                        saved.model = Some(m.clone());
                                    } else {
                                        let models = get_provider_models_with_live(
                                            &item.id,
                                            &item.base_url,
                                            Some(&final_key),
                                            &catalog,
                                        );
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
                                let models = get_provider_models_with_live(
                                    &item.id,
                                    &item.base_url,
                                    Some(&final_key),
                                    &catalog,
                                );
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
                            f.is_empty()
                                || m_id.to_lowercase().contains(&f)
                                || m_name.to_lowercase().contains(&f)
                        })
                        .collect();

                    if filtered_models.is_empty() {
                        *m_sel = 0;
                    } else if *m_sel >= filtered_models.len() {
                        *m_sel = filtered_models.len().saturating_sub(1);
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
                            KeyCode::Char('p') => *m_sel = m_sel.saturating_sub(1),
                            KeyCode::Char('n') if !filtered_models.is_empty() => {
                                *m_sel = (*m_sel + 1).min(filtered_models.len() - 1);
                            }
                            _ => {}
                        },
                        Event::Key(KeyEvent {
                            code,
                            kind: KeyEventKind::Press,
                            ..
                        }) => match code {
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
                                let chosen_model =
                                    if let Some(&(m_id, _)) = filtered_models.get(*m_sel) {
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
                                saved.auth_mode = Some(if item.no_auth {
                                    AUTH_MODE_NONE.into()
                                } else {
                                    AUTH_MODE_API_KEY.into()
                                });
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
