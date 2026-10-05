//! Provider picker modal (split from `setup`).

use super::*;

/// Direct `/model <id>` validation for the live connection — same candidate
/// list as the picker: `provider/models` for plugin connections, a live
/// `/models` fetch otherwise.
pub(crate) fn provider_models_for_config(config: &Config) -> (String, Vec<(String, String)>) {
    let (name, _key, plugin) = picker_scope(config);
    let models = match plugin {
        Some(installed) => super::context::fetch_plugin_provider_models(installed),
        None => picker_models_for(&config.base_url, config.api_key.as_deref()),
    };
    (name, models)
}

/// The provider behind the live config, as the picker sees it. Plugin
/// connections own their list via the sidecar's `provider/models` RPC —
/// the declared base_url is a shared relay placeholder an HTTP fetch can
/// never satisfy — so the cache key is `plugin:<provider_id>` (see
/// [`super::catalog::plugin_models_key`]) and the title is the plugin's own
/// name rather than a catalog lookup that can only ever say "Custom".
/// Returns (title, `provider_models.json` key, installed plugin provider).
pub(crate) fn picker_scope(
    config: &Config,
) -> (String, String, Option<crate::providers::InstalledProvider>) {
    if config.uses_plugin_credentials()
        && let Ok(home) = super::catalog::gray_home()
        && let Some(installed) = crate::providers::ProviderRegistry::load_cached(&home)
            .installed()
            .into_iter()
            .find(|p| p.provider_id() == config.provider_id)
    {
        let key = super::catalog::plugin_models_key(&installed.provider_id());
        return (installed.provider.name.clone(), key, Some(installed));
    }
    let catalog = load_catalog().unwrap_or_default();
    let name = catalog
        .iter()
        .find(|(_, p)| p.base_url == config.base_url)
        .map(|(_, p)| p.name.clone())
        .unwrap_or_else(|| "Custom".to_string());
    (name, config.base_url.clone(), None)
}

/// Models we already know without touching the network: the last fetched
/// list for this provider, persisted to disk after every successful fetch.
/// The picker paints from this immediately and refreshes in the background —
/// opening a modal should never wait on a round-trip.
/// `list_key` is the `provider_models.json` slot — a `plugin:` pseudo-key
/// for plugin providers; `sort_base` stays the configured base_url so the
/// current model and recents resolve under their normal key.
pub(crate) fn saved_models_for(list_key: &str, sort_base: &str) -> Vec<(String, String)> {
    let mut models = super::context::load_provider_model_list(list_key);
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(sort_base, &mut models);
    }
    models
}

/// Effort words that may ride at the end of a model id (`swe-2-max`):
/// the tier vocabulary minus `off` — a `-none` row is its own product,
/// never a level of the base. Mirrors the devin-sub `EFFORT_VARIANTS`
/// list and the clamp's known levels.
pub(crate) const VARIANT_EFFORT_WORDS: &[&str] =
    &["minimal", "low", "medium", "high", "xhigh", "max"];

/// Leaf segments naming a fast-serving variant at the end of a model id:
/// `-fast` on claude/swe-family rows, `-priority` on gpt-family rows
/// (the devin-sub catalog labels them "… Fast" / "… Thinking Fast").
/// Compared case-insensitively — upstream catalogs mix case (`GLM-5.2-Fast`).
pub(crate) const FAST_SUFFIXES: &[&str] = &["fast", "priority"];

/// `Some((base, tier))` when `model` reads as `<base>-<effort>`: the split
/// itself, checked only against the effort vocabulary — no row lookup.
fn effort_variant_shape(model: &str) -> Option<(&str, &str)> {
    let (base, tier) = model.rsplit_once('-')?;
    (!base.is_empty() && VARIANT_EFFORT_WORDS.contains(&tier)).then_some((base, tier))
}

/// A stored or typed id like `swe-2-max`: when the provider's known rows
/// declare `<base>` but not the id itself, the trailing word is the tier —
/// the family collapsed into one picker row whose `reasoning_efforts`
/// own the level. A declared row always wins (`qwen3-max` listed as its
/// own model is never split); `None` when no declared base matches.
pub(crate) fn split_effort_variant(
    model: &str,
    known: &[(String, String)],
) -> Option<(String, String)> {
    let (base, tier) = effort_variant_shape(model)?;
    if known.iter().any(|(id, _)| id == model) {
        return None;
    }
    known
        .iter()
        .any(|(id, _)| id == base)
        .then(|| (base.to_string(), tier.to_string()))
}

/// `Some((stem, suffix))` when `model` ends in a [`FAST_SUFFIXES`] leaf AND
/// the stem resolves back to a declared base — directly (`swe-1-6-fast` →
/// `swe-1-6`) or through one effort tier (`claude-opus-5-5-high-fast` →
/// `claude-opus-5-5-high` → `claude-opus-5-5`). Unlike the effort split a
/// declared `...-fast` row still decomposes: the fast row IS a variant of
/// its base, and keeping base+tier+flag separate preserves the effort knob.
fn split_fast_variant(model: &str, known: &[(String, String)]) -> Option<(String, &'static str)> {
    let (stem, leaf) = model.rsplit_once('-')?;
    if stem.is_empty() {
        return None;
    }
    let suffix = FAST_SUFFIXES
        .iter()
        .find(|s| leaf.eq_ignore_ascii_case(s))?;
    // The stem's canonical form: a declared row wins (its id keeps the
    // catalog's case); an effort-bearing stem resolves through the tier
    // split.
    let stem = match known.iter().find(|(id, _)| id.eq_ignore_ascii_case(stem)) {
        Some((id, _)) => id.clone(),
        None if split_effort_variant(stem, known).is_some() => stem.to_string(),
        None => return None,
    };
    Some((stem, *suffix))
}

/// Decompose a catalog id into `(base, tier, fast)` — a `-fast`/`-priority`
/// leaf peels first (even on declared rows), then the standard
/// `<base>-<effort>` split applies to the stem. Either half may be absent:
/// `swe-1-6-fast` is base+fast with no tier, `swe-2-max` is base+tier with
/// no fast. `None` when nothing decomposes.
pub(crate) fn decompose_model_variant(
    model: &str,
    known: &[(String, String)],
) -> Option<(String, Option<String>, bool)> {
    let (work, fast) = match split_fast_variant(model, known) {
        Some((stem, _)) => (stem, true),
        None => (model.to_string(), false),
    };
    let (base, tier) = split_effort_variant(&work, known)
        .map(|(b, t)| (b, Some(t)))
        .unwrap_or((work, None));
    (fast || tier.is_some()).then_some((base, tier, fast))
}

/// The provider's fast-serving sibling of `model` at `effort`, spelled the
/// way the catalog declares it: `claude-opus-5-5` + `high` →
/// `claude-opus-5-5-high-fast`; `gpt-6-sol` + `off` →
/// `gpt-6-sol-none-priority`; `swe-1-6` + anything → `swe-1-6-fast`.
/// `None` when the catalog has no fast row for the target — the caller
/// then sends `model` unchanged. An id already carrying the suffix is
/// already the fast product and never re-composes.
pub(crate) fn compose_fast_model(
    model: &str,
    effort: Option<&str>,
    known: &[(String, String)],
) -> Option<String> {
    if split_fast_variant(model, known).is_some() {
        return None;
    }
    let mut candidates: Vec<String> = Vec::with_capacity(4);
    let level = match effort {
        Some("off") | Some("") | None => Some("none"),
        Some(l) => Some(l),
    };
    if let Some(l) = level {
        for s in FAST_SUFFIXES {
            candidates.push(format!("{model}-{l}-{s}"));
        }
    }
    for s in FAST_SUFFIXES {
        candidates.push(format!("{model}-{s}"));
    }
    for cand in candidates {
        if let Some((id, _)) = known.iter().find(|(id, _)| id.eq_ignore_ascii_case(&cand)) {
            return Some(id.clone());
        }
    }
    None
}

/// The rows canonicalization may split against: the persisted picker
/// list, plus — for a plugin connection whose stored id still reads as a
/// variant it can't confirm — one live `provider/models` fetch (the
/// persisted list may predate the family collapse, or never have run).
/// Only the variant pattern pays the fetch; a clean id returns instantly.
/// Status-line effort chip: `high` normally, `high·fast` while the fast
/// model variant is on. Display only — the wire id composes in build_agent.
pub(crate) fn effort_chip(eff: &str, config: &Config) -> String {
    if config.fast_mode == Some(true) && eff != "off" {
        format!("{eff}·fast")
    } else {
        eff.to_string()
    }
}

pub(crate) fn canonical_model_rows(config: &Config) -> Vec<(String, String)> {
    let (_, list_key, plugin) = picker_scope(config);
    let known = saved_models_for(&list_key, &config.base_url);
    let stale_variant = config
        .model
        .as_deref()
        .is_some_and(|m| effort_variant_shape(m).is_some() && !known.iter().any(|(id, _)| id == m));
    match (stale_variant, plugin) {
        (true, Some(installed)) => {
            let live = super::context::fetch_plugin_provider_models(installed);
            if live.is_empty() { known } else { live }
        }
        _ => known,
    }
}

/// Adopt a `<base>-<tier>` id as `base` + `tier` effort: a collapsed
/// family row owns the level through `/thinking`, so a stale variant id
/// (`swe-2-max` saved before the collapse) shows the real model, makes
/// the effort knob honest — the variant IS the level it runs at — and
/// keeps the upstream call identical (the plugin maps `base`+`tier` back
/// to the same native id). Returns the applied `(base, tier)`; the model
/// and this target's remembered effort persist under the canonical key.
/// `GRAY_THINKING_EFFORT` stays a user override: the model still
/// canonicalizes but the env level wins.
pub(crate) fn canonicalize_effort_variant(
    config: &mut Config,
    known: &[(String, String)],
) -> Option<(String, Option<String>, bool)> {
    let model = config.model.clone()?;
    let (base, tier, fast) = decompose_model_variant(&model, known)?;
    config.model = Some(base.clone());
    if let Some(t) = &tier
        && std::env::var_os("GRAY_THINKING_EFFORT").is_none()
    {
        config.thinking_effort = Some(t.clone());
    }
    if fast && std::env::var_os("GRAY_FAST").is_none() {
        config.fast_mode = Some(true);
    }
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = lock_saved_config_at(&path).ok();
        let mut saved = load_saved_config_at(&path);
        saved.model = Some(base.clone());
        if let Some(t) = &tier {
            saved.remember_effort(
                &crate::setup::effort_memory_key(&config.provider_id, &config.base_url, &base),
                t,
            );
        }
        saved.thinking_effort = config.thinking_effort.clone();
        if fast {
            saved.fast_mode = Some(true);
        }
        let _ = save_saved_config_at(&path, &saved);
    }
    Some((base, tier, fast))
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

/// Connect-modal list: the last good list now, refreshed in the background.
/// Only a provider never fetched before waits on the network (up to 3s per
/// probed endpoint), which is what made /connect feel stuck.
pub(super) fn cached_models_for(base_url: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let cached = saved_models_for(base_url, base_url);
    if cached.is_empty() {
        return picker_models_for(base_url, api_key);
    }
    let (base, key) = (base_url.to_string(), api_key.map(str::to_string));
    // Detached: the fetch persists the fresh list to disk for the next open.
    std::thread::spawn(move || super::context::fetch_live_provider_models(&base, key.as_deref()));
    cached
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

    // The provider identity is local work; only the model list needs I/O,
    // and that no longer blocks the first frame. Plugin connections scope
    // by provider id — the placeholder base can never serve a list.
    let (item_name, list_key, plugin) = picker_scope(config);
    let mut models = saved_models_for(&list_key, &config.base_url);
    let (mut current_id, mut recent_ids) = recent_head_for(&config.base_url);
    let (refresh_tx, refresh_rx) = std::sync::mpsc::channel::<Vec<(String, String)>>();
    // Detached on purpose: a late reply lands in a channel nobody reads.
    let _refresh = {
        let plugin = plugin.clone();
        let base_url = config.base_url.clone();
        let api_key = config.api_key.clone();
        let tx = refresh_tx;
        std::thread::spawn(move || {
            let live = match plugin {
                Some(installed) => super::context::fetch_plugin_provider_models(installed),
                None => super::context::fetch_live_provider_models(&base_url, api_key.as_deref()),
            };
            let _ = tx.send(live);
        })
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
                let title_str = format!("Select model \u{2014} {}", item_name);
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
                        // An empty list has nothing to select. With a model
                        // already configured, Enter dismisses — the old
                        // literal "default" fallback silently clobbered the
                        // live model whenever a provider answered with no
                        // list (plugin providers hit exactly that).
                        if filtered_models.is_empty() && filter.is_empty() && config.model.is_some()
                        {
                            return Ok(false);
                        }
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
                            // Reached only with nothing configured: first
                            // run needs *a* value to continue.
                            "default".to_string()
                        };

                        config.model = Some(chosen_model.clone());

                        // A variant pick (`base-tier`, `base-tier-fast`)
                        // decomposes: the tier/fast it names wins over the
                        // remembered values applied below.
                        let variant = canonicalize_effort_variant(config, &models);
                        let path = saved_config_path()?;
                        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
                        let mut saved = load_saved_config_at(&path);
                        saved.base_url = Some(config.base_url.clone());
                        saved.model = config.model.clone();
                        // The model's own remembered effort (or default) —
                        // not whatever level the last model left behind.
                        if variant.as_ref().and_then(|(_, t, _)| t.as_ref()).is_none() {
                            super::provider_auth::apply_connection_effort(config, &mut saved);
                        }
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
