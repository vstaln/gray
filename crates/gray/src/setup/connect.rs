use super::*;
use anyhow::Context;
use ratatui::style::Color;

pub(crate) struct ConnectColors {
    pub box_bg: Color,
    pub input_bg: Color,
    pub accent_peach: Color,
    pub text_dim: Color,
}

pub(crate) fn is_removable(
    item: &ConnectItem,
    config: &Config,
    auth: &std::collections::BTreeMap<String, catalog::AuthEntry>,
) -> bool {
    if item.id.starts_with("custom:") {
        return true;
    }
    item.is_connected(config, auth) || auth.contains_key(&item.id)
}

fn forget_provider_at(
    item: &ConnectItem,
    config: &mut Config,
    auth_path: &std::path::Path,
    saved_path: &std::path::Path,
) -> anyhow::Result<String> {
    if let ConnectAuth::Plugin { auth_ref, .. } = &item.auth {
        let store = crate::auth::CredentialStore::new(auth_path.to_path_buf());
        let removed = store
            .remove(auth_ref)
            .with_context(|| "cannot remove provider credential")?;
        anyhow::ensure!(removed, "not installed: {}", item.id);
        if config.provider_id == item.id {
            config.provider_id.clear();
            config.credential_source.clear();
            config.auth_ref.clear();
            config.api_key = None;
            config.model = None;
        }
        let _saved_lock = crate::setup::lock_saved_config_at(saved_path).ok();
        let mut saved = load_saved_config_at(saved_path);
        if saved.provider_id == item.id {
            saved.provider_id.clear();
            saved.credential_source.clear();
            saved.auth_ref.clear();
            saved.api_key = None;
            saved.auth_mode = None;
            saved.model = None;
        }
        save_saved_config_at(saved_path, &saved)?;
        return Ok(item.name.clone());
    }
    catalog::remove_auth_entry_at(auth_path, &item.id)?;
    if item.id.starts_with("custom:")
        && let Ok(ep_path) = catalog::custom_endpoints_path()
    {
        let _ = catalog::remove_custom_endpoint_at(&ep_path, &item.base_url);
    }
    let name = item.name.clone();
    if normalize_custom_base_url(&config.base_url) == normalize_custom_base_url(&item.base_url) {
        config.api_key = None;
        let _cfg_lock = crate::setup::lock_saved_config_at(saved_path).ok();
        let mut saved = load_saved_config_at(saved_path);

        if saved.api_key.take().is_some() {
            saved.auth_mode = None;
            save_saved_config_at(saved_path, &saved)?;
        }
    }
    Ok(name)
}

pub(crate) fn forget_provider(item: &ConnectItem, config: &mut Config) -> anyhow::Result<String> {
    forget_provider_at(
        item,
        config,
        &catalog::auth_store_path()?,
        &saved_config_path()?,
    )
}

pub(crate) fn insert_paste(buf: &mut String, pasted: &str) {
    buf.push_str(&crate::composer::input::strip_escape_sequences(pasted).replace(['\r', '\n'], ""));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectOutcome {
    Connected,

    Removed(String),

    Dismissed,
}

pub fn run_connect_modal(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
) -> anyhow::Result<ConnectOutcome> {
    run_connect_modal_for(config, bg, None)
}

pub fn run_connect_modal_for(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
    preselect: Option<&str>,
) -> anyhow::Result<ConnectOutcome> {
    let before = config.clone();
    let outcome = connect_modal(config, bg, preselect);
    super::provider_auth::settle_connect_config(config, before, &outcome);
    outcome
}

fn spawn_plugin_login(
    installed: crate::providers::InstalledProvider,
) -> (
    tokio_util::sync::CancellationToken,
    tokio::sync::mpsc::UnboundedReceiver<PluginLoginProgress>,
) {
    let (progress_sender, progress_receiver) = tokio::sync::mpsc::unbounded_channel();
    let cancel = tokio_util::sync::CancellationToken::new();
    let login_cancel = cancel.clone();
    tokio::spawn(async move {
        if let Err(err) = run_plugin_login(installed, login_cancel, progress_sender.clone()).await {
            let _ = progress_sender.send(PluginLoginProgress::Failed(err.to_string()));
        }
    });
    (cancel, progress_receiver)
}

pub fn active_connect_id(config: &Config) -> Option<String> {
    if !config.provider_id.is_empty() {
        return Some(config.provider_id.clone());
    }
    let base = normalize_custom_base_url(&config.base_url);
    load_catalog()
        .ok()?
        .into_iter()
        .find(|(_, p)| normalize_custom_base_url(&p.base_url) == base)
        .map(|(id, _)| id)
}

fn filter_local_suggestions<'a>(
    suggestions: &'a [context::LocalEndpointSuggestion],
    typed: &str,
) -> Vec<&'a context::LocalEndpointSuggestion> {
    let typed = typed.trim().to_lowercase();
    suggestions
        .iter()
        .filter(|s| {
            typed.is_empty()
                || s.name.to_lowercase().contains(&typed)
                || s.url.to_lowercase().contains(&typed)
        })
        .collect()
}

fn migrate_legacy_custom(config: &Config, cat: &catalog::Catalog) {
    if !config.provider_id.is_empty() {
        return;
    }
    let base = normalize_custom_base_url(&config.base_url);
    if base.is_empty() {
        return;
    }
    let known = catalog::POPULAR_DEFS
        .iter()
        .any(|(.., url, _)| normalize_custom_base_url(url) == base)
        || cat
            .values()
            .any(|p| normalize_custom_base_url(&p.base_url) == base);
    if known {
        return;
    }
    let (Ok(auth_path), Ok(ep_path)) =
        (catalog::auth_store_path(), catalog::custom_endpoints_path())
    else {
        return;
    };
    let key = match catalog::load_mixed_store(&auth_path).get("custom") {
        Some(catalog::AuthEntry::Key(key)) => Some(key.clone()),
        _ => None,
    };

    let registered = catalog::load_custom_endpoints_at(&ep_path)
        .iter()
        .any(|e| normalize_custom_base_url(&e.base_url) == base);
    if registered && key.is_none() {
        return;
    }
    if catalog::upsert_custom_endpoint_at(&ep_path, &base, key.is_none()).is_err() {
        return;
    }
    if let Some(key) = key {
        let slug = catalog::custom_key_id(&base);
        if catalog::save_auth_key_at(&auth_path, &slug, &key).is_ok() {
            let _ = catalog::remove_auth_entry_at(&auth_path, "custom");
        }
    }
}

fn connect_modal(
    config: &mut Config,
    bg: Option<&BackgroundSnapshot>,
    mut preselect: Option<&str>,
) -> anyhow::Result<ConnectOutcome> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
    use std::time::Duration;

    let catalog = load_catalog()?;
    let mut filter = String::new();
    let mut sel = 0usize;
    let mut scroll_top = 0usize;

    enum ModalState {
        Selecting,
        EnteringBaseUrl {
            url_buf: String,
            status_msg: Option<String>,

            suggestions: Vec<context::LocalEndpointSuggestion>,
            sel: usize,

            probe_failed: bool,
        },
        EnteringKey {
            item: ConnectItem,
            key_buf: String,
            existing_key: Option<String>,

            env_var: Option<&'static str>,
            status_msg: Option<String>,
        },

        SelectingModel {
            item: ConnectItem,
        },
        ConfirmingRemove {
            item: ConnectItem,
        },
        AuthorizingPlugin {
            item: ConnectItem,
            verification_uri: Option<String>,
            status_msg: Option<String>,
            started: std::time::Instant,
            cancel: tokio_util::sync::CancellationToken,
            progress: tokio::sync::mpsc::UnboundedReceiver<PluginLoginProgress>,
        },
    }

    let mut state = ModalState::Selecting;
    let mut connected_name: Option<(String, String)> = None;

    let (_session, mut terminal) = super::open_modal()?;

    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);

    let _keyboard_enhancement = crate::composer::input::KeyboardEnhancementGuard::push();

    let colors = ConnectColors {
        box_bg: crate::theme::theme().surface_bg,
        input_bg: crate::theme::theme().input_bg,
        accent_peach: crate::theme::theme().accent,
        text_dim: crate::theme::theme().text_dim,
    };

    let bg_snapshot = bg
        .cloned()
        .unwrap_or_else(BackgroundSnapshot::default_initial);

    let detected =
        catalog::detect_connect_candidates(context::ollama_running(), |k| std::env::var(k).ok());

    migrate_legacy_custom(config, &catalog);

    let result = (|| -> anyhow::Result<ConnectOutcome> {
        loop {
            let auth = catalog::load_connect_auth();
            let providers =
                crate::providers::ProviderRegistry::load_cached(&crate::setup::gray_home()?)
                    .installed();
            let customs = catalog::load_custom_endpoints();
            let mut all_items = build_connect_items(&catalog, &providers, &customs);
            catalog::sort_connect_items(&mut all_items, config, &auth, &detected);

            for item in &mut all_items {
                if !item.is_connected(config, &auth)
                    && let Some(d) = detected.get(&item.id)
                {
                    item.sublabel = d.note.clone();
                }
            }
            if let Some(id) = preselect.take()
                && let Some(item) = all_items.iter().find(|item| item.id == id)
            {
                if let Some(installed) = providers.iter().find(|p| p.provider_id() == id) {
                    let (cancel, progress) = spawn_plugin_login(installed.clone());
                    state = ModalState::AuthorizingPlugin {
                        item: item.clone(),
                        verification_uri: None,
                        status_msg: None,
                        started: std::time::Instant::now(),
                        cancel,
                        progress,
                    };
                } else if matches!(item.auth, ConnectAuth::ApiKey) {
                    let existing_key = load_auth_keys().get(&item.id).cloned();
                    state = ModalState::EnteringKey {
                        item: item.clone(),
                        key_buf: String::new(),
                        status_msg: existing_key.is_some().then(|| {
                            format!("{} rejected the saved key — paste a new one", item.name)
                        }),
                        existing_key,
                        env_var: None,
                    };
                }
            }

            if let ModalState::SelectingModel { item } = &state {
                let item = item.clone();
                let installed = match &item.auth {
                    ConnectAuth::Plugin { .. } => {
                        let Some(installed) = providers
                            .iter()
                            .find(|p| p.provider_id() == item.id)
                            .cloned()
                        else {
                            anyhow::bail!("selected provider is no longer installed")
                        };
                        if config.provider_id != item.id {
                            config.model = None;
                        }
                        config.base_url = installed.provider.transport.base_url.to_string();
                        config.provider_id = installed.provider_id();
                        config.credential_source = "plugin".to_string();
                        config.auth_ref = installed.auth_ref();
                        config.api_key = None;
                        Some(installed)
                    }
                    _ => {
                        config.provider_id.clear();
                        config.credential_source.clear();
                        config.auth_ref.clear();
                        None
                    }
                };
                if super::model_modal::model_picker_loop(
                    &mut terminal,
                    config,
                    Some(&bg_snapshot),
                    None,
                )? {
                    let chosen_model = config
                        .model
                        .clone()
                        .unwrap_or_else(|| "default".to_string());
                    match &installed {
                        Some(installed) => {
                            activate_plugin_connection(config, installed, &chosen_model)?;
                        }
                        None => {
                            select_api_key_connection(config)?;
                            if catalog::is_custom_id(&item.id)
                                && let Ok(ep_path) = catalog::custom_endpoints_path()
                            {
                                let _ = catalog::upsert_custom_endpoint_at(
                                    &ep_path,
                                    &config.base_url,
                                    config.api_key.is_none(),
                                );
                            }
                            let path = saved_config_path()?;
                            let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
                            let mut saved = load_saved_config_at(&path);
                            saved.base_url = Some(config.base_url.clone());
                            saved.api_key = config.api_key.clone();
                            saved.model = config.model.clone();
                            saved.auth_mode = Some(if config.api_key.is_none() {
                                AUTH_MODE_NONE.into()
                            } else {
                                AUTH_MODE_API_KEY.into()
                            });
                            save_saved_config_at(&path, &saved)?;
                        }
                    }
                    connected_name = Some((item.name.clone(), chosen_model));
                    return Ok(ConnectOutcome::Connected);
                }
                state = ModalState::Selecting;
                continue;
            }

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
                        &auth,
                        &colors,
                    ),
                    ModalState::EnteringBaseUrl {
                        url_buf,
                        status_msg,
                        suggestions,
                        sel,
                        ..
                    } => super::connect_draw::render_entering_url(
                        frame,
                        area,
                        url_buf,
                        &filter_local_suggestions(suggestions, url_buf),
                        *sel,
                        status_msg,
                        &colors,
                    ),
                    ModalState::EnteringKey {
                        item,
                        key_buf,
                        existing_key,
                        env_var,
                        status_msg,
                    } => super::connect_draw::render_entering_key(
                        frame,
                        area,
                        item,
                        key_buf,
                        existing_key,
                        *env_var,
                        status_msg,
                        &colors,
                    ),
                    ModalState::ConfirmingRemove { item } => {
                        super::connect_draw::render_confirm_remove(frame, area, item, &colors)
                    }

                    ModalState::SelectingModel { .. } => {}
                    ModalState::AuthorizingPlugin {
                        item,
                        verification_uri,
                        status_msg,
                        started,
                        ..
                    } => super::connect_draw::render_authorizing_plugin(
                        frame,
                        area,
                        item,
                        verification_uri.as_deref(),
                        status_msg.as_deref(),
                        started.elapsed(),
                        &colors,
                    ),
                }
            })?;

            if !matches!(state, ModalState::AuthorizingPlugin { .. })
                && !poll(Duration::from_millis(100))?
            {
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
                        }) if modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(ConnectOutcome::Dismissed);
                        }
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
                            code: KeyCode::Enter,
                            modifiers,
                            kind: KeyEventKind::Press,
                            ..
                        }) if modifiers.contains(KeyModifiers::SHIFT) => {
                            if let Some(&item) = filtered.get(sel)
                                && is_removable(item, config, &auth)
                            {
                                state = ModalState::ConfirmingRemove { item: item.clone() };
                            }
                        }
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
                            KeyCode::Esc => return Ok(ConnectOutcome::Dismissed),
                            KeyCode::Enter => {
                                if let Some(&item) = filtered.get(sel) {
                                    if item.id == "custom" {
                                        state = ModalState::EnteringBaseUrl {
                                            url_buf: String::new(),
                                            status_msg: None,
                                            suggestions: context::probe_local_endpoints(),
                                            sel: 0,
                                            probe_failed: false,
                                        };
                                    } else if item.no_auth {
                                        config.base_url = item.base_url.clone();
                                        config.api_key = None;
                                        state = ModalState::SelectingModel { item: item.clone() };
                                    } else if let ConnectAuth::Plugin { .. } = &item.auth {
                                        let Some(installed) = providers
                                            .iter()
                                            .find(|provider| provider.provider_id() == item.id)
                                            .cloned()
                                        else {
                                            state = ModalState::Selecting;
                                            continue;
                                        };
                                        let (cancel, progress) = spawn_plugin_login(installed);
                                        state = ModalState::AuthorizingPlugin {
                                            item: item.clone(),
                                            verification_uri: None,
                                            status_msg: None,
                                            started: std::time::Instant::now(),
                                            cancel,
                                            progress,
                                        };
                                    } else {
                                        let stored =
                                            load_auth_keys().get(&item.id).cloned().or_else(|| {
                                                if config.base_url == item.base_url {
                                                    config.api_key.clone()
                                                } else {
                                                    None
                                                }
                                            });

                                        let (existing_key, env_var) = match stored {
                                            Some(key) => (Some(key), None),
                                            None => detected
                                                .get(&item.id)
                                                .and_then(|d| d.env.clone())
                                                .map(|(var, value)| (Some(value), Some(var)))
                                                .unwrap_or((None, None)),
                                        };
                                        state = ModalState::EnteringKey {
                                            item: item.clone(),
                                            key_buf: String::new(),
                                            existing_key,
                                            env_var,
                                            status_msg: None,
                                        };
                                    }
                                }
                            }
                            _ => {}
                        },
                        Event::Paste(pasted) => {
                            insert_paste(&mut filter, &pasted);
                            sel = 0;
                        }
                        Event::Resize(_, _) => {}
                        _ => {}
                    }
                }
                ModalState::EnteringBaseUrl {
                    url_buf,
                    status_msg,
                    suggestions,
                    sel,
                    probe_failed,
                } => {
                    let filtered = filter_local_suggestions(suggestions, url_buf);
                    if filtered.is_empty() {
                        *sel = 0;
                    } else if *sel >= filtered.len() {
                        *sel = filtered.len() - 1;
                    }
                    match read()? {
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
                            KeyCode::Up => *sel = sel.saturating_sub(1),
                            KeyCode::Down => {
                                if !filtered.is_empty() {
                                    *sel = (*sel + 1).min(filtered.len() - 1);
                                }
                            }
                            KeyCode::Char(ch) => {
                                url_buf.push(ch);
                                *status_msg = None;
                                *sel = 0;
                                *probe_failed = false;
                            }
                            KeyCode::Backspace => {
                                url_buf.pop();
                                *status_msg = None;
                                *sel = 0;
                                *probe_failed = false;
                            }
                            KeyCode::Tab => {
                                if let Some(s) = filtered.get(*sel) {
                                    *url_buf = s.url.to_string();
                                    *status_msg = None;
                                }
                            }
                            KeyCode::Enter => {
                                let chosen = if url_buf.trim().is_empty() {
                                    filtered.get(*sel).map(|s| s.url.to_string())
                                } else {
                                    Some(url_buf.trim().to_string())
                                };
                                let normalized = chosen
                                    .as_deref()
                                    .map(normalize_custom_base_url)
                                    .unwrap_or_default();
                                let valid = normalized.starts_with("http://")
                                    || normalized.starts_with("https://");
                                if normalized.is_empty() || !valid {
                                    *status_msg =
                                        Some("Enter a valid base URL (e.g. https://…/v1)".into());
                                    continue;
                                }

                                let proceed = if *probe_failed {
                                    *probe_failed = false;
                                    true
                                } else {
                                    *status_msg = Some("Checking endpoint…".into());
                                    terminal.draw(|frame| {
                                        let area = frame.area();
                                        if area.width >= 20 && area.height >= 6 {
                                            render_dimmed_background(frame, &bg_snapshot);
                                            super::connect_draw::render_entering_url(
                                                frame, area, url_buf, &filtered, *sel, status_msg,
                                                &colors,
                                            );
                                        }
                                    })?;
                                    match context::probe_custom_endpoint(&normalized) {
                                        context::EndpointProbe::Up { .. }
                                        | context::EndpointProbe::Auth => true,
                                        context::EndpointProbe::Down => {
                                            *status_msg = Some(
                                                "No answer — Enter again to connect anyway".into(),
                                            );
                                            *probe_failed = true;
                                            false
                                        }
                                    }
                                };
                                if proceed {
                                    let key_id = catalog::custom_key_id(&normalized);
                                    let existing =
                                        load_auth_keys().get(&key_id).cloned().or_else(|| {
                                            if config.base_url == normalized {
                                                config.api_key.clone()
                                            } else {
                                                None
                                            }
                                        });
                                    let item = ConnectItem {
                                        id: "custom".to_string(),
                                        name: "Custom".to_string(),
                                        sublabel: "(OpenAI-compatible)".to_string(),
                                        base_url: normalized,
                                        no_auth: false,
                                        auth: ConnectAuth::ApiKey,
                                    };
                                    state = ModalState::EnteringKey {
                                        item,
                                        key_buf: String::new(),
                                        existing_key: existing,
                                        env_var: None,
                                        status_msg: None,
                                    };
                                }
                            }
                            _ => {}
                        },
                        Event::Paste(pasted) => {
                            insert_paste(url_buf, &pasted);
                            *status_msg = None;
                            *sel = 0;
                            *probe_failed = false;
                        }
                        Event::Resize(_, _) => {}
                        _ => {}
                    }
                }
                ModalState::EnteringKey {
                    item,
                    key_buf,
                    existing_key,
                    env_var,
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
                                if catalog::is_custom_id(&item.id) && existing_key.is_none() {
                                    config.base_url = item.base_url.clone();
                                    config.api_key = None;
                                    state = ModalState::SelectingModel { item: item.clone() };
                                } else {
                                    *status_msg = Some(
                                        "No API key entered — please enter a valid key".into(),
                                    );
                                }
                                continue;
                            }

                            *status_msg = Some("Checking key…".into());
                            terminal.draw(|frame| {
                                let area = frame.area();
                                if area.width >= 20 && area.height >= 6 {
                                    render_dimmed_background(frame, &bg_snapshot);
                                    super::connect_draw::render_entering_key(
                                        frame,
                                        area,
                                        item,
                                        key_buf,
                                        existing_key,
                                        *env_var,
                                        status_msg,
                                        &colors,
                                    );
                                }
                            })?;
                            match context::verify_api_key(&item.base_url, &final_key) {
                                context::KeyCheck::Rejected(status) => {
                                    *status_msg = Some(format!(
                                        "{} rejected this key (HTTP {status}) — paste a new one",
                                        item.name
                                    ));
                                    key_buf.clear();
                                }
                                context::KeyCheck::Valid | context::KeyCheck::Unknown => {
                                    if existing_key.is_some()
                                        && item.id != "custom"
                                        && env_var.is_none()
                                    {
                                        save_auth_key(&item.id, &final_key)?;
                                        connect_saved_key(config, &item.base_url, &final_key)?;
                                        return Ok(ConnectOutcome::Connected);
                                    }

                                    let pid = if item.id == "custom" {
                                        catalog::custom_key_id(&item.base_url)
                                    } else {
                                        item.id.clone()
                                    };
                                    save_auth_key(&pid, &final_key)?;
                                    config.base_url = item.base_url.clone();
                                    config.api_key = Some(final_key);
                                    state = ModalState::SelectingModel { item: item.clone() };
                                }
                            }
                        }
                        _ => {}
                    },
                    Event::Paste(pasted) => {
                        insert_paste(key_buf, &pasted);
                        *status_msg = None;
                    }
                    Event::Resize(_, _) => {}
                    _ => {}
                },
                ModalState::AuthorizingPlugin {
                    item,
                    verification_uri,
                    status_msg,
                    cancel,
                    progress,
                    ..
                } => {
                    let mut next_state: Option<ModalState> = None;
                    while let Ok(progress) = progress.try_recv() {
                        match progress {
                            PluginLoginProgress::Started {
                                verification_uri: uri,
                            } => {
                                *verification_uri = Some(uri);
                                *status_msg = None;
                            }
                            PluginLoginProgress::Pending => {
                                *status_msg = Some("Waiting for browser approval".into())
                            }
                            PluginLoginProgress::CredentialSaved => {
                                *status_msg = Some("Credential saved".into())
                            }
                            PluginLoginProgress::Models(rows) => {
                                super::context::save_provider_model_rows(
                                    &super::catalog::plugin_models_key(&item.id),
                                    rows,
                                );
                                next_state =
                                    Some(ModalState::SelectingModel { item: item.clone() });
                            }
                            PluginLoginProgress::ModelsUnavailable(_message) => {
                                next_state =
                                    Some(ModalState::SelectingModel { item: item.clone() });
                            }
                            PluginLoginProgress::Cancelled => {
                                *status_msg = None;
                                next_state = Some(ModalState::Selecting);
                            }
                            PluginLoginProgress::Failed(message) => *status_msg = Some(message),
                        }
                    }
                    if let Some(next_state) = next_state.take() {
                        state = next_state;
                        continue;
                    }
                    if !poll(Duration::from_millis(100))? {
                        continue;
                    }
                    match read()? {
                        Event::Key(KeyEvent {
                            code: KeyCode::Esc,
                            kind: KeyEventKind::Press,
                            ..
                        }) => {
                            cancel.cancel();
                            state = ModalState::Selecting;
                        }
                        Event::Key(KeyEvent {
                            code: KeyCode::Char('c'),
                            modifiers,
                            kind: KeyEventKind::Press,
                            ..
                        }) if modifiers.contains(KeyModifiers::CONTROL) => {
                            return Ok(ConnectOutcome::Dismissed);
                        }
                        _ => {}
                    }
                }
                ModalState::ConfirmingRemove { item } => match read()? {
                    Event::Key(KeyEvent {
                        code: KeyCode::Char('c'),
                        modifiers,
                        kind: KeyEventKind::Press,
                        ..
                    }) if modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(ConnectOutcome::Dismissed);
                    }
                    Event::Key(KeyEvent {
                        code,
                        kind: KeyEventKind::Press,
                        ..
                    }) => match code {
                        KeyCode::Esc => {
                            state = ModalState::Selecting;
                        }
                        KeyCode::Enter => {
                            let name = forget_provider(item, config)?;
                            return Ok(ConnectOutcome::Removed(name));
                        }
                        _ => {}
                    },
                    Event::Resize(_, _) => {}
                    _ => {}
                },

                ModalState::SelectingModel { .. } => {}
            }
        }
    })();

    let _ = terminal.clear();

    result
}

#[path = "paste_tests.rs"]
#[cfg(test)]
mod paste_tests;

#[path = "remove_tests.rs"]
#[cfg(test)]
mod remove_tests;
