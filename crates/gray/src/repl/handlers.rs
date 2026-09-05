//! Slash-command handlers: skills, sys, model, thinking (split from `repl`).

use super::*;

/// Expands `/skills:<name> [args]` into a Prompt carrying the skill body
/// (Grok-style: frontmatter stripped, wrapped in a `<skill>` envelope, args
/// appended). Bare `/skills` opens an interactive picker like /resume.
/// With `local` set (Esc mid-turn), the skill is announced but never expanded
/// into an AI prompt — the turn was cancelled, nothing talks to the model.
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
        // Bare /skills — interactive picker (EnterAlternateScreen, like /resume)
        let bg = tui.as_ref().map(|s| s.lock().expect("tui lock").snapshot());
        let picked = match with_modal_sync(tui, || crate::setup::run_skills_modal(cwd, bg.as_ref()))
        {
            Ok(v) => v,
            Err(e) => {
                say(tui, &format!("skills picker error: {e}"));
                return ReplCommand::Empty;
            }
        };
        let Some((skill, picked_args)) = picked else {
            // Esc — picker cancelled; viewport already restored
            return ReplCommand::Empty;
        };
        // load skill body and optionally append args part from query
        let expanded = match std::fs::read_to_string(&skill.file_path) {
            Ok(content) => {
                let body = crate::skills_tool::strip_frontmatter(&content);
                let mut out = format!(
                    "<skill name=\"{}\" path=\"{}\">\n{}\n</skill>",
                    skill.name,
                    skill.file_path.display(),
                    body
                );
                if !picked_args.trim().is_empty() {
                    out.push_str(&format!("\n\n**ARGUMENTS:** {}", picked_args.trim()));
                }
                out
            }
            Err(e) => {
                say(
                    tui,
                    &format!("failed to read {}: {e}", skill.file_path.display()),
                );
                return ReplCommand::Empty;
            }
        };
        // surface a dim line like resume does so user sees the pick
        say(
            tui,
            &format!(
                "→ /skills:{} {}",
                skill.name,
                if picked_args.trim().is_empty() {
                    String::new()
                } else {
                    picked_args.trim().to_string()
                }
            ),
        );
        return to_prompt(expanded);
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
    let expanded = match std::fs::read_to_string(&skill.file_path) {
        Ok(content) => {
            let body = crate::skills_tool::strip_frontmatter(&content);
            let mut out = format!(
                "<skill name=\"{}\" path=\"{}\">\n{}\n</skill>",
                skill.name,
                skill.file_path.display(),
                body
            );
            if let Some(a) = args.as_deref().filter(|a| !a.is_empty()) {
                out.push_str(&format!("\n\n**ARGUMENTS:** {a}"));
            }
            out
        }
        Err(e) => {
            say(
                tui,
                &format!("failed to read {}: {e}", skill.file_path.display()),
            );
            return ReplCommand::Empty;
        }
    };
    to_prompt(expanded)
}

/// Handles the `/agentsmd` command family (alias `/sys`): edit, show, reset.
pub(crate) async fn handle_sys(
    config: &Config,
    cwd: &Path,
    action: SysAction,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
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
            if let Err(e) = std::fs::write(&path, DEFAULT_SYS_PROMPT) {
                say(tui, &format!("failed to reset {}: {e}", path.display()));
                return;
            }
            say(
                tui,
                &format!("✓ system prompt restored to default ({})", path.display()),
            );
            reload_agent(agent, config, cwd).await;
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
            let mut editor = crate::sys_editor::SysEditor::new(&initial, &path);
            let res = editor.run();
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
                Ok(Some(saved)) => {
                    if let Err(e) = std::fs::write(&path, &saved) {
                        say(tui, &format!("failed to save {}: {e}", path.display()));
                        return;
                    }
                    say(
                        tui,
                        "✓ system prompt saved — applies from your next message",
                    );
                    reload_agent(agent, config, cwd).await;
                }
                Ok(None) => {
                    say(tui, "prompt unchanged");
                }
                Err(e) => {
                    say(tui, &format!("editor error: {e}"));
                }
            }
        }
    }
}

/// Rebuilds the agent after a system-prompt change, preserving conversation history.
pub(crate) async fn reload_agent(agent: &mut Option<Agent>, config: &Config, cwd: &Path) {
    let old = agent.take();
    let mut rebuilt = match build_agent(config, cwd, None).await {
        Ok(a) => a,
        Err(e) => {
            println!("{e}");
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
) {
    if let Some(m) = direct {
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
        reload_agent(agent, config, cwd).await;
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
            reload_agent(agent, config, cwd).await;
        }
        Ok(false) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.textarea.set_text("");
                t.matches.clear();
                t.sel = 0;
                t.history_idx = None;
                t.draft.clear();
                t.attachments.clear();
                t.pending_pastes.clear();
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
            } else {
                println!("✓ Thinking effort set to {eff_clean}");
            }
            reload_agent(agent, config, cwd).await;
            return;
        }
        let msg = format!(
            "unknown level '{eff_clean}' — try: off, minimal, low, medium, high, xhigh, max"
        );
        if let Some(shared) = tui {
            shared
                .lock()
                .expect("tui lock")
                .push_dim(format!("└ {msg}"));
        } else {
            println!("{msg}");
        }
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
                }
                let _ = t.draw();
            }
            reload_agent(agent, config, cwd).await;
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
                } else {
                    println!("{msg}");
                }
            } else if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.textarea.set_text("");
                t.matches.clear();
                t.sel = 0;
                t.history_idx = None;
                t.draft.clear();
                t.attachments.clear();
                t.pending_pastes.clear();
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
