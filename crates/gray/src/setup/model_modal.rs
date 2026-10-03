//! Provider picker modal (split from `setup`).

use super::*;

/// Provider id + display name resolved from the catalog by base URL,
/// plus the known-model list behind the picker (live `/models`).
/// Shared by the picker and direct `/model <id>` validation so both
/// accept exactly the same ids.
pub(crate) fn provider_models_for(
    base_url: &str,
    api_key: Option<&str>,
) -> (String, String, Vec<(String, String)>) {
    let catalog = load_catalog().unwrap_or_default();
    let (item_id, item_name) =
        if let Some((pid, p)) = catalog.iter().find(|(_, p)| p.base_url == base_url) {
            (pid.clone(), p.name.clone())
        } else {
            ("custom".to_string(), "Custom".to_string())
        };
    let models = picker_models_for(base_url, api_key);
    (item_id, item_name, models)
}

/// Models we already know without touching the network: the last fetched
/// list for this provider, persisted to disk after every successful fetch.
/// The picker paints from this immediately and refreshes in the background —
/// opening a modal should never wait on an HTTP round-trip.
pub(super) fn saved_models_for(base_url: &str) -> Vec<(String, String)> {
    let mut models = super::context::load_provider_model_list(base_url);
    // Subscription routes have no /models endpoint: seed the pinned catalog
    // so the picker offers them before (and without) any fetch.
    for id in gray_provider::claude_subscription::pinned_ids() {
        let full = format!("{}{id}", gray_provider::claude_subscription::MODEL_PREFIX);
        if !models.iter().any(|(m, _)| m == &full) {
            models.push((full, format!("Claude {id} (subscription)")));
        }
    }
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(base_url, &mut models);
    }
    models
}

/// The saved current model + recents for this provider: the same source
/// `sort_models` parks first, so the count below is the divider position.
fn recent_head_for(base_url: &str) -> (Option<String>, Vec<String>) {
    let Ok(path) = saved_config_path() else {
        return (None, Vec::new());
    };
    let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
    let saved = load_saved_config_at(&path);
    let base = normalize_custom_base_url(base_url);
    let current = saved
        .base_url
        .as_deref()
        .filter(|url| normalize_custom_base_url(url) == base)
        .and(saved.model.clone());
    let recent = saved.recent_models.get(&base).cloned().unwrap_or_default();
    (current, recent)
}

/// Leading run of the (already sorted) list that is the current model or a
/// recent one. 0 or full-length means no divider.
fn recent_prefix_len<'a>(
    current: Option<&str>,
    recent: &[String],
    ids: impl Iterator<Item = &'a str>,
) -> usize {
    ids.take_while(|id| Some(*id) == current || recent.iter().any(|m| m.as_str() == *id))
        .count()
}

/// A picker row: a model by index into the filtered list, or the
/// recent/all divider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Model(usize),
    Divider,
}

/// Selection may never rest on the divider: after any move, step once more
/// in the direction of travel. The divider always has models on both sides
/// (it only renders when `0 < sep < len`), so one step suffices.
fn skip_divider(rows: &[Row], sel: usize, down: bool) -> usize {
    if rows.get(sel) == Some(&Row::Divider) {
        if down {
            sel.saturating_add(1).min(rows.len().saturating_sub(1))
        } else {
            sel.saturating_sub(1)
        }
    } else {
        sel
    }
}

/// Merge a live list into what the picker already shows, keeping the
/// saved ordering. Same result as [`picker_models_for`], minus the wait.
fn merge_models(base_url: &str, live: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut models = live;
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(base_url, &mut models);
    }
    models
}

/// Shared ordering for /model and every connect-modal model list.
pub(super) fn picker_models_for(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let mut models = fetch_live_provider_models(base_url, api_key);
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(base_url, &mut models);
    }
    models
}

pub fn run_model_modal(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};
    use std::time::Duration;

    // The provider identity is local catalog work; only the model list
    // needs the network, and that no longer blocks the first frame.
    let catalog = load_catalog().unwrap_or_default();
    let (item_id, item_name) =
        if let Some((pid, p)) = catalog.iter().find(|(_, p)| p.base_url == config.base_url) {
            (pid.clone(), p.name.clone())
        } else {
            ("custom".to_string(), "Custom".to_string())
        };
    let mut models = saved_models_for(&config.base_url);
    let (mut current_id, mut recent_ids) = recent_head_for(&config.base_url);
    let (refresh_tx, refresh_rx) = std::sync::mpsc::channel::<Vec<(String, String)>>();
    // Detached on purpose: a late reply lands in a channel nobody reads.
    let _refresh = {
        let base_url = config.base_url.clone();
        let api_key = config.api_key.clone();
        let tx = refresh_tx;
        std::thread::spawn(move || {
            let live = super::context::fetch_live_provider_models(&base_url, api_key.as_deref());
            let _ = tx.send(live);
        })
    };

    let item = ConnectItem {
        id: item_id.clone(),
        name: item_name.clone(),
        sublabel: String::new(),
        base_url: config.base_url.clone(),
        no_auth: false,
        auth: crate::setup::ConnectAuth::ApiKey,
    };

    let (_session, mut terminal) = super::open_modal()?;

    let box_bg = crate::theme::theme().surface_bg;
    let accent_peach = crate::theme::theme().accent;
    let text_dim = crate::theme::theme().text_dim;

    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;

    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        let mut refreshing = true;
        loop {
            // A finished refresh updates the open list in place; the modal
            // was already usable while it ran.
            if refreshing && let Ok(live) = refresh_rx.try_recv() {
                refreshing = false;
                if !live.is_empty() {
                    models = merge_models(&config.base_url, live);
                    (current_id, recent_ids) = recent_head_for(&config.base_url);
                }
            }
            let filtered_models: Vec<&(String, String)> = models
                .iter()
                .filter(|(m_id, m_name)| {
                    let f = filter.to_lowercase();
                    f.is_empty()
                        || m_id.to_lowercase().contains(&f)
                        || m_name.to_lowercase().contains(&f)
                })
                .collect();
            // One divider after the recent head; selection skips it.
            let sep = recent_prefix_len(
                current_id.as_deref(),
                &recent_ids,
                filtered_models.iter().map(|(id, _)| id.as_str()),
            );
            let div = (sep > 0 && sep < filtered_models.len()).then_some(sep);
            let mut rows: Vec<Row> = Vec::with_capacity(filtered_models.len() + 1);
            for i in 0..filtered_models.len() {
                if div == Some(i) {
                    rows.push(Row::Divider);
                }
                rows.push(Row::Model(i));
            }

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
                // Say so while the live list is still loading: a short list
                // that is still filling in must not read as the whole truth.
                let hint = if refreshing {
                    "  refreshing\u{2026}"
                } else {
                    ""
                };
                let pad_len = (inner.width as usize).saturating_sub(
                    title_str.chars().count() + esc_str.chars().count() + hint.chars().count(),
                );
                let header_line = Line::from(vec![
                    Span::styled(
                        title_str,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                            .bg(box_bg),
                    ),
                    Span::styled(" ".repeat(pad_len), Style::default().bg(box_bg)),
                    Span::styled(hint, Style::default().fg(text_dim).bg(box_bg)),
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
                            Style::default()
                                .fg(crate::theme::theme().text_dim)
                                .bg(box_bg),
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
                    let safe_sel =
                        skip_divider(&rows, sel.min(rows.len().saturating_sub(1)), false);
                    if safe_sel < scroll_top {
                        scroll_top = safe_sel;
                    } else if safe_sel >= scroll_top + list_h {
                        scroll_top = safe_sel.saturating_sub(list_h.saturating_sub(1));
                    }

                    for r in 0..list_h {
                        let idx = scroll_top + r;
                        let Some(row) = rows.get(idx) else {
                            break;
                        };
                        if *row == Row::Divider {
                            let label = "─ recent ";
                            let fill = (inner.width as usize).saturating_sub(label.chars().count());
                            let div_line = Line::from(Span::styled(
                                format!("{label}{}", "─".repeat(fill)),
                                Style::default().fg(text_dim).bg(box_bg),
                            ));
                            frame.render_widget(
                                Paragraph::new(div_line),
                                Rect::new(inner.x, list_y + r as u16, inner.width, 1),
                            );
                            continue;
                        }
                        let Row::Model(mi) = row else {
                            continue;
                        };

                        let (m_id, m_name) = filtered_models[*mi];
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
                                display_name,
                                Style::default()
                                    .fg(Color::White)
                                    .add_modifier(Modifier::BOLD)
                                    .bg(box_bg),
                            );
                            let sub_span = Span::styled(
                                sub,
                                Style::default()
                                    .fg(crate::theme::theme().text_dim)
                                    .bg(box_bg),
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

            if rows.is_empty() {
                sel = 0;
            } else {
                sel = skip_divider(&rows, sel.min(rows.len() - 1), false);
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
                    KeyCode::Char('p') => sel = skip_divider(&rows, sel.saturating_sub(1), false),
                    KeyCode::Char('n') if !rows.is_empty() => {
                        sel = skip_divider(&rows, (sel + 1).min(rows.len() - 1), true);
                    }
                    _ => {}
                },
                Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => match code {
                    KeyCode::Up => sel = skip_divider(&rows, sel.saturating_sub(1), false),
                    KeyCode::Down => {
                        if !rows.is_empty() {
                            sel = skip_divider(&rows, (sel + 1).min(rows.len() - 1), true);
                        }
                    }
                    KeyCode::PageUp => sel = skip_divider(&rows, sel.saturating_sub(8), false),
                    KeyCode::PageDown => {
                        if !rows.is_empty() {
                            sel = skip_divider(&rows, (sel + 8).min(rows.len() - 1), true);
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
                        let picked: Option<String> = match rows.get(sel) {
                            Some(Row::Model(mi)) => {
                                filtered_models.get(*mi).map(|(m_id, _)| m_id.clone())
                            }
                            _ => None,
                        };
                        let chosen_model = if let Some(id) = picked {
                            id
                        } else if !filter.is_empty() {
                            // Canonicalize known ids (case/tail); unknown
                            // filters still fall through as custom models —
                            // the picker keeps its "Use custom model" path.
                            // Direct `/model <id>` callers must use
                            // `validate_direct_model_id` and reject `Err`.
                            validate_direct_model_id(filter.trim(), &models)
                                .unwrap_or_else(|_| filter.trim().to_string())
                        } else {
                            "default".to_string()
                        };

                        config.model = Some(chosen_model.clone());

                        let path = saved_config_path()?;
                        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
                        let mut saved = load_saved_config_at(&path);
                        saved.base_url = Some(config.base_url.clone());
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

    result
}

/// Validates a directly-typed `/model <id>` against the picker's known list
/// (live `/models` + catalog snapshot behind `fetch_live_provider_models`).
/// Exact id (or display name) wins; case-insensitive and unique `provider/`
/// tail matches canonicalize. Unknown ids are rejected with a hint mirroring
/// the picker empty-state (`type /model to browse ...`) instead of being
/// silently accepted until the first prompt fails. Empty known-list fails
/// open (offline/custom endpoints like Ollama accept any id).
pub(crate) fn validate_direct_model_id(
    raw: &str,
    models: &[(String, String)],
) -> Result<String, String> {
    let input = raw.trim();
    if input.is_empty() {
        return Err("usage: /model <model-id> — type /model to browse models".to_string());
    }
    // Subscription routes never appear in the HTTP known-list (no /models
    // endpoint): validate against the provider's pinned table instead. The
    // builder re-checks at switch time; this keeps the direct path and the
    // picker agreeing on the same ids.
    if let Some(native) = input.strip_prefix(gray_provider::claude_subscription::MODEL_PREFIX) {
        return match gray_provider::claude_subscription::native_model(native) {
            Ok(_) => Ok(input.to_string()),
            Err(e) => Err(format!("unknown model '{input}': {e}")),
        };
    }
    if models.is_empty() {
        return Ok(input.to_string());
    }
    if let Some((id, _)) = models.iter().find(|(id, _)| id == input) {
        return Ok(id.clone());
    }
    let lower = input.to_lowercase();
    if let Some((id, _)) = models.iter().find(|(id, _)| id.to_lowercase() == lower) {
        return Ok(id.clone());
    }
    if let Some((id, _)) = models.iter().find(|(_, name)| name.to_lowercase() == lower) {
        return Ok(id.clone());
    }
    if !input.contains('/') {
        let tails: Vec<&(String, String)> = models
            .iter()
            .filter(|(id, _)| {
                id.rsplit('/')
                    .next()
                    .is_some_and(|t| t.to_lowercase() == lower)
            })
            .collect();
        if tails.len() == 1 {
            return Ok(tails[0].0.clone());
        }
        if tails.len() > 1 {
            let mut ids: Vec<&str> = tails.iter().map(|(id, _)| id.as_str()).collect();
            ids.sort();
            let shown = ids.iter().take(5).copied().collect::<Vec<_>>().join(", ");
            return Err(format!(
                "ambiguous model '{input}' — matches {} models ({shown}{}); type /model to pick",
                ids.len(),
                if ids.len() > 5 { ", …" } else { "" },
            ));
        }
    }
    Err(format!(
        "unknown model '{input}' — no match; type /model to browse {} models",
        models.len(),
    ))
}

#[path = "model_modal_tests.rs"]
#[cfg(test)]
mod tests;
