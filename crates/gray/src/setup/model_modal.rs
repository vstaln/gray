use super::*;

pub(crate) fn provider_models_for_config(config: &Config) -> (String, Vec<(String, String)>) {
    let (name, _key, plugin) = picker_scope(config);
    let models = match plugin {
        Some(installed) => super::context::fetch_plugin_provider_models(installed),
        None => picker_models_for(&config.base_url, config.api_key.as_deref()),
    };
    (name, models)
}

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

pub(crate) fn saved_models_for(list_key: &str, sort_base: &str) -> Vec<(String, String)> {
    let mut models = super::context::load_provider_model_list(list_key);
    if let Ok(path) = saved_config_path() {
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        load_saved_config_at(&path).sort_models(sort_base, &mut models);
    }
    models
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DropState {
    row: String,
    slot: usize,
    sel: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickLevel {
    List,
    Slot(usize),
}

fn enter_level(level: PickLevel, slots: usize) -> (PickLevel, bool) {
    match level {
        PickLevel::List if slots > 0 => (PickLevel::Slot(0), false),
        PickLevel::List => (PickLevel::List, true),
        PickLevel::Slot(i) if i + 1 < slots => (PickLevel::Slot(i + 1), false),
        PickLevel::Slot(_) => (PickLevel::List, true),
    }
}

fn esc_level(level: PickLevel) -> Option<PickLevel> {
    match level {
        PickLevel::List => None,
        PickLevel::Slot(0) => Some(PickLevel::List),
        PickLevel::Slot(i) => Some(PickLevel::Slot(i - 1)),
    }
}

fn option_parts(
    shape: &super::variants::RowShape,
    cur: &super::variants::Parts,
    slot: usize,
    option: &str,
) -> Option<super::variants::Parts> {
    let key = &shape.slots.get(slot)?.key;
    shape.option_parts(cur, key, option)
}

fn step_option(
    shape: &super::variants::RowShape,
    cur: &super::variants::Parts,
    slot: usize,
    from: usize,
    down: bool,
) -> usize {
    let Some(s) = shape.slots.get(slot) else {
        return from;
    };
    let ok = |i: usize| option_parts(shape, cur, slot, &s.options[i].id).is_some();
    let mut i = from;
    loop {
        let next = if down {
            if i + 1 >= s.options.len() {
                return from;
            }
            i + 1
        } else {
            if i == 0 {
                return from;
            }
            i - 1
        };
        if ok(next) {
            return next;
        }
        i = next;
    }
}

fn open_option(
    shape: &super::variants::RowShape,
    cur: &super::variants::Parts,
    slot: usize,
) -> usize {
    let Some(s) = shape.slots.get(slot) else {
        return 0;
    };
    let servable = |i: &usize| option_parts(shape, cur, slot, &s.options[*i].id).is_some();
    s.options
        .iter()
        .position(|o| cur.get(&s.key) == Some(&o.id))
        .filter(servable)
        .or_else(|| (0..s.options.len()).find(servable))
        .unwrap_or(0)
}

pub fn run_model_modal(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
    focus: Option<&str>,
) -> anyhow::Result<bool> {
    let (_session, mut terminal) = super::open_modal()?;
    model_picker_loop(&mut terminal, config, bg, focus)
}

pub(crate) fn model_picker_loop(
    terminal: &mut super::ModalTerminal,
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
    focus: Option<&str>,
) -> anyhow::Result<bool> {
    use crossterm::event::{Event, KeyEvent, KeyEventKind, poll, read};
    use std::time::Duration;

    let (item_name, list_key, plugin) = picker_scope(config);
    let mut models = saved_models_for(&list_key, &config.base_url);
    let (mut current_id, mut recent_ids) = recent_head_for(&config.base_url);
    let (refresh_tx, refresh_rx) = std::sync::mpsc::channel::<Vec<(String, String)>>();

    let _refresh = (models.is_empty() || model_refresh_due(&list_key)).then(|| {
        let plugin = plugin.clone();
        let list_key = list_key.clone();
        let base_url = config.base_url.clone();
        let api_key = config.api_key.clone();
        let tx = refresh_tx;
        std::thread::spawn(move || {
            let live = fetch_models_live(&list_key, plugin, &base_url, api_key.as_deref());
            let _ = tx.send(live);
        })
    });

    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;

    let mut place_on_current = true;
    let focus = focus.map(str::to_string);

    let effort_saved = saved_config_path()
        .map(|path| {
            let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
            load_saved_config_at(&path)
        })
        .unwrap_or_default();
    let mut efforts = RowEfforts::default();
    let mut toggles = PickerToggles::from_config(config);

    let mut drop: Option<DropState> = None;

    let curated = crate::setup::active_connect_id(config)
        .and_then(|id| super::context::curated_recommended(&id));

    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let result = (|| -> anyhow::Result<bool> {
        let mut refreshing = _refresh.is_some();
        let mut painted = false;
        loop {
            if refreshing && let Ok(live) = refresh_rx.try_recv() {
                refreshing = false;
                if !live.is_empty() {
                    models = merge_models(&config.base_url, live);
                    (current_id, recent_ids) = recent_head_for(&config.base_url);
                }
            }
            let f = filter.to_lowercase();
            let matched: Vec<&(String, String)> = models
                .iter()
                .filter(|(m_id, m_name)| {
                    f.is_empty()
                        || m_id.to_lowercase().contains(&f)
                        || m_name.to_lowercase().contains(&f)
                })
                .collect();

            let lead = matched.iter().filter(|(id, _)| is_composite_id(id)).count();
            let mut ordered = composites_first(&matched);
            let mut plain = ordered.split_off(lead);

            let sep = recent_prefix_len(
                current_id.as_deref(),
                &recent_ids,
                plain.iter().map(|(id, _)| id.as_str()),
            );

            let mut all_head = false;
            let mut div_at = sep;
            if sep == 0 && f.is_empty() {
                let head: Vec<String> = super::recommend::recommended_ids(
                    &models,
                    curated.as_deref(),
                    super::context::model_meta,
                    RECOMMENDED_HEAD,
                )
                .into_iter()
                .filter(|id| !is_composite_id(id))
                .collect();
                plain = head_first(&plain, &head);
                if !head.is_empty() && head.len() < plain.len() {
                    div_at = head.len();
                    all_head = true;
                }
            }

            let div = (div_at > 0 && div_at < plain.len()).then_some(lead + div_at);
            ordered.extend(plain);
            let filtered_models = ordered;
            let mut rows: Vec<Row> = Vec::with_capacity(filtered_models.len() + 1);
            for i in 0..filtered_models.len() {
                if div == Some(i) {
                    rows.push(Row::Divider);
                }
                rows.push(Row::Model(i));
            }
            if place_on_current {
                let target = focus.as_deref().or(config.model.as_deref());
                if let Some(pos) = rows.iter().position(|row| match row {
                    Row::Model(mi) => target == Some(filtered_models[*mi].0.as_str()),
                    _ => false,
                }) {
                    sel = pos;
                    place_on_current = false;

                    if let Some(id) = focus.as_deref()
                        && let Some(shape) =
                            super::variants::row_shape(id).filter(|s| s.is_composite())
                    {
                        let cur = efforts.parts_of(config, &effort_saved, id);
                        drop = Some(DropState {
                            row: id.to_string(),
                            slot: 0,
                            sel: open_option(&shape, &cur, 0),
                        });
                    }
                }
            }

            if let Some(d) = &drop {
                match rows.iter().position(|row| match row {
                    Row::Model(mi) => filtered_models[*mi].0 == d.row,
                    _ => false,
                }) {
                    Some(pos) => sel = pos,
                    None => drop = None,
                }
            }

            let drop_shape = drop
                .as_ref()
                .and_then(|d| super::variants::row_shape(&d.row));
            let mut draw_sel = sel;
            if let (Some(d), Some(shape)) = (&drop, &drop_shape) {
                let n = shape.slots.get(d.slot).map_or(0, |s| s.options.len());
                let at = sel.min(rows.len()) + 1;
                for i in (0..n).rev() {
                    rows.insert(at.min(rows.len()), Row::Opt(i));
                }
                draw_sel = at + d.sel;
            }
            efforts.fast = toggles.fast;

            let gap = composite_gap(&rows, lead);

            let view = PickerView {
                provider: &item_name,
                refreshing,
                filter: &filter,
                rows: &rows,
                filtered: &filtered_models,
                models: &models,
                config,
                saved: &effort_saved,
                efforts: &efforts,
                toggles,
                drop: drop.as_ref(),
                all_head,
                gap,
            };
            // Apply already-queued keys before repainting (see model_panes).
            if !std::mem::replace(&mut painted, true) || !poll(Duration::ZERO)? {
                terminal.draw(|frame| {
                    draw_picker(frame, &bg_snapshot, &view, draw_sel, &mut scroll_top)
                })?;
            }

            if !poll(Duration::from_millis(100))? {
                continue;
            }

            if drop.is_none() {
                if rows.is_empty() {
                    sel = 0;
                } else {
                    sel = skip_divider(&rows, sel.min(rows.len() - 1), false);
                }
            }

            let Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = read()?
            else {
                continue;
            };
            place_on_current = false;
            if kind != KeyEventKind::Press {
                continue;
            }
            let Some(key) = picker_key(code, modifiers) else {
                continue;
            };
            let highlighted: Option<String> = view.highlighted(draw_sel).map(str::to_string);
            let live_parts = highlighted
                .as_deref()
                .map(|id| view.live_parts(id))
                .unwrap_or_default();
            let fast_supported =
                highlighted
                    .as_deref()
                    .is_some_and(|id| match super::variants::row_shape(id) {
                        Some(shape) if !shape.variants.is_empty() => {
                            let parts = selection_parts(&shape, &live_parts);
                            let eff = super::variants::effective_fast(
                                &shape.variants,
                                toggles.fast,
                                &parts,
                            );
                            super::variants::has_variant(&shape.variants, !eff, &parts)
                        }
                        _ => toggles.fast || view.fast_variant(id).is_some(),
                    });

            if let (Some(d), Some(shape)) = (drop.clone(), drop_shape.clone()) {
                let cur = efforts.parts_of(config, &effort_saved, &d.row);
                match key {
                    PickerKey::Cancel if code != crossterm::event::KeyCode::Esc => {
                        return Ok(false);
                    }
                    PickerKey::Cancel => {
                        drop = match esc_level(PickLevel::Slot(d.slot)) {
                            Some(PickLevel::Slot(i)) => Some(DropState {
                                row: d.row.clone(),
                                slot: i,
                                sel: open_option(&shape, &cur, i),
                            }),
                            _ => None,
                        };
                    }
                    PickerKey::Up | PickerKey::PageUp => {
                        let s = step_option(&shape, &cur, d.slot, d.sel, false);
                        drop = Some(DropState { sel: s, ..d });
                    }
                    PickerKey::Down | PickerKey::PageDown => {
                        let s = step_option(&shape, &cur, d.slot, d.sel, true);
                        drop = Some(DropState { sel: s, ..d });
                    }

                    PickerKey::EffortDown | PickerKey::EffortUp if d.slot == 0 => {
                        let up = key == PickerKey::EffortUp;
                        efforts.step_with(config, &effort_saved, &d.row, &live_parts, up);
                    }
                    PickerKey::ToggleFast => toggles.toggle_fast(fast_supported),
                    PickerKey::ToggleReasoning => toggles.toggle_reasoning(),
                    PickerKey::Confirm => {
                        let Some(opt) = shape.slots.get(d.slot).and_then(|s| s.options.get(d.sel))
                        else {
                            continue;
                        };
                        let Some(parts) = option_parts(&shape, &cur, d.slot, &opt.id) else {
                            continue;
                        };
                        efforts.parts.insert(d.row.clone(), parts.clone());
                        match enter_level(PickLevel::Slot(d.slot), shape.slots.len()) {
                            (PickLevel::Slot(i), false) => {
                                drop = Some(DropState {
                                    row: d.row.clone(),
                                    slot: i,
                                    sel: open_option(&shape, &parts, i),
                                });
                            }
                            _ => {
                                let shown = efforts.shown(config, &effort_saved, &d.row);
                                let stepped = efforts.was_stepped(&d.row);
                                commit_selection(
                                    config,
                                    &models,
                                    d.row.clone(),
                                    (shown, stepped),
                                    parts,
                                    toggles,
                                )?;
                                return Ok(true);
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            }

            let last = rows.len().saturating_sub(1);
            match key {
                PickerKey::Cancel => return Ok(false),
                PickerKey::Up => sel = skip_divider(&rows, sel.saturating_sub(1), false),
                PickerKey::Down => sel = skip_divider(&rows, (sel + 1).min(last), true),
                PickerKey::PageUp => sel = skip_divider(&rows, sel.saturating_sub(8), false),
                PickerKey::PageDown => sel = skip_divider(&rows, (sel + 8).min(last), true),

                PickerKey::EffortDown | PickerKey::EffortUp => {
                    if let Some(m_id) = &highlighted {
                        let up = key == PickerKey::EffortUp;
                        efforts.step(config, &effort_saved, m_id, up);
                    }
                }
                PickerKey::ToggleFast => toggles.toggle_fast(fast_supported),
                PickerKey::ToggleReasoning => toggles.toggle_reasoning(),
                PickerKey::Type(ch) => {
                    filter.push(ch);
                    sel = 0;
                }
                PickerKey::Backspace => {
                    filter.pop();
                    sel = 0;
                }
                PickerKey::Confirm => {
                    if let Some(id) = highlighted.as_deref()
                        && let Some(shape) =
                            super::variants::row_shape(id).filter(|s| s.is_composite())
                        && let (PickLevel::Slot(0), false) =
                            enter_level(PickLevel::List, shape.slots.len())
                    {
                        let cur = efforts.parts_of(config, &effort_saved, id);
                        drop = Some(DropState {
                            row: id.to_string(),
                            slot: 0,
                            sel: open_option(&shape, &cur, 0),
                        });
                        continue;
                    }

                    if filtered_models.is_empty() && filter.is_empty() && config.model.is_some() {
                        return Ok(false);
                    }

                    let row_effort = highlighted
                        .as_deref()
                        .map(|m_id| {
                            (
                                efforts.shown(config, &effort_saved, m_id),
                                efforts.was_stepped(m_id),
                            )
                        })
                        .unwrap_or((None, false));
                    let chosen_model = if let Some(id) = highlighted {
                        id
                    } else if !filter.is_empty() {
                        validate_direct_model_id(filter.trim(), &models)
                            .unwrap_or_else(|_| filter.trim().to_string())
                    } else {
                        // Nothing listed and nothing typed: don't save a fake
                        // "default" model id — wait for the user to type one.
                        continue;
                    };
                    commit_selection(
                        config,
                        &models,
                        chosen_model,
                        row_effort,
                        super::variants::Parts::new(),
                        toggles,
                    )?;
                    return Ok(true);
                }
            }
        }
    })();

    let _ = terminal.clear();

    result
}

fn commit_selection(
    config: &mut Config,
    models: &[(String, String)],
    chosen_model: String,
    row_effort: (Option<&str>, bool),
    parts: super::variants::Parts,
    toggles: PickerToggles,
) -> anyhow::Result<()> {
    config.model = Some(chosen_model);
    config.model_parts = parts;

    let variant = canonicalize_effort_variant(config, models);
    let path = saved_config_path()?;
    let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
    let mut saved = load_saved_config_at(&path);
    saved.base_url = Some(config.base_url.clone());
    saved.model = config.model.clone();
    if variant.as_ref().and_then(|(_, t, _)| t.as_ref()).is_none() {
        let (shown, stepped) = row_effort;
        commit_row_effort(config, &mut saved, shown, stepped);
    }
    let key = crate::setup::effort_memory_key(
        &config.provider_id,
        &config.base_url,
        config.model.as_deref().unwrap_or_default(),
    );
    saved.remember_parts(&key, &config.model_parts);

    toggles.commit(config, &mut saved);
    save_saved_config_at(&path, &saved)?;
    Ok(())
}

pub(crate) fn validate_direct_model_id(
    raw: &str,
    models: &[(String, String)],
) -> Result<String, String> {
    let input = raw.trim();
    if input.is_empty() {
        return Err("usage: /models <model-id> — type /models to browse models".to_string());
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
                "ambiguous model '{input}' — matches {} models ({shown}{}); type /models to pick",
                ids.len(),
                if ids.len() > 5 { ", …" } else { "" },
            ));
        }
    }
    Err(format!(
        "unknown model '{input}' — no match; type /models to browse {} models",
        models.len(),
    ))
}

mod variants;
pub(crate) use variants::*;
mod rows;
pub(crate) use rows::*;
mod pricing;
pub(crate) use pricing::*;
mod catalog;
pub(crate) use catalog::*;
mod draw;
pub(crate) use draw::*;
