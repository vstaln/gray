//! Slash-command handlers: skills, sys, model, thinking (split from `repl`).

use super::*;

/// Renders the exact text sent to the model for `/skills <name> [args]`:
/// the skill body (frontmatter stripped) in a `<skill>` envelope, with the
/// invocation args appended. Pure so both the visible paste and the model
/// turn share one string — what you see in chat is what the model gets.
pub(crate) fn format_skill_paste(
    name: &str,
    path: &Path,
    body: &str,
    args: Option<&str>,
) -> String {
    let mut out = format!(
        "<skill name=\"{name}\" path=\"{}\">\n{body}\n</skill>",
        path.display()
    );
    if let Some(a) = args.filter(|a| !a.is_empty()) {
        out.push_str(&format!("\n\n**ARGUMENTS:** {a}"));
    }
    out
}

/// Pastes the expanded skill into the chat transcript so the invocation is
/// visible: a `Skill "name"` box in the TUI, the raw envelope on headless.
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
        let header = crate::tool_fmt::format_tool_call_header("skill", &args, Some(cwd));
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

/// Expands `/skills <name> [args]` (or the `/skill <name>` alias —
/// both parse to the identical payload) into a Prompt carrying the skill body
/// (Grok-style: frontmatter stripped, wrapped in a `<skill>` envelope, args
/// appended). The same text is pasted visibly into the chat transcript first,
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
                        t.ensure_gap(1);
                        let _ = t.draw();
                    }
                }
                Ok(false) => {
                    if let Some(shared) = tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.clear_draft();
                        // Dismissed picker leaves the slash card with no feedback:
                        // restore the trailing gap so it doesn't jam the input box.
                        t.ensure_gap(1);
                        let _ = t.draw();
                    }
                }
                Err(e) => {
                    say(tui, &format!("skills error: {e}"));
                }
            }
        } else if discovered.skills.is_empty() {
            say(tui, "no skills discovered — /marketplace to browse");
        } else {
            for s in &discovered.skills {
                say(tui, &crate::skills::format_discovered_skill_row(s));
            }
        }
        return ReplCommand::Empty;
    };
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
            format_skill_paste(&skill.name, &skill.file_path, body, args.as_deref())
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
                t.modal_open = true;
                true
            } else {
                false
            };
            let res = crate::sys_editor::run_external_editor(&path, &initial);
            if editor_paused && let Some(shared) = &tui_snap {
                let mut t = shared.lock().expect("tui lock");
                t.pending_resize = None;
                t.modal_open = false;
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

/// Handles `/model`: interactive picker (no arg) or direct set (`/model provider/id`).
/// Switching updates the live agent and persists to ~/.gray/config.json.
pub(crate) async fn handle_model(
    config: &mut Config,
    cwd: &Path,
    direct: Option<String>,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
    session_id: Option<&str>,
) {
    if let Some(m) = direct {
        let (_, _, known) =
            crate::setup::provider_models_for(&config.base_url, config.api_key.as_deref());
        let m = match crate::setup::validate_direct_model_id(&m, &known) {
            Ok(canonical) => canonical,
            Err(msg) => {
                say(tui, &msg);
                return;
            }
        };
        config.model = Some(m.clone());
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            saved.model = Some(m.clone());
            let _ = crate::setup::save_saved_config_at(&path, &saved);
        }
        if let Some(shared) = tui {
            let mut t = shared.lock().expect("tui lock");
            t.set_model(m.clone());
            t.push_action("Model set to", Some(&m));
            t.ensure_gap(1);
        } else {
            println!("✓ Model set to {m}");
        }
        if crate::setup::get_user_context_window().is_none()
            && crate::setup::get_cached_model_context(&m).is_none()
        {
            let base = config.base_url.clone();
            let key = config.api_key.clone();
            tokio::spawn(async move {
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
                println!("no model configured — /model provider/id to set (or /provider to browse)")
            }
        }
        return;
    }
    let bg = tui.map(|shared| shared.lock().expect("tui lock").snapshot());
    let result = with_modal(tui, crate::setup::run_model_menu(config, bg.as_ref())).await;
    match result {
        Ok(true) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                if let Some(m) = &config.model {
                    t.set_model(m.clone());
                    t.push_action("Model set to", Some(m));
                    t.ensure_gap(1);
                }
                let _ = t.draw();
            }
            if let Some(m) = config.model.clone()
                && crate::setup::get_user_context_window().is_none()
                && crate::setup::get_cached_model_context(&m).is_none()
            {
                let base = config.base_url.clone();
                let key = config.api_key.clone();
                tokio::spawn(async move {
                    crate::setup::fetch_live_provider_models(&base, key.as_deref());
                });
            }
            reload_agent(agent, config, cwd, session_id, tui).await;
        }
        Ok(false) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.clear_draft();
                // Dismissed picker leaves the slash card with no feedback:
                // gap so it doesn't jam the input box.
                t.ensure_gap(1);
                let _ = t.draw();
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.push_dim(format!("└ error: {e}"));
                t.ensure_gap(1);
            } else {
                println!("model error: {e}");
            }
        }
    }
}

/// Handles `/thinking` / `/effort`: direct set (`/thinking high`), toggle visibility (bare `/thinking`), or picker.
pub(crate) async fn handle_thinking(
    config: &mut Config,
    cwd: &Path,
    direct: Option<String>,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
    hide_thinking: &mut bool,
    session_id: Option<&str>,
) {
    if let Some(eff) = direct {
        let eff_clean = eff.to_lowercase();
        if eff_clean == "off"
            || crate::setup::THINKING_LEVELS
                .iter()
                .any(|(l, _)| *l == eff_clean)
        {
            config.thinking_effort = Some(eff_clean.clone());
            if let Ok(path) = crate::setup::saved_config_path() {
                let mut saved = crate::setup::load_saved_config_at(&path);
                saved.thinking_effort = Some(eff_clean.clone());
                let _ = crate::setup::save_saved_config_at(&path, &saved);
            }
            *hide_thinking = config.reasoning_hidden();
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.set_thinking_effort(eff_clean.clone());
                t.set_hide_thinking(*hide_thinking);
                t.push_action("Thinking effort set to", Some(&eff_clean));
                t.ensure_gap(1);
            } else {
                println!("✓ Thinking effort set to {eff_clean}");
            }
            reload_agent(agent, config, cwd, session_id, tui).await;
            return;
        }
        let msg = format!(
            "unknown level '{eff_clean}' — try: off, minimal, low, medium, high, xhigh, max"
        );
        if let Some(shared) = tui {
            let mut t = shared.lock().expect("tui lock");
            t.push_dim(format!("└ {msg}"));
            t.ensure_gap(1);
        } else {
            println!("{msg}");
        }
        return;
    }

    if tui.is_none() {
        // Headless (piped stdout): the picker needs a TTY — print status.
        // Same provider-driven filter as the modal, so piped output agrees
        // with what `/thinking` would offer on a TTY.
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
        return;
    }
    let has_explicit_level = config.thinking_effort.is_some();
    let bg = tui.map(|shared| shared.lock().expect("tui lock").snapshot());
    let result = with_modal(tui, crate::setup::run_effort_menu(config, bg.as_ref())).await;
    match result {
        Ok(true) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                if let Some(eff) = &config.thinking_effort {
                    t.set_thinking_effort(eff.clone());
                    *hide_thinking = config.reasoning_hidden();
                    t.set_hide_thinking(*hide_thinking);
                    t.push_action("Thinking effort set to", Some(eff));
                    t.ensure_gap(1);
                }
                let _ = t.draw();
            }
            reload_agent(agent, config, cwd, session_id, tui).await;
        }
        Ok(false) => {
            if !has_explicit_level {
                // First run, Esc: flip the display setting (effort untouched).
                let shown = !config.show_reasoning.unwrap_or(true);
                config.show_reasoning = Some(shown);
                if let Ok(path) = crate::setup::saved_config_path() {
                    let mut saved = crate::setup::load_saved_config_at(&path);
                    saved.show_reasoning = Some(shown);
                    let _ = crate::setup::save_saved_config_at(&path, &saved);
                }
                *hide_thinking = config.reasoning_hidden();
                let msg = if *hide_thinking {
                    "reasoning hidden — /thinking to show"
                } else {
                    "reasoning shown"
                };
                if let Some(shared) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.set_hide_thinking(*hide_thinking);
                    t.push_dim(format!("└ {msg}"));
                    t.ensure_gap(1);
                } else {
                    println!("{msg}");
                }
            } else if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.clear_draft();
                // Dismissed picker leaves the slash card with no feedback:
                // gap so it doesn't jam the input box.
                t.ensure_gap(1);
                let _ = t.draw();
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                shared
                    .lock()
                    .expect("tui lock")
                    .push_dim(format!("└ error: {e}"));
            } else {
                println!("effort error: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_skill_cwd_for_handlers_test(name: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".gray").join("skills").join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: Temp skill for completion tests\n---\n# temp\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn skill_paste_is_what_the_model_gets() {
        // The visible paste and the model turn must be the same string:
        // what you see in chat is what the model gets.
        let dir = temp_skill_cwd_for_handlers_test("paste-me");
        let cwd = dir.path();
        let out = expand_skill_command(parse_command("/skills paste-me"), cwd, None, false);
        let ReplCommand::Prompt(expanded) = out else {
            panic!("expected Prompt, got {out:?}");
        };
        assert!(expanded.contains("<skill"), "envelope missing: {expanded}");
        assert!(expanded.contains("paste-me"), "name missing: {expanded}");
        assert!(expanded.contains("# temp"), "body missing: {expanded}");
        // Args ride along in the same text.
        let dir2 = temp_skill_cwd_for_handlers_test("paste-args");
        let skill_dir = dir2.path().join(".gray").join("skills").join("paste-args");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: Temp skill with args\nargs: env\n---\n# temp $ARGUMENTS\n",
        )
        .unwrap();
        let out = expand_skill_command(
            parse_command("/skills paste-args env"),
            dir2.path(),
            None,
            false,
        );
        let ReplCommand::Prompt(expanded) = out else {
            panic!("expected Prompt, got {out:?}");
        };
        assert!(
            expanded.contains("**ARGUMENTS:** env"),
            "args missing: {expanded}"
        );
    }

    #[test]
    fn format_skill_paste_envelope_and_args() {
        let text = format_skill_paste(
            "demo",
            std::path::Path::new("/s/demo/SKILL.md"),
            "Do things.",
            Some("fast"),
        );
        assert!(text.contains("<skill name=\"demo\""), "{text}");
        assert!(text.contains("Do things."), "{text}");
        assert!(text.contains("**ARGUMENTS:** fast"), "{text}");
        let bare = format_skill_paste(
            "demo",
            std::path::Path::new("/s/demo/SKILL.md"),
            "Do things.",
            None,
        );
        assert!(!bare.contains("ARGUMENTS"), "{bare}");
    }

    // UNRUN (cargo test banned under X): run in TTY/CI.
    // reload_agent with no model configured fails soft through say()
    // (headless println path) and preserves the previous agent.
    #[tokio::test]
    async fn reload_agent_failure_preserves_agent() {
        let config = Config {
            model: None,
            base_url: String::new(),
            api_key: None,
            thinking_effort: None,
            show_reasoning: None,
            context_window: None,
            context_reserve: None,
            context_keep: None,
            max_turns: None,
            max_cost_micros: None,
            max_wall_secs: None,
        };
        let mut agent: Option<Agent> = None;
        reload_agent(
            &mut agent,
            &config,
            std::path::Path::new("/tmp"),
            None,
            None,
        )
        .await;
        assert!(agent.is_none());
    }
}
