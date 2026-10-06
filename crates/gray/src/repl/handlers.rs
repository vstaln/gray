//! Slash-command handlers: skills, sys, model, thinking (split from `repl`).

use super::*;

/// Renders the exact text sent to the model for `/skills <name> [args]`:
/// a binding directive naming the skill, then the skill body (frontmatter
/// stripped), with the invocation args appended. Pure so both the visible
/// paste and the model turn share one string — what you see in chat is what
/// the model gets.
///
/// The directive exists because a bare body pasted mid-task reads as
/// background material: the model resumes whatever plan it was already on
/// (observed: a `/skills` invocation ignored, the pre-interrupt `ls` re-run
/// immediately after the paste). Naming the skill and stating the
/// instructions are binding — with an explicit "even mid-task, drop
/// conflicting plans" — re-anchors the turn on the skill.
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

/// Pastes the expanded skill into the chat transcript so the invocation is
/// visible: a `Skill "name"` box in the TUI, the raw body on headless.
/// Runs before the model turn, so the transcript shows the skill and then
/// the model's response to it.
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
        println!("{expanded}");
    }
}

/// `/skills enable <name>` / `/skills disable <name>` management verbs.
/// Returns `(on, name)`. Anything else (invocations, bare, extras) is `None`
/// — a skill literally named `enable`/`disable` is unreachable by design.
pub(crate) fn parse_skill_toggle(rest: &str) -> Option<(bool, String)> {
    parse_skill_name_toggle(rest)
}

/// Global auto-load switch: `/skills on` | `/skills off` (plus `enable` /
/// `disable` as aliases). `Some(on)` for a bare switch word, `None` for
/// anything else (per-skill toggles, invocations, bare, extras).
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

/// Per-skill toggle core behind [`parse_skill_toggle`]: two words
/// (`enable|disable <name>`). Bare switch words are global (see
/// [`parse_skills_auto_toggle`]) — so `enable`/`disable` alone never reach
/// here, while a skill literally named `on`/`off` stays invokable.
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

/// Global-switch core against an explicit config path (test seam): flips the
/// persisted auto flag and reports. `off` hides the model's prompt list;
/// manual `/skills <name>` still runs. `on` re-enables auto-loading and
/// clears the per-skill disabled set so it truly turns all skills on.
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

/// Toggle core against an explicit config path (test seam): validates the
/// name against discovery, flips the persisted set, and reports. Disabled
/// hides the skill from the model's prompt list; manual use still runs.
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

/// Subsystems carrying a persisted on/off master switch, mirroring
/// `skills_auto`. Each gates its autonomous path — cron fires, the gateway
/// server — while the manual path keeps working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Subsystem {
    Cron,
    Gateway,
}

impl Subsystem {
    /// The name users type (`/memory`, `/cron`, `gray gateway`).
    pub(crate) fn label(self) -> &'static str {
        match self {
            Subsystem::Cron => "cron",
            Subsystem::Gateway => "gateway",
        }
    }

    /// (on report, off effect, manual path that survives off).
    fn copy(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Subsystem::Cron => (
                "cron on — scheduled jobs will fire",
                "scheduled jobs won't fire",
                "/cron still runs them",
            ),
            Subsystem::Gateway => (
                "gateway on",
                "run/start will refuse to start",
                "status/stop still work",
            ),
        }
    }

    /// The persisted field this switch flips.
    fn field(self, saved: &mut crate::setup::SavedConfig) -> &mut Option<bool> {
        match self {
            Subsystem::Cron => &mut saved.cron_auto,
            Subsystem::Gateway => &mut saved.gw_auto,
        }
    }
}

/// Bare switch-word parse shared by the subsystem toggles: exactly one word
/// from `on|off|enable|disable` (case-insensitive). Anything else — extras,
/// names, garbage — is `None`, so each command keeps its own parse shape for
/// everything that is not a bare switch.
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

/// Toggle core against an explicit config path (test seam): flips the
/// persisted flag and reports. `off` gates the autonomous path only — the
/// manual path named in the report keeps working. `on` clears the flag so
/// the subsystem reads as enabled.
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

/// Resolves the live config path and applies a subsystem toggle, returning
/// the report (or the failure) as a string. Shared by the REPL arms and the
/// `gray gateway on|off` CLI so the path/error dance is written once.
pub(crate) fn toggle_subsystem(subsystem: Subsystem, on: bool) -> String {
    match crate::setup::saved_config_path() {
        Ok(path) => match apply_subsystem_toggle(&path, subsystem, on) {
            Ok(msg) | Err(msg) => msg,
        },
        Err(e) => format!("{e:#}"),
    }
}

/// Expands `/skills <name> [args]` (or the `/skill <name>` alias —
/// both parse to the identical payload) into a Prompt carrying the skill body
/// (Grok-style: frontmatter stripped, args appended). The same text is pasted
/// visibly into the chat transcript first,
/// so invoking a skill shows the actual skill in chat instead of silently
/// handing the model a hidden prompt. Bare `/skills` opens the skills manager
/// (TTY) or prints the text list (headless). Both list *discovered* skills
/// (global + project), not just `~/.gray/skills` installs.
/// With `local` set (Esc mid-turn), nothing is pasted or expanded — the turn
/// was cancelled, nothing talks to the model.
pub(crate) fn expand_skill_command(
    cmd: ReplCommand,
    cwd: &Path,
    tui: Option<&crate::composer::SharedTui>,
    local: bool,
) -> ReplCommand {
    // local: turn was cancelled — run everything as a no-AI no-op
    let to_prompt = |expanded: String| {
        if local {
            ReplCommand::Empty
        } else {
            ReplCommand::Prompt(expanded)
        }
    };
    let ReplCommand::Skill(payload) = cmd else {
        return cmd;
    };
    let discovered = crate::skills::discover_skills(cwd);
    let Some(rest) = payload else {
        // Bare /skills — manager on TTY (like /plugins), text list headless.
        if tui.is_some() {
            let bg = tui.as_ref().map(|s| s.lock().expect("tui lock").snapshot());
            match with_modal_sync(tui, || crate::setup::run_skills_modal(bg.as_ref(), cwd)) {
                Ok(true) => {
                    if let Some(shared) = tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.push_action("Skills updated", None);
                        t.ensure_gap();
                        let _ = t.draw();
                    }
                }
                Ok(false) => {
                    if let Some(shared) = tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.clear_draft();
                        // Dismissed picker leaves the slash card with no feedback:
                        // restore the trailing gap so it doesn't jam the input box.
                        t.ensure_gap();
                        let _ = t.draw();
                    }
                }
                Err(e) => {
                    say(tui, &format!("skills error: {e}"));
                }
            }
        } else if discovered.skills.is_empty() {
            say(tui, "no skills discovered");
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
        }
        return ReplCommand::Empty;
    };
    if let Some(on) = parse_skills_auto_toggle(&rest) {
        // Global switch — but a skill literally named `on`/`off` (or
        // `enable`/`disable`) stays invokable: an exact discovery hit wins
        // over the switch interpretation.
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
    // Esc-cancelled (`local`): no paste, no prompt — the turn is dead and
    // `to_prompt` already maps to `Empty`. Pasting here would print a skill
    // body for a turn that never runs.
    if !local {
        paste_skill_into_chat(tui, cwd, &skill.name, &expanded);
    }
    to_prompt(expanded)
}

/// Handles the `/agentsmd` command family (alias `/sys`): edit, show, reset.
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
            if let Err(e) = std::fs::write(&path, DEFAULT_SYS_PROMPT) {
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
            // Make sure the file exists before opening an editor on it.
            let initial = match load_or_create_system_prompt_at(&path) {
                Ok(b) => b,
                Err(e) => {
                    say(tui, &format!("{e}"));
                    return;
                }
            };
            // One external-editor path for TUI and headless alike: `$EDITOR`
            // when set, `vi` otherwise. Snapshot before the editor overwrites
            // in place; the TUI pauses its draw loop across the handover and
            // reflows once the editor exits.
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

/// Where `/undo` cuts: the index of the last message the *user* sent, so the
/// exchange it opened (and everything the model said in reply) goes, while
/// earlier turns stay. `None` when the conversation holds no user turn.
///
/// The cut is on a user boundary by construction, which is what keeps the
/// remaining history valid for the provider: an assistant message with tool
/// calls whose results were dropped would be a protocol error, and a tool
/// result whose call was dropped is an orphan.
pub(crate) fn undo_cut(messages: &[Message]) -> Option<usize> {
    messages
        .iter()
        .rposition(|message| message.role == gray_core::message::Role::User)
}

/// `/undo` drops the last exchange (the last user turn and everything the
/// model said after it); `/retry` is the same rewind plus sending that text
/// again, which the dispatcher does with the returned string.
///
/// Conversation only. Files the model wrote are untouched, and the pre-undo
/// transcript is archived under `~/.gray/sessions/archive/`, so this is
/// recoverable by hand. Images attached to the dropped turn are not restored
/// by `/retry`: the resent message is its text.
///
/// Returns the text to re-send (`/retry` only), or `None` when nothing was
/// dropped or the rewind failed — in which case memory and disk are both left
/// as they were, because a half-undone session is worse than none.
pub(crate) async fn handle_undo(
    retry: bool,
    agent: &mut Option<Agent>,
    session_state: &mut Option<SessionState>,
    pending_history: &mut Vec<Message>,
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
    // Disk first: a session that survived the rewind on disk but not in
    // memory would replay the dropped turn on the next resume.
    if let Some(state) = session_state
        && let Err(e) = state.store.rewind(&state.session_id, kept.len()).await
    {
        say(tui, &format!("undo failed: {e}"));
        return None;
    }
    ag.set_messages(kept.clone());
    pending_history.truncate(kept.len());
    let dropped = messages.len() - kept.len();
    say(
        tui,
        &format!("undid the last turn · {dropped} messages out of context"),
    );
    (!resent.is_empty()).then_some(resent)
}

/// Rebuilds the agent after a system-prompt change, preserving conversation history.
/// `session_id` pins the Responses `prompt_cache_key` shard: rebuilding with
/// `None` would rotate the shard mid-session and bust prefix-cache hits.
/// Build failures render via `say()` (never raw `println!` over the live viewport).
pub(crate) async fn reload_agent(
    agent: &mut Option<Agent>,
    config: &Config,
    cwd: &Path,
    session_id: Option<&str>,
    tui: Option<&crate::composer::SharedTui>,
) {
    let old = agent.take();
    let mut rebuilt = match build_agent(config, cwd, session_id).await {
        Ok(a) => a,
        Err(e) => {
            say(tui, &format!("{e}"));
            *agent = old;
            return;
        }
    };
    if let Some(old) = old {
        rebuilt = rebuilt.with_messages(old.messages().to_vec());
    }
    *agent = Some(rebuilt);
}

/// What `/model` was asked for: the picker (optionally focused on a row —
/// a plugin `model_picker` outcome like `/fusion`), or a direct id.
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

/// Handles `/model`: interactive picker (no arg) or direct set (`/model provider/id`).
/// Switching updates the live agent and persists to ~/.gray/config.json.
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
        // Validate against the cached list first; only an id it doesn't
        // know waits on the live fetch (a brand-new model). Plugin
        // connections list via `provider/models`, not the placeholder base.
        let (_, list_key, _) = crate::setup::picker_scope(config);
        let cached = crate::setup::saved_models_for(&list_key, &config.base_url);
        let mut checked = crate::setup::validate_direct_model_id(&m, &cached);
        let mut live_rows: Vec<(String, String)> = Vec::new();
        if checked.is_err() {
            let (_, known) = crate::setup::provider_models_for_config(config);
            checked = crate::setup::validate_direct_model_id(&m, &known);
            live_rows = known;
        }
        // Decompose the target once against the merged known list: a
        // declared `-fast`/`-priority` row and a stale `<base>-<tier>` id
        // alike split into base + tier + fast.
        let mut ids = cached.clone();
        for row in live_rows {
            if !ids.iter().any(|(id, _)| *id == row.0) {
                ids.push(row);
            }
        }
        let (m, variant_tier, fast) = match checked {
            Ok(canonical) => crate::setup::decompose_model_variant(&canonical, &ids)
                .unwrap_or((canonical, None, false)),
            Err(msg) => {
                // A `<base>-<tier>` id names a level on its family row: the
                // picker collapsed the variants into `base`, so a stale or
                // hand-typed `swe-2-max` resolves to `swe-2` at `max`.
                match crate::setup::decompose_model_variant(&m, &ids) {
                    Some(v) => v,
                    None => {
                        say(tui, &msg);
                        return;
                    }
                }
            }
        };
        if fast && std::env::var_os("GRAY_FAST").is_none() {
            config.fast_mode = Some(true);
            if let Ok(path) = crate::setup::saved_config_path() {
                let mut saved = crate::setup::load_saved_config_at(&path);
                saved.fast_mode = Some(true);
                let _ = crate::setup::save_saved_config_at(&path, &saved);
            }
        }
        config.model = Some(m.clone());
        // Effort follows the model: adopt this target's remembered level
        // (or the default) before clamping, so a stale level never lands on
        // the new model or gets remembered under its key. A tier the id
        // itself named wins over memory — `swe-2-max` means max.
        let prev_effort = config.thinking_effort.clone();
        match &variant_tier {
            Some(tier) if std::env::var_os("GRAY_THINKING_EFFORT").is_none() => {
                config.thinking_effort = Some(tier.clone());
                if let Ok(path) = crate::setup::saved_config_path() {
                    let mut saved = crate::setup::load_saved_config_at(&path);
                    saved.remember_effort(
                        &crate::setup::effort_memory_key(&config.provider_id, &config.base_url, &m),
                        tier,
                    );
                    saved.thinking_effort = config.thinking_effort.clone();
                    let _ = crate::setup::save_saved_config_at(&path, &saved);
                }
            }
            _ => crate::setup::adopt_connection_effort(config),
        }
        let clamped = super::clamp_thinking_to_model(config);
        if clamped.is_some() || config.thinking_effort != prev_effort {
            *hide_thinking = config.reasoning_hidden();
        }
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            saved.model = Some(m.clone());
            let _ = crate::setup::save_saved_config_at(&path, &saved);
        }
        if let Some(shared) = tui {
            let mut t = shared.lock().expect("tui lock");
            t.set_model(m.clone());
            t.set_model_label(crate::setup::composite_label_for(config));
            if config.thinking_effort != prev_effort
                && let Some(eff) = &config.thinking_effort
            {
                t.set_thinking_effort(crate::setup::effort_chip(eff, config));
                t.set_hide_thinking(*hide_thinking);
            }
            t.push_action("Model set to", Some(&m));
            if let Some((old, new)) = clamped {
                t.push_dim(format!(
                    "└ thinking effort clamped from {old} to {new} (not supported by this model)"
                ));
            }
            t.ensure_gap();
            let _ = t.draw();
        } else {
            println!("✓ Model set to {m}");
            if let Some((old, new)) = clamped {
                println!(
                    "Thinking effort clamped from {old} to {new} (not supported by this model)"
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
        reload_agent(agent, config, cwd, session_id, tui).await;
        return;
    }

    if tui.is_none() {
        // Headless (piped stdout): the picker needs a TTY — print status.
        match config.model.as_deref() {
            Some(m) => println!("model {m} — /model provider/id to switch"),
            None => {
                println!(
                    "no model configured — run /connect to set up provider & key (or /model provider/id; /help)"
                )
            }
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
            // The picker already committed the row's effort (and any Tab /
            // ctrl+r toggle); clamp whatever came out so the footer never
            // shows an unsupported level, and paint the committed state.
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
                    // Name the effort alongside the model when it has a
                    // level to choose: the picker set both at once.
                    let label = match (
                        &config.thinking_effort,
                        crate::setup::composite_label_for(config),
                    ) {
                        // A composite names its picks (`Fusion · Opus 5.5
                        // High + SWE-2 High`); fast still shows.
                        (_, Some(composite)) if config.fast_mode == Some(true) => {
                            format!("{composite} · fast")
                        }
                        (_, Some(composite)) => composite,
                        (Some(eff), None)
                            if crate::setup::supported_thinking_levels(m).len() > 1 =>
                        {
                            format!("{m} · {}", crate::setup::effort_chip(eff, config))
                        }
                        _ => m.clone(),
                    };
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
                    t.push_dim(format!(
                        "└ thinking effort clamped from {old} to {new} (not supported by this model)"
                    ));
                }
                t.ensure_gap();
                let _ = t.draw();
            } else if let Some((old, new)) = clamped {
                println!(
                    "Thinking effort clamped from {old} to {new} (not supported by this model)"
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
            reload_agent(agent, config, cwd, session_id, tui).await;
        }
        Ok(false) => {
            // Tab / ctrl+r are pending until Enter: a dismissed picker
            // changed nothing.
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.clear_draft();
                // Dismissed picker leaves the slash card with no feedback:
                // gap so it doesn't jam the input box.
                t.ensure_gap();
                let _ = t.draw();
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.push_dim(format!("└ error: {e}"));
                t.ensure_gap();
            } else {
                println!("model error: {e}");
            }
        }
    }
}

/// Where `/thinking` (aliases `/effort`, `/reasoning`) goes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ThinkingRoute {
    /// `/thinking <level>`: set the level directly.
    Set(String),
    /// Bare, headless: print the level and the model's choices.
    Status,
    /// Bare, interactive: the `/model` picker owns effort (←/→ per row).
    ModelPicker,
}

pub(crate) fn thinking_route(direct: Option<String>, interactive: bool) -> ThinkingRoute {
    match direct {
        Some(eff) => ThinkingRoute::Set(eff),
        None if interactive => ThinkingRoute::ModelPicker,
        None => ThinkingRoute::Status,
    }
}

/// Handles `/thinking` / `/effort`: direct set (`/thinking high`), a status
/// line when headless, else the `/model` picker on the live model's row.
pub(crate) async fn handle_thinking(
    config: &mut Config,
    cwd: &Path,
    direct: Option<String>,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
    hide_thinking: &mut bool,
    session_id: Option<&str>,
) {
    let route = thinking_route(direct, tui.is_some());
    if route == ThinkingRoute::ModelPicker {
        handle_model(
            config,
            cwd,
            ModelArg::Picker { focus: None },
            agent,
            tui,
            session_id,
            hide_thinking,
        )
        .await;
        return;
    }
    if let ThinkingRoute::Set(eff) = route {
        let eff_clean = eff.to_lowercase();
        // Validate against what the CURRENT model accepts, not the global
        // catalog — Prime-Agent rejects unknown-for-model levels the same way.
        let supported: Vec<&str> =
            crate::setup::supported_thinking_levels(&config.model.clone().unwrap_or_default())
                .iter()
                .map(|(l, _)| *l)
                .collect();
        if eff_clean == "off" || supported.iter().any(|l| *l == eff_clean) {
            config.thinking_effort = Some(eff_clean.clone());
            if let Ok(path) = crate::setup::saved_config_path() {
                let mut saved = crate::setup::load_saved_config_at(&path);
                saved.remember_effort(
                    &crate::setup::effort_memory_key(
                        &config.provider_id,
                        &config.base_url,
                        config.model.as_deref().unwrap_or_default(),
                    ),
                    &eff_clean,
                );
                let _ = crate::setup::save_saved_config_at(&path, &saved);
            }
            *hide_thinking = config.reasoning_hidden();
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.set_thinking_effort(crate::setup::effort_chip(&eff_clean, config));
                t.set_hide_thinking(*hide_thinking);
                t.push_action("Thinking effort set to", Some(&eff_clean));
                t.ensure_gap();
                let _ = t.draw();
            } else {
                println!("✓ Thinking effort set to {eff_clean}");
            }
            reload_agent(agent, config, cwd, session_id, tui).await;
            return;
        }
        let msg = format!(
            "unknown level '{eff_clean}' for this model — try: {}",
            supported.join(", ")
        );
        if let Some(shared) = tui {
            let mut t = shared.lock().expect("tui lock");
            t.push_dim(format!("└ {msg}"));
            t.ensure_gap();
        } else {
            println!("{msg}");
        }
        return;
    }

    // Headless (piped stdout): the picker needs a TTY — print status.
    // Same provider-driven filter as the picker rows, so piped output
    // agrees with what `/model` would offer on a TTY.
    let model = config.model.clone().unwrap_or_default();
    let levels = crate::setup::supported_thinking_levels(&model)
        .iter()
        .map(|(l, _)| *l)
        .collect::<Vec<_>>()
        .join(", ");
    let cur = config
        .thinking_effort
        .clone()
        .unwrap_or_else(|| "default (high)".to_string());
    println!("thinking effort: {cur} — levels: {levels}; /thinking <level> to set");
}

/// Handles `/fast [on|off|status]`: prefer the provider's fast-serving
/// variant of the current model (`-fast`/`-priority` catalog rows). Bare
/// `/fast` toggles; `status` reports without changing. The wire model id
/// composes at agent build, so toggling reloads the agent.
pub(crate) async fn handle_fast(
    config: &mut Config,
    cwd: &Path,
    direct: Option<String>,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
    session_id: Option<&str>,
) {
    let arg = direct.as_deref().unwrap_or("").trim().to_lowercase();
    let current = config.fast_mode == Some(true);
    let (next, report_only) = match arg.as_str() {
        "" => (!current, false),
        "on" => (true, false),
        "off" => (false, false),
        "status" => (current, true),
        _ => {
            say(
                tui,
                "usage: /fast [on|off|status] — prefer the provider's fast model variant",
            );
            return;
        }
    };
    // What the flag would send — the mode is global, but only catalog fast
    // rows change the wire id, so report the honest outcome for this model.
    let model = config.model.clone().unwrap_or_default();
    let wire = crate::setup::fast_wire_for(config);
    let detail = if next {
        match &wire {
            Some(id) => format!("{model} → {id}"),
            None => format!("{model} has no fast variant — sends unchanged"),
        }
    } else {
        model.clone()
    };
    if !report_only {
        config.fast_mode = Some(next);
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            saved.fast_mode = Some(next);
            let _ = crate::setup::save_saved_config_at(&path, &saved);
        }
    }
    let verb = if next { "on" } else { "off" };
    let msg = if report_only {
        format!("fast mode is {verb} — {detail}")
    } else {
        format!("fast mode {verb} — {detail}")
    };
    if let Some(shared) = tui {
        let mut t = shared.lock().expect("tui lock");
        if let Some(eff) = &config.thinking_effort {
            t.set_thinking_effort(crate::setup::effort_chip(eff, config));
        }
        t.push_action("Fast mode", Some(verb));
        t.push_dim(format!("└ {msg}"));
        t.ensure_gap();
        let _ = t.draw();
    } else {
        println!("{msg}");
    }
    if !report_only {
        reload_agent(agent, config, cwd, session_id, tui).await;
    }
}

#[path = "handlers_tests.rs"]
#[cfg(test)]
mod tests;
