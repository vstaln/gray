use super::*;

pub(crate) fn format_skill_paste(body: &str, name: &str, args: Option<&str>) -> String {
    let mut out = format!(
        "The user explicitly invoked the \"{name}\" skill. Its instructions are binding for the current task: follow them now, starting from the first step, even if you were mid-task — abandon any plan that conflicts with them."
    );
    out.push_str("\n\n");
    out.push_str(body);
    if let Some(a) = args.filter(|a| !a.is_empty()) {
        out.push_str(&format!("\n\n**ARGUMENTS:** {a}"));
    }
    out
}

fn paste_skill_into_chat(
    tui: Option<&crate::composer::SharedTui>,
    cwd: &Path,
    name: &str,
    expanded: &str,
) {
    if let Some(shared) = tui {
        let args = serde_json::json!({ "name": name });
        let header = crate::tool_fmt::format_tool_call_header("skill", &args, Some(cwd), None);
        let body: Vec<ratatui::text::Line<'static>> = expanded
            .lines()
            .map(|l| ratatui::text::Line::from(l.to_string()))
            .collect();
        let mut t = shared.lock().expect("tui lock");
        t.push_tool_box(header, body);
        let _ = t.draw();
    } else {
        say(None, expanded);
    }
}

pub(crate) fn parse_skill_toggle(rest: &str) -> Option<(bool, String)> {
    parse_skill_name_toggle(rest)
}

pub(crate) fn parse_skills_auto_toggle(rest: &str) -> Option<bool> {
    let mut parts = rest.split_whitespace();
    let word = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if word.eq_ignore_ascii_case("on") || word.eq_ignore_ascii_case("enable") {
        Some(true)
    } else if word.eq_ignore_ascii_case("off") || word.eq_ignore_ascii_case("disable") {
        Some(false)
    } else {
        None
    }
}

fn parse_skill_name_toggle(rest: &str) -> Option<(bool, String)> {
    let mut parts = rest.split_whitespace();
    let verb = parts.next()?;
    let on = if verb.eq_ignore_ascii_case("enable") {
        true
    } else if verb.eq_ignore_ascii_case("disable") {
        false
    } else {
        return None;
    };
    let name = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((on, name.to_string()))
}

pub(crate) fn apply_skills_auto_toggle(config_path: &Path, on: bool) -> Result<String, String> {
    let _cfg_lock =
        crate::setup::lock_saved_config_at(config_path).map_err(|e| format!("{e:#}"))?;
    let mut saved = crate::setup::load_saved_config_at(config_path);
    saved.skills_auto = if on { None } else { Some(false) };
    if on {
        saved.disabled_skills.clear();
    }
    crate::setup::save_saved_config_at(config_path, &saved).map_err(|e| format!("{e:#}"))?;
    Ok(if on {
        "✓ skills on — all skills back in context".to_string()
    } else {
        "✓ skills off — hidden from the model, /skills <name> still runs".to_string()
    })
}

pub(crate) fn apply_skill_toggle(
    config_path: &Path,
    discovered: &[crate::skills::Skill],
    on: bool,
    name: &str,
) -> Result<String, String> {
    if !discovered.iter().any(|s| s.name == name) {
        let names = discovered
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "no skill '{name}' (available: {})",
            if names.is_empty() { "(none)" } else { &names }
        ));
    }
    let _cfg_lock = crate::setup::lock_saved_config_at(config_path).ok();
    let mut saved = crate::setup::load_saved_config_at(config_path);
    if on {
        saved.disabled_skills.remove(name);
    } else {
        saved.disabled_skills.insert(name.to_string());
    }
    crate::setup::save_saved_config_at(config_path, &saved).map_err(|e| format!("{e:#}"))?;
    Ok(if on {
        format!("✓ skill '{name}' enabled")
    } else {
        format!("✓ skill '{name}' disabled — hidden from the model, /skills {name} still runs")
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Subsystem {
    Gateway,
    Intro,
}

impl Subsystem {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Subsystem::Gateway => "gateway",
            Subsystem::Intro => "intro",
        }
    }

    fn copy(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Subsystem::Gateway => (
                "gateway on",
                "run/start will refuse to start",
                "status/stop still work",
            ),
            Subsystem::Intro => (
                "intro on — the logo animates at startup",
                "the settled banner shows instead",
                "/intro on restores it",
            ),
        }
    }

    fn field(self, saved: &mut crate::setup::SavedConfig) -> &mut Option<bool> {
        match self {
            Subsystem::Gateway => &mut saved.gw_auto,
            Subsystem::Intro => &mut saved.intro,
        }
    }
}

pub(crate) fn parse_on_off(rest: &str) -> Option<bool> {
    let mut parts = rest.split_whitespace();
    let word = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    match word.to_ascii_lowercase().as_str() {
        "on" | "enable" => Some(true),
        "off" | "disable" => Some(false),
        _ => None,
    }
}

pub(crate) fn apply_subsystem_toggle(
    config_path: &Path,
    subsystem: Subsystem,
    on: bool,
) -> Result<String, String> {
    let _cfg_lock = crate::setup::lock_saved_config_at(config_path).ok();
    let mut saved = crate::setup::load_saved_config_at(config_path);
    *subsystem.field(&mut saved) = if on { None } else { Some(false) };
    crate::setup::save_saved_config_at(config_path, &saved).map_err(|e| format!("{e:#}"))?;
    let (on_report, off_effect, manual) = subsystem.copy();
    Ok(if on {
        format!("✓ {on_report}")
    } else {
        format!("✓ {} off — {off_effect}, {manual}", subsystem.label())
    })
}

pub(crate) fn toggle_subsystem(subsystem: Subsystem, on: bool) -> String {
    match crate::setup::saved_config_path() {
        Ok(path) => match apply_subsystem_toggle(&path, subsystem, on) {
            Ok(msg) | Err(msg) => msg,
        },
        Err(e) => format!("{e:#}"),
    }
}

pub(crate) fn expand_skill_command(
    cmd: ReplCommand,
    cwd: &Path,
    tui: Option<&crate::composer::SharedTui>,
    local: bool,
) -> ReplCommand {
    let to_prompt = |expanded: String| {
        if local {
            ReplCommand::Empty
        } else {
            ReplCommand::Prompt(expanded)
        }
    };

    if let ReplCommand::Unknown(raw) = &cmd {
        let body = raw.trim().trim_start_matches('/');
        let (name, rest) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
        if let Some(t) = crate::prompt_templates::find(cwd, name) {
            return to_prompt(crate::prompt_templates::expand(&t, rest));
        }
        return cmd;
    }
    let ReplCommand::Skill(payload) = cmd else {
        return cmd;
    };
    let discovered = crate::skills::discover_skills(cwd);
    let rest = match payload {
        Some(rest) => rest,
        None => {
            use crate::setup::ManagerOutcome;
            if tui.is_some() {
                let bg = tui.as_ref().map(|s| s.lock().expect("tui lock").snapshot());
                match with_modal_sync(tui, || crate::setup::run_skills_modal(bg.as_ref(), cwd)) {
                    Ok(ManagerOutcome::Invoke(name)) => name,
                    Ok(ManagerOutcome::Closed(true)) => {
                        if let Some(shared) = tui {
                            let mut t = shared.lock().expect("tui lock");
                            t.push_action("Skills updated", None);
                            t.ensure_gap();
                            let _ = t.draw();
                        }
                        return ReplCommand::Empty;
                    }
                    Ok(ManagerOutcome::Closed(false)) => {
                        if let Some(shared) = tui {
                            let mut t = shared.lock().expect("tui lock");
                            t.clear_draft();

                            t.ensure_gap();
                            let _ = t.draw();
                        }
                        return ReplCommand::Empty;
                    }
                    Err(e) => {
                        say(tui, &format!("skills error: {e}"));
                        return ReplCommand::Empty;
                    }
                }
            } else if discovered.skills.is_empty() {
                say(tui, "no skills discovered");
                return ReplCommand::Empty;
            } else {
                let auto = crate::setup::skills_auto_enabled();
                let disabled = crate::setup::disabled_skill_names();
                if !auto {
                    say(tui, "skills auto-load is off (/skills on to re-enable)");
                }
                for s in &discovered.skills {
                    let mut row = crate::skills::format_discovered_skill_row(s);
                    if !auto {
                        row.push_str(" [auto-off]");
                    } else if disabled.contains(&s.name) {
                        row.push_str(" [disabled]");
                    }
                    say(tui, &row);
                }
                return ReplCommand::Empty;
            }
        }
    };
    if let Some(on) = parse_skills_auto_toggle(&rest) {
        let shadowed = discovered.skills.iter().any(|s| s.name == rest.trim());
        if !shadowed {
            if !local {
                match crate::setup::saved_config_path() {
                    Ok(path) => match apply_skills_auto_toggle(&path, on) {
                        Ok(msg) | Err(msg) => say(tui, &msg),
                    },
                    Err(e) => say(tui, &format!("{e:#}")),
                }
            }
            return ReplCommand::Empty;
        }
    }
    if let Some((on, toggle_name)) = parse_skill_toggle(&rest) {
        if !local {
            match crate::setup::saved_config_path() {
                Ok(path) => match apply_skill_toggle(&path, &discovered.skills, on, &toggle_name) {
                    Ok(msg) | Err(msg) => say(tui, &msg),
                },
                Err(e) => say(tui, &format!("{e:#}")),
            }
        }
        return ReplCommand::Empty;
    }
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n.trim(), Some(a.trim().to_string())),
        None => (rest.as_str(), None),
    };
    let Some(skill) = discovered.skills.iter().find(|s| s.name == name) else {
        let names = discovered
            .skills
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        say(
            tui,
            &format!(
                "no skill '{name}' (available: {})",
                if names.is_empty() { "(none)" } else { &names }
            ),
        );
        return ReplCommand::Empty;
    };
    if let Err(msg) = crate::skills::validate_skill_args(skill, args.as_deref()) {
        say(tui, &msg);
        return ReplCommand::Empty;
    };
    let expanded = match std::fs::read_to_string(&skill.file_path) {
        Ok(content) => {
            let body = crate::skills_tool::strip_frontmatter(&content);
            format_skill_paste(body, &skill.name, args.as_deref())
        }
        Err(e) => {
            say(
                tui,
                &format!("failed to read {}: {e}", skill.file_path.display()),
            );
            return ReplCommand::Empty;
        }
    };

    if !local {
        paste_skill_into_chat(tui, cwd, &skill.name, &expanded);
    }
    to_prompt(expanded)
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
            Err(e) => say(tui, &format!("failed to read {}: {e}", path.display())),
        },
        SysAction::Reset => {
            match crate::sys_editor::backup_before_overwrite(&path) {
                Ok(Some(backup)) => say(tui, &format!("backup: {}", backup.display())),
                Ok(None) => {}
                Err(e) => {
                    say(tui, &format!("backup failed ({e}) — reset aborted"));
                    return;
                }
            }
            if let Err(e) = std::fs::write(&path, crate::stamped_system_prompt(DEFAULT_SYS_PROMPT))
            {
                say(tui, &format!("failed to reset {}: {e}", path.display()));
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

            let _ = crate::sys_editor::backup_before_overwrite(&path);
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
                Err(e) => say(tui, &format!("editor error: {e}")),
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
        say(tui, &format!("undo failed: {e}"));
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
    if let Some(f) = fast {
        config.fast_mode = Some(f);
        if let Some(path) = saved_path {
            let mut saved = crate::setup::load_saved_config_at(path);
            saved.fast_mode = Some(f);
            let _ = crate::setup::save_saved_config_at(path, &saved);
        }
    }
    config.model = Some(m.clone());

    let prev_effort = config.thinking_effort.clone();
    match variant_tier.or_else(|| effort.map(str::to_string)) {
        Some(e) if std::env::var_os("GRAY_THINKING_EFFORT").is_none() => {
            config.thinking_effort = Some(e.clone());
            if let Some(path) = saved_path {
                let mut saved = crate::setup::load_saved_config_at(path);
                saved.remember_effort(
                    &crate::setup::effort_memory_key(&config.provider_id, &config.base_url, &m),
                    &e,
                );
                saved.thinking_effort = config.thinking_effort.clone();
                let _ = crate::setup::save_saved_config_at(path, &saved);
            }
        }
        Some(_) => {}
        None => {
            let _cfg_lock = saved_path.and_then(|p| crate::setup::lock_saved_config_at(p).ok());
            let mut saved = saved_path.map(crate::setup::load_saved_config_at);
            crate::setup::apply_connection_effort(
                config,
                saved.get_or_insert_with(Default::default),
            );
            if let (Some(path), Some(saved)) = (saved_path, saved) {
                let _ = crate::setup::save_saved_config_at(path, &saved);
            }
        }
    }
    let clamped = super::clamp_thinking_to_model(config);
    if let Some(path) = saved_path {
        let mut saved = crate::setup::load_saved_config_at(path);
        saved.model = Some(m.clone());
        let _ = crate::setup::save_saved_config_at(path, &saved);
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
        let base = config.base_url.clone();
        let key = config.api_key.clone();
        tokio::task::spawn_blocking(move || {
            crate::setup::fetch_live_provider_models(&base, key.as_deref());
        });
    }
    switch_agent_model(agent, config, cwd, session_id, tui).await;
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
            let base = config.base_url.clone();
            let key = config.api_key.clone();
            tokio::task::spawn_blocking(move || {
                crate::setup::fetch_live_provider_models(&base, key.as_deref());
            });
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

#[path = "handlers_tests.rs"]
#[cfg(test)]
mod tests;
