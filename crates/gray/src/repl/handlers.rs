use super::*;

pub(crate) fn expand_prompt_template(cmd: ReplCommand, cwd: &Path, local: bool) -> ReplCommand {
    let ReplCommand::Unknown(raw) = &cmd else {
        return cmd;
    };
    let body = raw.trim().trim_start_matches('/');
    let (name, rest) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
    match crate::prompt_templates::find(cwd, name) {
        Some(_) if local => ReplCommand::Empty,
        Some(t) => ReplCommand::Prompt(crate::prompt_templates::expand(&t, rest)),
        None => cmd,
    }
}

pub(crate) async fn handle_sys(
    config: &Config,
    cwd: &Path,
    action: SysAction,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
    session_id: Option<&str>,
) {
    let path = match crate::sys_prompt_path() {
        Ok(p) => p,
        Err(e) => {
            say(tui, &format!("{e}"));
            return;
        }
    };
    match action {
        SysAction::Show => match load_or_create_system_prompt_at(&path) {
            Ok(body) => {
                say(
                    tui,
                    &format!("system prompt: {}\n---\n{body}\n---", path.display()),
                );
            }
            Err(e) => say_err(tui, &format!("failed to read {}: {e}", path.display())),
        },
        SysAction::Reset => {
            match crate::sys_editor::backup_before_overwrite(&path) {
                Ok(Some(backup)) => say(tui, &format!("backup: {}", backup.display())),
                Ok(None) => {}
                Err(e) => {
                    say_err(tui, &format!("backup failed ({e}) — reset aborted"));
                    return;
                }
            }
            if let Err(e) = std::fs::write(&path, crate::stamped_system_prompt(DEFAULT_SYS_PROMPT))
            {
                say_err(tui, &format!("failed to reset {}: {e}", path.display()));
                return;
            }
            say(
                tui,
                &format!("✓ system prompt restored to default ({})", path.display()),
            );
            reload_agent(agent, config, cwd, session_id, tui).await;
        }
        SysAction::Edit => {
            if let Err(e) = load_or_create_system_prompt_at(&path) {
                say(tui, &format!("{e}"));
                return;
            }
            let initial = std::fs::read_to_string(&path).unwrap_or_default();

            if let Err(e) = crate::sys_editor::backup_before_overwrite(&path) {
                say(
                    tui,
                    &format!("backup failed ({e}) — editing without a copy"),
                );
            }
            let tui_snap = tui.cloned();
            let editor_paused = if let Some(shared) = &tui_snap {
                let mut t = shared.lock().expect("tui lock");
                t.pending_resize = Some((
                    t.last_width,
                    std::time::Instant::now() + std::time::Duration::from_secs(3600),
                ));
                t.set_modal_open(true);
                true
            } else {
                false
            };
            let res = crate::sys_editor::run_external_editor(&path, &initial);
            if editor_paused && let Some(shared) = &tui_snap {
                let mut t = shared.lock().expect("tui lock");
                t.pending_resize = None;
                t.set_modal_open(false);
                if let Ok((cols, _)) = crossterm::terminal::size() {
                    t.reflow_on_resize(cols);
                } else {
                    let _ = t.draw();
                }
            }
            match res {
                Ok(Some(_)) => {
                    say(
                        tui,
                        "✓ system prompt saved — applies from your next message",
                    );
                    reload_agent(agent, config, cwd, session_id, tui).await;
                }
                Ok(None) => say(tui, "prompt unchanged"),
                Err(e) => say_err(tui, &format!("editor error: {e}")),
            }
        }
    }
}

pub(crate) fn undo_cut(messages: &[Message]) -> Option<usize> {
    messages
        .iter()
        .rposition(|message| message.role == gray_core::message::Role::User)
}

pub(crate) async fn handle_undo(
    retry: bool,
    agent: &mut Option<Agent>,
    session_state: &mut Option<SessionState>,
    pending_history: &mut std::sync::Arc<Vec<Message>>,
    tui: Option<&crate::composer::SharedTui>,
) -> Option<String> {
    let Some(ag) = agent.as_mut() else {
        say(tui, "nothing to undo (no conversation yet)");
        return None;
    };
    let messages = ag.messages().to_vec();
    let Some(cut) = undo_cut(&messages) else {
        say(tui, "nothing to undo (no turn of your own to drop)");
        return None;
    };
    let resent = if retry {
        messages[cut].text_content()
    } else {
        String::new()
    };
    let kept: Vec<Message> = messages[..cut].to_vec();

    if let Some(state) = session_state
        && let Err(e) = state.store.rewind(&state.session_id, kept.len()).await
    {
        say_err(tui, &format!("undo failed: {e}"));
        return None;
    }
    ag.set_messages(kept.clone());
    std::sync::Arc::make_mut(pending_history).truncate(kept.len());
    let dropped = messages.len() - kept.len();
    say(
        tui,
        &format!("undid the last turn · {dropped} messages out of context"),
    );
    (!resent.is_empty()).then_some(resent)
}

pub(crate) async fn reload_agent(
    agent: &mut Option<Agent>,
    config: &Config,
    cwd: &Path,
    session_id: Option<&str>,
    tui: Option<&crate::composer::SharedTui>,
) -> bool {
    let started = std::time::Instant::now();
    let old = agent.take();
    let mut rebuilt = match build_agent(config, cwd, session_id).await {
        Ok(a) => a,
        Err(e) => {
            say(tui, &format!("{e}"));
            *agent = old;
            return false;
        }
    };
    if let Some(old) = old {
        rebuilt = rebuilt.with_messages(old.messages().to_vec());
    }
    *agent = Some(rebuilt);
    log::debug!(target: "gray_timing", "reload_agent elapsed_ms={}", started.elapsed().as_millis());
    true
}

pub(crate) async fn switch_agent_model(
    agent: &mut Option<Agent>,
    config: &Config,
    cwd: &Path,
    session_id: Option<&str>,
    tui: Option<&crate::composer::SharedTui>,
) {
    let Some(old) = agent.take() else {
        reload_agent(agent, config, cwd, session_id, tui).await;
        return;
    };
    let started = std::time::Instant::now();
    match crate::build_model_agent(config, cwd, session_id).await {
        Ok(fresh) => {
            let messages = old.messages().to_vec();
            *agent = Some(fresh.with_tool_surface_of(old).with_messages(messages));
            log::debug!(target: "gray_timing", "switch_agent_model elapsed_ms={}", started.elapsed().as_millis());
        }
        Err(e) => {
            say(tui, &format!("{e}"));
            *agent = Some(old);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ModelArg {
    Picker { focus: Option<String> },
    Direct(String),
}

impl From<Option<String>> for ModelArg {
    fn from(direct: Option<String>) -> Self {
        match direct {
            Some(m) => ModelArg::Direct(m),
            None => ModelArg::Picker { focus: None },
        }
    }
}

pub(crate) struct ModelApply {
    pub(crate) model: String,
    pub(crate) clamped: Option<(String, String)>,
    pub(crate) prev_effort: Option<String>,
}

pub(crate) fn apply_model_selection(
    config: &mut Config,
    raw: &str,
    effort: Option<&str>,
    fast: Option<bool>,
) -> Result<ModelApply, String> {
    let path = crate::setup::saved_config_path().ok();
    apply_model_selection_at(config, raw, effort, fast, path.as_deref())
}

pub(crate) fn apply_model_selection_at(
    config: &mut Config,
    raw: &str,
    effort: Option<&str>,
    fast: Option<bool>,
    saved_path: Option<&Path>,
) -> Result<ModelApply, String> {
    let (_, list_key, _) = crate::setup::picker_scope(config);
    let cached = crate::setup::saved_models_for(&list_key, &config.base_url);
    let mut checked = crate::setup::validate_direct_model_id(raw, &cached);
    let cached_variant = if checked.is_err() {
        crate::setup::decompose_model_variant(raw, &cached)
    } else {
        None
    };
    let mut live_rows: Vec<(String, String)> = Vec::new();
    if checked.is_err() && cached_variant.is_none() {
        let started = std::time::Instant::now();
        let (_, known) = crate::setup::provider_models_for_config(config);
        checked = crate::setup::validate_direct_model_id(raw, &known);
        live_rows = known;
        log::debug!(target: "gray_timing", "model list live fetch elapsed_ms={}", started.elapsed().as_millis());
    }

    let mut ids = cached.clone();
    for row in live_rows {
        if !ids.iter().any(|(id, _)| *id == row.0) {
            ids.push(row);
        }
    }
    let (m, variant_tier, id_fast) = match checked {
        Ok(canonical) => crate::setup::decompose_model_variant(&canonical, &ids)
            .unwrap_or((canonical, None, false)),
        Err(msg) => {
            match cached_variant.or_else(|| crate::setup::decompose_model_variant(raw, &ids)) {
                Some(v) => v,
                None => return Err(msg),
            }
        }
    };

    let fast = if std::env::var_os("GRAY_FAST").is_some() {
        None
    } else {
        fast.or(id_fast.then_some(true))
    };
    let _saved_lock = match saved_path {
        Some(path) => Some(crate::setup::lock_saved_config_at(path).map_err(|e| format!("{e:#}"))?),
        None => None,
    };
    let mut saved = saved_path.map(crate::setup::load_saved_config_at);

    if let Some(f) = fast {
        config.fast_mode = Some(f);
        if let Some(saved) = saved.as_mut() {
            saved.fast_mode = Some(f);
        }
    }
    config.model = Some(m.clone());

    let prev_effort = config.thinking_effort.clone();
    match variant_tier.or_else(|| effort.map(str::to_string)) {
        Some(e) if std::env::var_os("GRAY_THINKING_EFFORT").is_none() => {
            config.thinking_effort = Some(e.clone());
            if let Some(saved) = saved.as_mut() {
                saved.remember_effort(
                    &crate::setup::effort_memory_key(&config.provider_id, &config.base_url, &m),
                    &e,
                );
                saved.thinking_effort = config.thinking_effort.clone();
            }
        }
        Some(_) => {}
        None => {
            crate::setup::apply_connection_effort(
                config,
                saved.get_or_insert_with(Default::default),
            );
        }
    }
    let clamped = super::clamp_thinking_to_model(config);
    if let Some(saved) = saved.as_mut() {
        saved.model = Some(m.clone());
    }
    if let (Some(path), Some(saved)) = (saved_path, saved.as_ref()) {
        crate::setup::save_saved_config_at(path, saved)
            .map_err(|e| format!("could not save the model choice: {e:#}"))?;
    }
    Ok(ModelApply {
        model: m,
        clamped,
        prev_effort,
    })
}

async fn finish_model_pick(
    config: &mut Config,
    cwd: &Path,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
    session_id: Option<&str>,
    hide_thinking: &mut bool,
    (prev_effort, prev_show, prev_fast): (Option<String>, Option<bool>, Option<bool>),
) {
    let clamped = super::clamp_thinking_to_model(config);
    let show_changed = config.show_reasoning != prev_show;
    let fast_changed = config.fast_mode != prev_fast;
    if clamped.is_some() || config.thinking_effort != prev_effort || show_changed {
        *hide_thinking = config.reasoning_hidden();
    }
    if let Some(shared) = tui {
        let mut t = shared.lock().expect("tui lock");
        if let Some(m) = &config.model {
            t.set_model(m.clone());
            t.set_model_label(crate::setup::composite_label_for(config));
            t.set_model_label_parts(crate::setup::composite_label_parts_for(config));

            let label = super::model_receipt_label(config).unwrap_or_else(|| m.clone());
            t.push_action("Model set to", Some(&label));
        }
        if (config.thinking_effort != prev_effort || fast_changed)
            && let Some(eff) = &config.thinking_effort
        {
            t.set_thinking_effort(crate::setup::effort_chip(eff, config));
        }
        if config.thinking_effort != prev_effort || show_changed {
            t.set_hide_thinking(*hide_thinking);
        }
        if let Some((old, new)) = clamped {
            super::notice::push_notice(
                &mut t,
                "Thinking effort clamped",
                &format!("from {old} to {new} (not supported by this model)"),
                false,
            );
        }
        t.ensure_gap();
        let _ = t.draw();
    } else if let Some((old, new)) = clamped {
        say(
            None,
            &format!("Thinking effort clamped from {old} to {new} (not supported by this model)"),
        );
    }
    if let Some(m) = config.model.clone()
        && crate::setup::get_user_context_window().is_none()
        && crate::setup::get_cached_model_context(&m).is_none()
    {
        crate::setup::warm_model_context_cache(config);
    }
    switch_agent_model(agent, config, cwd, session_id, tui).await;
}

/// `/models` installable-login path: install the provider plugin exactly the
/// way `/plugin install` does, grant `provider.credentials` (the capability
/// the row exists for), then hand the new provider id to the sign-in flow.
/// Returns the provider id to preselect, or None when the flow should stop.
async fn install_login_plugin(
    name: &str,
    tui: Option<&crate::composer::SharedTui>,
) -> Option<String> {
    super::say(tui, &format!("installing {name}…"));
    let home = match crate::plugin_cli::home() {
        Ok(home) => home,
        Err(e) => {
            super::say_err(tui, &format!("install failed: {e:#}"));
            return None;
        }
    };
    let lines = match crate::plugin_cli::install_spec_lines(&home, name, false).await {
        Ok(lines) => lines,
        Err(e) => {
            super::say_err(tui, &format!("install failed: {e:#}"));
            return None;
        }
    };
    let granted = crate::plugin_cli::grant_capability(
        &home,
        name,
        crate::providers::registry::PROVIDER_CAPABILITY,
    )
    .unwrap_or(false);
    for line in lines {
        if granted && line.contains("Grant later: gray plugin capabilities") {
            continue;
        }
        super::say(tui, &line);
    }
    if granted {
        super::say(tui, &format!("{name}: granted provider.credentials"));
    }
    let provider = crate::setup::gray_home()
        .ok()
        .map(|home| crate::providers::ProviderRegistry::load_cached(&home).installed())
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.plugin == name);
    match provider {
        Some(installed) => Some(installed.provider_id()),
        None => {
            super::say(
                tui,
                &format!("installed {name}, but no sign-in provider appeared — check /plugins"),
            );
            None
        }
    }
}

pub(crate) async fn handle_model_panes(
    config: &mut Config,
    cwd: &Path,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
    session_id: Option<&str>,
    hide_thinking: &mut bool,
) -> Option<String> {
    if tui.is_none() {
        match config.model.as_deref() {
            Some(m) => say(None, &format!("model {m} — /models provider/id to switch")),
            None => say(
                None,
                "no model configured — run /models to set up provider & key (or /models provider/id; /help)",
            ),
        }
        return None;
    }
    let prev_effort = config.thinking_effort.clone();
    let prev_show = config.show_reasoning;
    let prev_fast = config.fast_mode;
    let bg = tui.map(|shared| shared.lock().expect("tui lock").snapshot());
    let result = super::with_modal_sync(tui, || crate::setup::run_model_panes(config, bg.as_ref()));
    match result {
        Ok(crate::setup::PanesOutcome::Picked) => {
            finish_model_pick(
                config,
                cwd,
                agent,
                tui,
                session_id,
                hide_thinking,
                (prev_effort, prev_show, prev_fast),
            )
            .await;
        }
        Ok(crate::setup::PanesOutcome::Removed(name)) => {
            reload_agent(agent, config, cwd, session_id, tui).await;
            super::say(tui, &format!("removed {name}"));
        }
        Ok(outcome) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.clear_draft();
                t.ensure_gap();
                let _ = t.draw();
            }
            if let crate::setup::PanesOutcome::Connect(id) = outcome {
                return Some(id);
            }
            if let crate::setup::PanesOutcome::Install(name) = outcome {
                return install_login_plugin(&name, tui).await;
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                super::notice::push_notice(&mut t, "Model error", &e.to_string(), true);
                t.ensure_gap();
            } else {
                say(None, &format!("model error: {e}"));
            }
        }
    }
    None
}

pub(crate) async fn handle_model(
    config: &mut Config,
    cwd: &Path,
    arg: ModelArg,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
    session_id: Option<&str>,
    hide_thinking: &mut bool,
) {
    let (direct, focus) = match arg {
        ModelArg::Direct(m) => (Some(m), None),
        ModelArg::Picker { focus } => (None, focus),
    };
    if let Some(m) = direct {
        let applied = match apply_model_selection(config, &m, None, None) {
            Ok(a) => a,
            Err(msg) => {
                say(tui, &msg);
                return;
            }
        };
        let m = applied.model;
        if applied.clamped.is_some() || config.thinking_effort != applied.prev_effort {
            *hide_thinking = config.reasoning_hidden();
        }
        if let Some(shared) = tui {
            let mut t = shared.lock().expect("tui lock");
            t.set_model(m.clone());
            t.set_model_label(crate::setup::composite_label_for(config));
            t.set_model_label_parts(crate::setup::composite_label_parts_for(config));
            if config.thinking_effort != applied.prev_effort
                && let Some(eff) = &config.thinking_effort
            {
                t.set_thinking_effort(crate::setup::effort_chip(eff, config));
                t.set_hide_thinking(*hide_thinking);
            }
            t.push_action("Model set to", Some(&m));
            if let Some((old, new)) = &applied.clamped {
                super::notice::push_notice(
                    &mut t,
                    "Thinking effort clamped",
                    &format!("from {old} to {new} (not supported by this model)"),
                    false,
                );
            }
            t.ensure_gap();
            let _ = t.draw();
        } else {
            say(None, &format!("✓ Model set to {m}"));
            if let Some((old, new)) = &applied.clamped {
                say(
                    None,
                    &format!(
                        "Thinking effort clamped from {old} to {new} (not supported by this model)"
                    ),
                );
            }
        }
        if crate::setup::get_user_context_window().is_none()
            && crate::setup::get_cached_model_context(&m).is_none()
        {
            crate::setup::warm_model_context_cache(config);
        }
        switch_agent_model(agent, config, cwd, session_id, tui).await;
        return;
    }

    if tui.is_none() {
        match config.model.as_deref() {
            Some(m) => say(None, &format!("model {m} — /models provider/id to switch")),
            None => say(
                None,
                "no model configured — run /models to set up provider & key (or /models provider/id; /help)",
            ),
        }
        return;
    }
    let prev_effort = config.thinking_effort.clone();
    let prev_show = config.show_reasoning;
    let prev_fast = config.fast_mode;
    let bg = tui.map(|shared| shared.lock().expect("tui lock").snapshot());
    let result = with_modal(
        tui,
        crate::setup::run_model_menu(config, bg.as_ref(), focus.as_deref()),
    )
    .await;
    match result {
        Ok(true) => {
            finish_model_pick(
                config,
                cwd,
                agent,
                tui,
                session_id,
                hide_thinking,
                (prev_effort, prev_show, prev_fast),
            )
            .await;
        }
        Ok(false) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.clear_draft();

                t.ensure_gap();
                let _ = t.draw();
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                super::notice::push_notice(&mut t, "Model error", &e.to_string(), true);
                t.ensure_gap();
            } else {
                say(None, &format!("model error: {e}"));
            }
        }
    }
}
