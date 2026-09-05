pub mod catalog;
pub use catalog::{
    build_connect_items, gray_home, load_catalog, load_saved_config_at, mask_key_pretty,
    normalize_auth_mode, save_saved_config_at, saved_config_path, Catalog, CatalogModel,
    CatalogProvider, ConnectItem, SavedConfig, AUTH_MODE_API_KEY, AUTH_MODE_NONE,
    AUTH_MODE_OAUTH, OAUTH_CAPABLE, PROVIDERS_JSON,
};
pub(crate) use catalog::{load_auth_keys, save_auth_key};

pub mod context;
pub use context::{
    cache_model_context, cache_model_context_if_absent, cache_models_dev_if_absent,
    context_source, default_keep_for_window, default_reserve_for_window, estimate_str_tokens,
    extract_context_length_from_json, fetch_litellm_context_windows, fetch_models_dev_context,
    fetch_live_provider_models, fetch_openrouter_rates, format_context_length, format_cost, friendly_model_name,
    get_cached_model_context, get_model_rate, get_provider_models, get_provider_models_with_live,
    get_user_context_window, load_models_cache_to_memory, model_context_info, model_max_context,
    parse_context_window, parse_litellm_context_json, parse_models_dev_json, parse_openrouter_models_json,
    resolve_model_context_length, save_models_cache_to_disk, set_user_context_window,
    set_user_keep_recent_tokens, set_user_reserve_tokens, turn_cost, user_keep_for,
    user_keep_recent_tokens, user_reserve_tokens, user_reserve_tokens_for, ContextParts, ModelRate,
    DEFAULT_KEEP_RECENT_TOKENS, DEFAULT_RESERVE_TOKENS,
};

pub mod ui;
pub use ui::{BackgroundSnapshot, dim_color, dim_line, dim_style, render_dimmed_background};
pub mod icons;
pub use icons::{has_nerd_font, icon, init_nerd_font, set_nerd_font};

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
        ChoosingAuthMethod {
            item: ConnectItem,
            sel: usize,
            status_msg: Option<String>,
        },
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
    let mut authed_via_oauth = false;

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::terminal::Clear(crossterm::terminal::ClearType::All), crossterm::cursor::Hide)?;
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
            let oauth_ok: std::collections::HashSet<&str> = ["xai", "codex"]
                .into_iter()
                .filter(|p| crate::oauth::has_oauth(p))
                .collect();

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
                                    || (config.base_url == item.base_url && config.api_key.is_some())
                                    || crate::oauth::oauth_provider_for_connect_id(&item.id)
                                        .is_some_and(|p| oauth_ok.contains(p));

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
                    ModalState::ChoosingAuthMethod { item, sel, status_msg } => {
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

                        let title_str = format!("Connect {}", item.name);
                        let esc_str = "esc";
                        let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                        frame.render_widget(
                            Paragraph::new(Line::from(vec![
                                Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                                Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                                Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                            ])),
                            Rect::new(inner.x, inner.y, inner.width, 1),
                        );

                        let rows = ["Log in with browser (recommended)", "Paste an API key instead"];
                        for (r, label) in rows.iter().enumerate() {
                            let raw = format!("  {label}");
                            let fill = (inner.width as usize).saturating_sub(raw.chars().count());
                            let line = if *sel == r {
                                Line::from(Span::styled(
                                    format!("{raw}{}", " ".repeat(fill)),
                                    Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD),
                                ))
                            } else {
                                Line::from(vec![
                                    Span::styled(format!("  {label}"), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                                    Span::styled(" ".repeat(fill), Style::default().bg(box_bg)),
                                ])
                            };
                            frame.render_widget(Paragraph::new(line), Rect::new(inner.x, inner.y + 2 + r as u16, inner.width, 1));
                        }

                        if let Some(msg) = status_msg {
                            let trunc: String = msg.chars().take(80).collect();
                            frame.render_widget(
                                Paragraph::new(Line::from(Span::styled(
                                    format!(" • {trunc}"),
                                    Style::default().fg(Color::Rgb(239, 68, 68)).bg(box_bg),
                                ))),
                                Rect::new(inner.x, inner.y + 4, inner.width, 1),
                            );
                        }

                        let footer = Line::from(vec![
                            Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                            Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                            Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
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
                                    } else if item.oauth_capable {
                                        state = ModalState::ChoosingAuthMethod {
                                            item: item.clone(),
                                            sel: 0,
                                            status_msg: None,
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
                                saved.auth_mode = Some(AUTH_MODE_API_KEY.into());
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
                ModalState::ChoosingAuthMethod { item, sel, status_msg } => match read()? {
                    Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                        if modifiers.contains(KeyModifiers::CONTROL) => {
                        state = ModalState::Selecting;
                    }
                    Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                        KeyCode::Esc => {
                            state = ModalState::Selecting;
                        }
                        KeyCode::Up => {
                            *sel = sel.saturating_sub(1);
                        }
                        KeyCode::Down => {
                            *sel = (*sel + 1).min(1);
                        }
                        KeyCode::Enter => {
                            let use_oauth = *sel == 0
                                && crate::oauth::oauth_provider_for_connect_id(&item.id).is_some();
                            if !use_oauth {
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
                            } else if let Some(provider) =
                                crate::oauth::oauth_provider_for_connect_id(&item.id)
                            {
                                // Browser login prints its URL and blocks on the
                                // loopback callback, so it runs outside the
                                // alternate screen, then the modal re-enters.
                                let _ = terminal.clear();
                                let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
                                if !was_raw {
                                    let _ = disable_raw_mode();
                                }
                                let _ = std::io::stdout().flush();
                                // All callers run under multi-thread tokio::main.
                                let auth_result = tokio::task::block_in_place(|| {
                                    tokio::runtime::Handle::current().block_on(async {
                                        match provider {
                                            "xai" => crate::oauth::run_xai_signin().await,
                                            _ => crate::oauth::run_codex_signin().await,
                                        }?;
                                        crate::oauth::ensure_access_token(provider).await
                                    })
                                });
                                if !was_raw {
                                    let _ = enable_raw_mode();
                                }
                                let mut stdout_handle = std::io::stdout();
                                let _ = crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::terminal::Clear(crossterm::terminal::ClearType::All), crossterm::cursor::Hide);
                                match auth_result {
                                    Ok(token) => {
                                        config.base_url = item.base_url.clone();
                                        config.api_key = Some(token.clone());
                                        authed_via_oauth = true;
                                        let models = get_provider_models_with_live(&item.id, &item.base_url, Some(&token), &catalog);
                                        state = ModalState::SelectingModel {
                                            item: item.clone(),
                                            models,
                                            filter: String::new(),
                                            sel: 0,
                                            scroll_top: 0,
                                        };
                                    }
                                    Err(e) => {
                                        let short: String = format!("{e:#}").chars().take(100).collect();
                                        *status_msg = Some(short.replace('\n', " "));
                                    }
                                }
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
                                if authed_via_oauth && !item.no_auth {
                                    // OAuth bearer lives in auth.json, never config.json.
                                    saved.api_key = None;
                                    saved.auth_mode = Some(AUTH_MODE_OAUTH.into());
                                } else {
                                    saved.auth_mode = Some(if item.no_auth { AUTH_MODE_NONE.into() } else { AUTH_MODE_API_KEY.into() });
                                }
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
        oauth_capable: crate::setup::catalog::OAUTH_CAPABLE.contains(&item_id.as_str()),
    };

    let models = get_provider_models_with_live(&item_id, &config.base_url, config.api_key.as_deref(), &catalog);

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::terminal::Clear(crossterm::terminal::ClearType::All), crossterm::cursor::Hide)?;
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
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::terminal::Clear(crossterm::terminal::ClearType::All), crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();

    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);

    let current_level = config.thinking_effort.clone().unwrap_or_else(|| "high".to_string());
    let mut sel = THINKING_LEVELS.iter().position(|(l, _)| *l == current_level).unwrap_or(4);
    // Extra trailing row: reasoning-text display toggle (not an effort level).
    let rows = THINKING_LEVELS.len() + 1;
    let max_sel = rows.saturating_sub(1);

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
                let modal_h = (rows as u16 + 5).min(area.height.saturating_sub(2)).max(10).min(area.height);
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

                // List of levels + display toggle
                let list_y = inner.y + 2;
                for idx in 0..rows {
                    let (level, desc, is_current) = if idx < THINKING_LEVELS.len() {
                        let (l, d) = THINKING_LEVELS[idx];
                        (l, d, current_level == l)
                    } else {
                        let shown = config.show_reasoning.unwrap_or(true);
                        ("display", if shown { "Reasoning text shown" } else { "Reasoning text hidden" }, shown)
                    };
                    let is_selected = idx == sel;

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
                        let desc_span = Span::styled(desc, Style::default().fg(Color::Rgb(140, 140, 140)).bg(box_bg));
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
                        KeyCode::Char('n') => sel = (sel + 1).min(max_sel),
                        _ => {}
                    }
                }
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => sel = (sel + 1).min(max_sel),
                    KeyCode::Esc => return Ok(false),
                    KeyCode::Enter => {
                        if sel == THINKING_LEVELS.len() {
                            // Display toggle: flip, persist, stay open.
                            let shown = !config.show_reasoning.unwrap_or(true);
                            config.show_reasoning = Some(shown);
                            let path = saved_config_path()?;
                            let mut saved = load_saved_config_at(&path);
                            saved.show_reasoning = Some(shown);
                            save_saved_config_at(&path, &saved)?;
                            continue;
                        }
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
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::terminal::Clear(crossterm::terminal::ClearType::All), crossterm::cursor::Hide)?;
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

/// Interactive `/context` visual: Codex-style 10×10 usage grid + per-category
/// breakdown + editable window/reserve/keep. Mutates `config` (and the global
/// override cells + saved config) on save. Returns true if anything changed.
pub fn run_context_modal(
    config: &mut Config,
    parts: &ContextParts,
    model: &str,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<bool> {
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

    fn persist(cfg: &Config) {
        if let Ok(path) = saved_config_path() {
            let mut saved = load_saved_config_at(&path);
            saved.context_window = cfg.context_window;
            saved.context_reserve = cfg.context_reserve;
            saved.context_keep = cfg.context_keep;
            let _ = save_saved_config_at(&path, &saved);
        }
    }

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        enable_raw_mode()?;
    }
    let mut stdout_handle = std::io::stdout();
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::terminal::Clear(crossterm::terminal::ClearType::All), crossterm::cursor::Hide)?;
    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);
    // jewel-tone category colors (deliberately not Claude's purple/pink)
    let c_sys = Color::Rgb(148, 163, 184); // slate
    let c_ctx = Color::Rgb(45, 212, 191); // teal
    let c_tools = Color::Rgb(56, 189, 248); // sky
    let c_skills = Color::Rgb(163, 230, 53); // lime
    let c_msgs = Color::Rgb(251, 191, 36); // amber
    let c_free = Color::Rgb(90, 90, 90);
    let c_reserve = Color::Rgb(251, 113, 133); // rose

    let mut sel = 0usize;
    let mut editing: Option<usize> = None;
    let mut buf = String::new();
    let mut status: String = String::new();
    let mut changed = false;
    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);
    let setting_labels = ["Window", "Reserve", "Keep"];

    let result = (|| -> anyhow::Result<bool> {
        loop {
            let window = resolve_model_context_length(model);
            let max = model_max_context(model);
            let reserve = user_reserve_tokens_for(window);
            let keep = user_keep_for(window);
            let used = parts.used();
            let free = parts.free(window, reserve);
            let pct = if window > 0 { used * 100 / window } else { 0 };
            let cells = parts.grid_cells(window, reserve);
            // cell kind per grid position: 0-4 categories, 5 free, 6 buffer
            let mut flat: Vec<usize> = Vec::with_capacity(100);
            for (kind, n) in cells.iter().enumerate() {
                flat.extend(std::iter::repeat_n(kind, *n));
            }
            while flat.len() < 100 {
                flat.push(5);
            }

            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 40 || area.height < 20 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = 76.min(area.width.saturating_sub(2)).max(60).min(area.width);
                let modal_h = 24.min(area.height.saturating_sub(1)).max(22).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);
                frame.render_widget(Clear, modal_rect);
                frame.render_widget(Block::default().style(Style::default().bg(box_bg)), modal_rect);
                let inner = Rect::new(modal_x + 3, modal_y + 1, modal_w.saturating_sub(6), modal_h.saturating_sub(2));

                let title = "Context Usage";
                let esc = "esc";
                let pad = (inner.width as usize).saturating_sub(title.chars().count() + esc.chars().count());
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled(" ".repeat(pad), Style::default().bg(box_bg)),
                        Span::styled(esc, Style::default().fg(text_dim).bg(box_bg)),
                    ])),
                    Rect::new(inner.x, inner.y, inner.width, 1),
                );
                let head = format!(
                    "{} · {}/{} tokens ({pct}%)",
                    if model.is_empty() { "no model" } else { model },
                    format_context_length(used),
                    format_context_length(window),
                );
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        head,
                        Style::default().fg(Color::White).bg(box_bg),
                    ))),
                    Rect::new(inner.x, inner.y + 1, inner.width, 1),
                );

                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "Estimated usage by category",
                        Style::default().fg(text_dim).add_modifier(Modifier::ITALIC).bg(box_bg),
                    ))),
                    Rect::new(inner.x, inner.y + 2, inner.width, 1),
                );

                let grid_y = inner.y + 3;
                let detail_x = inner.x + 24;
                let detail_w = inner.width.saturating_sub(24);
                let pct_of = |n: usize| if window > 0 { n * 100 / window } else { 0 };
                let row = |glyph: &str, color: Color, label: &str, n: usize| {
                    Line::from(vec![
                        Span::styled(format!("{glyph} "), Style::default().fg(color).bg(box_bg)),
                        Span::styled(
                            format!("{label}: {} tokens ({}%)", format_context_length(n), pct_of(n)),
                            Style::default().fg(Color::White).bg(box_bg),
                        ),
                    ])
                };
                let details = [
                    row(icon("cell"), c_sys, "System prompt", parts.system_prompt),
                    row(icon("cell"), c_ctx, "Project context", parts.project_context),
                    row(icon("cell"), c_tools, "System tools", parts.tools),
                    row(icon("cell"), c_skills, "Skills", parts.skills),
                    row(icon("cell"), c_msgs, "Messages", parts.messages),
                    Line::from(vec![
                        Span::styled(format!("{} ", icon("cell_free")), Style::default().fg(c_free).bg(box_bg)),
                        Span::styled(
                            format!("Free space: {} ({}%)", format_context_length(free), pct_of(free)),
                            Style::default().fg(text_dim).bg(box_bg),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(format!("{} ", icon("cell_buffer")), Style::default().fg(c_reserve).bg(box_bg)),
                        Span::styled(
                            format!("Autocompact buffer: {} tokens ({}%)", format_context_length(reserve), pct_of(reserve)),
                            Style::default().fg(text_dim).bg(box_bg),
                        ),
                    ]),
                ];
                for r in 0..10u16 {
                    let mut spans = Vec::with_capacity(10);
                    for c in 0..10usize {
                        let kind = flat[(r as usize) * 10 + c];
                        let (g, col) = match kind {
                            0 => (icon("cell"), c_sys),
                            1 => (icon("cell"), c_ctx),
                            2 => (icon("cell"), c_tools),
                            3 => (icon("cell"), c_skills),
                            4 => (icon("cell"), c_msgs),
                            5 => (icon("cell_free"), c_free),
                            _ => (icon("cell_buffer"), c_reserve),
                        };
                        spans.push(Span::styled(format!("{g} "), Style::default().fg(col).bg(box_bg)));
                    }
                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect::new(inner.x, grid_y + r, 20, 1),
                    );
                    if (r as usize) < details.len() && detail_w > 4 {
                        frame.render_widget(
                            Paragraph::new(details[r as usize].clone()),
                            Rect::new(detail_x, grid_y + r, detail_w, 1),
                        );
                    }
                }

                // settings rows
                let set_y = grid_y + 11;
                let win_src = context_source(model);
                let set_rows = [
                    format!("Window: {} ({}) [{win_src}, max {}]", format_context_length(window), window, format_context_length(max)),
                    format!("Reserve: {} ({})", format_context_length(reserve), reserve),
                    format!("Keep: {} ({})", format_context_length(keep), keep),
                ];
                for (i, text) in set_rows.iter().enumerate() {
                    if set_y + i as u16 >= inner.y + inner.height - 2 {
                        break;
                    }
                    let y = set_y + i as u16;
                    if i == sel && editing.is_none() {
                        let raw = format!("{} {text}", icon("arrow"));
                        let fill = (inner.width as usize).saturating_sub(raw.chars().count());
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(
                                format!("{raw}{}", " ".repeat(fill)),
                                Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD),
                            ))),
                            Rect::new(inner.x, y, inner.width, 1),
                        );
                    } else {
                        frame.render_widget(
                            Paragraph::new(Line::from(vec![
                                Span::styled(
                                    format!("  {text}"),
                                    Style::default().fg(if i == sel { Color::White } else { text_dim }).bg(box_bg),
                                ),
                            ])),
                            Rect::new(inner.x, y, inner.width, 1),
                        );
                    }
                }
                // input / footer
                let foot_y = inner.y + inner.height - 1;
                if let Some(idx) = editing {
                    let line = format!("{} {}: {buf}█", icon("arrow"), setting_labels[idx]);
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(line, Style::default().fg(Color::White).bg(box_bg)))),
                        Rect::new(inner.x, foot_y - 1, inner.width, 1),
                    );
                    if !status.is_empty() {
                        frame.render_widget(
                            Paragraph::new(Line::from(Span::styled(status.clone(), Style::default().fg(c_reserve).bg(box_bg)))),
                            Rect::new(inner.x, foot_y, inner.width, 1),
                        );
                    }
                } else {
                    let foot = if status.is_empty() {
                        "↑↓ navigate · enter edit · esc close".to_string()
                    } else {
                        status.clone()
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(foot, Style::default().fg(text_dim).bg(box_bg)))),
                        Rect::new(inner.x, foot_y, inner.width, 1),
                    );
                }
            })?;

            if !poll(Duration::from_millis(100))? {
                continue;
            }
            match read()? {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    return Ok(changed)
                }
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    match code {
                        KeyCode::Char('p') if editing.is_none() => sel = sel.saturating_sub(1),
                        KeyCode::Char('n') if editing.is_none() => sel = (sel + 1).min(2),
                        _ => {}
                    }
                }
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => {
                    if let Some(idx) = editing {
                        match code {
                            KeyCode::Esc => {
                                editing = None;
                                buf.clear();
                                status.clear();
                            }
                            KeyCode::Enter => {
                                let v = buf.trim().to_lowercase();
                                let saved: Option<usize> = match idx {
                                    // window: auto clears, else parse + clamp to model max
                                    0 => {
                                        if ["auto", "clear", "reset", "0"].contains(&v.as_str()) {
                                            config.context_window = None;
                                            set_user_context_window(None);
                                            Some(0)
                                        } else {
                                            parse_context_window(&v).map(|n| {
                                                let m = model_max_context(model);
                                                let f = n.min(m);
                                                config.context_window = Some(f);
                                                set_user_context_window(Some(f));
                                                f
                                            })
                                        }
                                    }
                                    // reserve: auto clears to default, else parse
                                    1 => {
                                        if ["auto", "clear", "reset", "default"].contains(&v.as_str()) {
                                            config.context_reserve = None;
                                            set_user_reserve_tokens(None);
                                            Some(user_reserve_tokens_for(model_max_context(model)))
                                        } else {
                                            parse_context_window(&v).map(|n| {
                                                config.context_reserve = Some(n);
                                                set_user_reserve_tokens(Some(n));
                                                n
                                            })
                                        }
                                    }
                                    // keep: auto clears, off/0 = summary only, else parse
                                    _ => {
                                        if ["auto", "clear", "reset", "default"].contains(&v.as_str()) {
                                            config.context_keep = None;
                                            set_user_keep_recent_tokens(None);
                                            Some(user_keep_for(model_max_context(model)))
                                        } else if v == "off" || v == "0" {
                                            config.context_keep = Some(0);
                                            set_user_keep_recent_tokens(Some(0));
                                            Some(0)
                                        } else {
                                            parse_context_window(&v).map(|n| {
                                                config.context_keep = Some(n);
                                                set_user_keep_recent_tokens(Some(n));
                                                n
                                            })
                                        }
                                    }
                                };
                                match saved {
                                    Some(_) => {
                                        persist(config);
                                        changed = true;
                                        status = format!("{} updated", setting_labels[idx]);
                                        editing = None;
                                        buf.clear();
                                    }
                                    None => {
                                        status = format!("invalid '{v}' — e.g. 128k, 1m, auto");
                                    }
                                }
                            }
                            KeyCode::Backspace => {
                                buf.pop();
                            }
                            KeyCode::Char(c) => {
                                if buf.len() < 24 {
                                    buf.push(c);
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }
                    match code {
                        KeyCode::Up => sel = sel.saturating_sub(1),
                        KeyCode::Down => sel = (sel + 1).min(2),
                        KeyCode::Esc | KeyCode::Char('q') => return Ok(changed),
                        KeyCode::Enter => {
                            buf = match sel {
                                0 => get_user_context_window()
                                    .map(|n| format_context_length(n))
                                    .unwrap_or_else(|| "auto".to_string()),
                                1 => format_context_length(user_reserve_tokens_for(window)),
                                _ => {
                                    if user_keep_for(window) == 0 {
                                        "off".to_string()
                                    } else {
                                        format_context_length(user_keep_for(window))
                                    }
                                }
                            };
                            status.clear();
                            editing = Some(sel);
                        }
                        _ => {}
                    }
                }
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

/// Rows for the `/gateway` picker: (command, label, status). Platform rows
/// carry an empty command — their action (connect vs disconnect) is decided at
/// Enter time from the enabled state.
pub fn gateway_modal_rows(cfg: &gray_gateway::config::GatewayConfig, running: bool) -> Vec<(String, String, String)> {
    use gray_gateway::config::Platform;
    let mut rows = Vec::new();
    for plat in [Platform::Telegram, Platform::Discord, Platform::Slack] {
        let status = match cfg.platforms.get(&plat) {
            Some(p) if p.enabled => "enabled".to_string(),
            Some(p) if p.token.as_ref().is_some_and(|t| !t.is_empty()) => "disabled — token saved".to_string(),
            _ => "disabled — enter token".to_string(),
        };
        rows.push((String::new(), plat.label().to_string(), status));
    }
    rows.push(("__sep".to_string(), String::new(), String::new()));
    rows.push((
        format!("/gateway {}", if running { "stop" } else { "run" }),
        if running { "Stop gateway" } else { "Start gateway" }.to_string(),
        String::new(),
    ));
    rows.push(("/gateway install".to_string(), "Install systemd service".to_string(), String::new()));
    rows.push(("/gateway uninstall".to_string(), "Remove systemd service".to_string(), String::new()));
    rows.push((
        format!("/gateway autostart {}", if cfg.autostart { "off" } else { "on" }),
        format!("Autostart on launch: {}", if cfg.autostart { "on" } else { "off" }),
        String::new(),
    ));
    rows
}

/// Move a modal selection, skipping `__sep` spacer rows so arrow keys never
/// land on the invisible gap (e.g. between platforms and actions).
fn move_sel(rows: &[(String, String, String)], sel: usize, delta: i32) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let max = rows.len() - 1;
    let mut next = (sel as i32 + delta).clamp(0, max as i32) as usize;
    while rows[next].0 == "__sep" {
        let stepped = (next as i32 + delta.signum()).clamp(0, max as i32) as usize;
        if stepped == next {
            break;
        }
        next = stepped;
    }
    next
}

/// Interactive `/gateway` picker (like `/model`, `/skills`): platforms with
/// enabled state, plus daemon/service actions. Enter on a platform without a
/// saved token opens an inline token input; on a disabled platform with a
/// saved token it re-enables; on an enabled platform it disconnects (token kept).
/// Returns the equivalent command string for the caller to execute.
pub fn run_gateway_modal(bg: Option<&BackgroundSnapshot>, running: bool) -> anyhow::Result<Option<String>> {
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
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::terminal::Clear(crossterm::terminal::ClearType::All), crossterm::cursor::Hide)?;
    let _ = crossterm::terminal::size();
    let backend = CrosstermBackend::new(stdout_handle);
    let mut terminal = Terminal::new(backend)?;

    let box_bg = Color::Rgb(22, 22, 22);
    let accent_peach = Color::Rgb(246, 173, 126);
    let text_dim = Color::Rgb(120, 120, 120);
    let status_ok = Color::Rgb(74, 222, 128);

    let mut sel = 0usize;
    // token-input mode: (platform label, buffer)
    let mut input_for: Option<String> = None;
    let mut input_buf = String::new();
    let bg_snapshot = bg.cloned().unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<Option<String>> {
        loop {
            let cfg = gray_gateway::config::load_gateway_config();
            let rows = gateway_modal_rows(&cfg, running);
            let max_sel = rows.len().saturating_sub(1);
            if sel > max_sel {
                sel = max_sel;
            }
            terminal.draw(|frame| {
                let area = frame.area();
                if area.width < 20 || area.height < 6 {
                    return;
                }
                render_dimmed_background(frame, &bg_snapshot);
                let modal_w = 62.min(area.width.saturating_sub(4)).max(40).min(area.width);
                let modal_h = (rows.len() as u16 + 6).min(area.height.saturating_sub(2)).max(10).min(area.height);
                let modal_x = (area.width.saturating_sub(modal_w)) / 2;
                let modal_y = (area.height.saturating_sub(modal_h)) / 3;
                let modal_rect = Rect::new(modal_x, modal_y, modal_w, modal_h);
                frame.render_widget(Clear, modal_rect);
                frame.render_widget(Block::default().style(Style::default().bg(box_bg)), modal_rect);
                let pad_x = 3u16;
                let inner_w = modal_w.saturating_sub(pad_x * 2);
                let inner = Rect::new(modal_x + pad_x, modal_y + 1, inner_w, modal_h.saturating_sub(2));

                if let Some(plat) = &input_for {
                    // token input mode
                    let title = format!("Gateway — {plat} token");
                    let esc_str = "esc back";
                    let pad_len = (inner.width as usize).saturating_sub(title.chars().count() + esc_str.chars().count());
                    let header_line = Line::from(vec![
                        Span::styled(title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                        Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                    ]);
                    frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));
                    let sub_line = Line::from(Span::styled(
                        "paste or type token  •  enter save  •  esc cancel",
                        Style::default().fg(Color::Rgb(100, 100, 100)).bg(box_bg),
                    ));
                    frame.render_widget(Paragraph::new(sub_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));
                    // Never render the token itself (stream-safe): dots only.
                    let masked = "•".repeat(input_buf.chars().count());
                    let field = format!(" {} {masked}█", icon("arrow"));
                    let field_line = Line::from(Span::styled(field, Style::default().fg(Color::White).bg(box_bg)));
                    frame.render_widget(Paragraph::new(field_line), Rect::new(inner.x, inner.y + 3, inner.width, 1));
                } else {
                    let title_str = "Gateway";
                    let esc_str = "esc";
                    let pad_len = (inner.width as usize).saturating_sub(title_str.chars().count() + esc_str.chars().count());
                    let header_line = Line::from(vec![
                        Span::styled(title_str, Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                        Span::styled(esc_str, Style::default().fg(text_dim).bg(box_bg)),
                    ]);
                    frame.render_widget(Paragraph::new(header_line), Rect::new(inner.x, inner.y, inner.width, 1));
                    let sub_line = Line::from(Span::styled(
                        "~/.gray/gateway.yaml  •  messaging gateway",
                        Style::default().fg(Color::Rgb(100, 100, 100)).bg(box_bg),
                    ));
                    frame.render_widget(Paragraph::new(sub_line), Rect::new(inner.x, inner.y + 1, inner.width, 1));
                    let list_y = inner.y + 3;
                    for (idx, (cmd, label, status)) in rows.iter().enumerate() {
                        let row_y = list_y + idx as u16;
                        if cmd == "__sep" {
                            continue;
                        }
                        let is_selected = idx == sel;
                        let is_platform = cmd.is_empty();
                        let enabled = status == "enabled";
                        if is_selected {
                            let raw_content = format!(" › {label:<22}  {status}");
                            let fill = (inner.width as usize).saturating_sub(raw_content.chars().count());
                            let full = format!("{raw_content}{}", " ".repeat(fill));
                            frame.render_widget(
                                Paragraph::new(Line::from(Span::styled(full, Style::default().fg(Color::Black).bg(accent_peach).add_modifier(Modifier::BOLD)))),
                                Rect::new(inner.x, row_y, inner.width, 1),
                            );
                        } else {
                            let name_span = Span::styled(format!("   {label:<22}  "), Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg));
                            let status_span = if is_platform {
                                Span::styled(status.clone(), Style::default().fg(if enabled { status_ok } else { text_dim }).bg(box_bg))
                            } else {
                                Span::styled(status.clone(), Style::default().fg(text_dim).bg(box_bg))
                            };
                            frame.render_widget(Paragraph::new(Line::from(vec![name_span, status_span])), Rect::new(inner.x, row_y, inner.width, 1));
                        }
                    }
                    let footer_line = Line::from(vec![
                        Span::styled("↑↓ ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled("navigate    ", Style::default().fg(text_dim).bg(box_bg)),
                        Span::styled("enter ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD).bg(box_bg)),
                        Span::styled("select", Style::default().fg(text_dim).bg(box_bg)),
                    ]);
                    frame.render_widget(Paragraph::new(footer_line), Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1));
                }
            })?;
            if !poll(Duration::from_millis(100))? {
                continue;
            }
            let ev = read()?;
            if let Some(plat) = input_for.clone() {
                match ev {
                    Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                    Event::Paste(pasted) => {
                        // bracketed paste: tokens never contain whitespace —
                        // keep the first run so trailing newlines can't sneak in.
                        if let Some(tok) = pasted.split_whitespace().next() {
                            input_buf.push_str(tok);
                        }
                    }
                    Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                        KeyCode::Esc => {
                            input_for = None;
                            input_buf.clear();
                        }
                        KeyCode::Enter if !input_buf.trim().is_empty() => {
                            let cmd = format!("/gateway connect {plat} {}", input_buf.trim());
                            return Ok(Some(cmd));
                        }
                        KeyCode::Enter => {}
                        KeyCode::Backspace => {
                            input_buf.pop();
                        }
                        KeyCode::Char(c) => input_buf.push(c),
                        _ => {}
                    },
                    _ => {}
                }
                continue;
            }
            match ev {
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => match code {
                    KeyCode::Char('p') => sel = move_sel(&rows, sel, -1),
                    KeyCode::Char('n') => sel = move_sel(&rows, sel, 1),
                    _ => {}
                },
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                    KeyCode::Up => sel = move_sel(&rows, sel, -1),
                    KeyCode::Down => sel = move_sel(&rows, sel, 1),
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Enter => {
                        let (cmd, label, _) = &rows[sel];
                        if cmd == "__sep" {
                            continue;
                        }
                        if cmd.is_empty() {
                            // platform row: no saved token → token input,
                            // disabled with token → re-enable, enabled → disconnect
                            let Ok(plat) = label.parse::<gray_gateway::config::Platform>() else { continue; };
                            let cfg_now = gray_gateway::config::load_gateway_config();
                            let entry = cfg_now.platforms.get(&plat);
                            let enabled = entry.is_some_and(|p| p.enabled);
                            if enabled {
                                return Ok(Some(format!("/gateway disconnect {plat}")));
                            }
                            if entry.and_then(|p| p.token.as_ref()).is_some_and(|t| !t.is_empty()) {
                                return Ok(Some(format!("/gateway enable {plat}")));
                            }
                            input_for = Some(label.clone());
                            input_buf.clear();
                        } else {
                            return Ok(Some(cmd.clone()));
                        }
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {
                    let _ = terminal.clear();
                }
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

#[cfg(test)]
mod gateway_modal_tests {
    use super::*;

    #[test]
    fn gateway_modal_rows_reflect_state() {
        use gray_gateway::config::{GatewayConfig, Platform, PlatformConfig};
        let mut platforms = std::collections::HashMap::new();
        platforms.insert(Platform::Telegram, PlatformConfig { enabled: true, token: Some("x".into()), ..Default::default() });
        let cfg = GatewayConfig { platforms, ..Default::default() };
        let rows = gateway_modal_rows(&cfg, false);
        assert_eq!(rows[0], (String::new(), "Telegram".into(), "enabled".into()));
        assert_eq!(rows[1].2, "disabled — enter token");
        assert!(rows.iter().any(|(c, l, _)| c == "/gateway run" && l == "Start gateway"));
        let rows = gateway_modal_rows(&cfg, true);
        assert!(rows.iter().any(|(c, l, _)| c == "/gateway stop" && l == "Stop gateway"));
        assert!(rows.iter().any(|(c, _, _)| c == "/gateway install"));
        assert!(rows.iter().any(|(c, _, _)| c == "/gateway uninstall"));
    }

    #[test]
    fn gateway_modal_rows_show_saved_token() {
        use gray_gateway::config::{GatewayConfig, Platform, PlatformConfig};
        let mut platforms = std::collections::HashMap::new();
        platforms.insert(Platform::Discord, PlatformConfig { enabled: false, token: Some("saved".into()), ..Default::default() });
        let cfg = GatewayConfig { platforms, ..Default::default() };
        let rows = gateway_modal_rows(&cfg, false);
        assert_eq!(rows[1], (String::new(), "Discord".into(), "disabled — token saved".into()));
    }

    #[test]
    fn move_sel_skips_sep() {
        let rows = vec![
            ("".to_string(), "Telegram".to_string(), String::new()),
            ("".to_string(), "Discord".to_string(), String::new()),
            ("".to_string(), "Slack".to_string(), String::new()),
            ("__sep".to_string(), String::new(), String::new()),
            ("/gateway run".to_string(), "Start gateway".to_string(), String::new()),
        ];
        // Slack -> one Down lands on Start gateway, not the invisible gap.
        assert_eq!(move_sel(&rows, 2, 1), 4);
        // ...and back up in one press.
        assert_eq!(move_sel(&rows, 4, -1), 2);
        assert_eq!(move_sel(&rows, 0, -1), 0);
        assert_eq!(move_sel(&rows, 4, 1), 4);
    }
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
    crossterm::execute!(stdout_handle, EnterAlternateScreen, crossterm::terminal::Clear(crossterm::terminal::ClearType::All), crossterm::cursor::Hide)?;
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

