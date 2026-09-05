//! REPL slash-command dispatch (split from `repl`).

use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Flow {
    Continue,
    Break,
}

#[allow(clippy::too_many_arguments)]
// Mechanical split of `run_repl_mode`: params are the loop state the arms borrow.
pub(crate) async fn dispatch_command(
    cmd: ReplCommand,
    agent: &mut Option<Agent>,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &TuiOpt,
    session_state: &mut Option<SessionState>,
    session_totals: &mut SessionTotals,
    pending_command: &mut Option<ReplCommand>,
    pending_history: &mut Vec<Message>,
    unconfigured: &mut bool,
    hide_thinking: &mut bool,
) -> anyhow::Result<Flow> {
    Ok(match cmd {
        ReplCommand::Empty | ReplCommand::Prompt(_) => Flow::Continue,
        ReplCommand::Quit => {
            shutdown_hooks(agent.as_ref()).await;
            if let Some((shared, stop)) = tui {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                let mut t = shared.lock().expect("tui lock");
                t.shutdown();
                print_exit_hint(session_state);
            } else {
                print_exit_hint(session_state);
            }
            Flow::Break
        }
        ReplCommand::Sys(action) => {
            handle_sys(
                config,
                cwd,
                action,
                &mut *agent,
                tui.as_ref().map(|(s, _)| s),
            )
            .await;
            Flow::Continue
        }
        ReplCommand::Model(direct) => {
            handle_model(
                config,
                cwd,
                direct,
                &mut *agent,
                tui.as_ref().map(|(s, _)| s),
            )
            .await;
            Flow::Continue
        }
        ReplCommand::Help => {
            if let Some((shared, _)) = tui {
                let mut out = String::new();
                for d in REGISTRY {
                    out.push_str(&format!("  /{:<10} {}\n", d.name, d.desc));
                }
                if let Some(a) = agent.as_ref() {
                    for (n, d) in plugin_help_entries(a.hooks()) {
                        out.push_str(&format!("  /{n:<10} {d}\n"));
                    }
                }
                shared
                    .lock()
                    .expect("tui lock")
                    .push_dim(out.trim_end().to_string());
            } else {
                println!("{}", crate::rule("commands"));
                for d in REGISTRY {
                    println!("  /{:<8} {}", d.name, d.desc);
                }
                if let Some(a) = agent.as_ref() {
                    for (n, d) in plugin_help_entries(a.hooks()) {
                        println!("  /{n:<8} {d}");
                    }
                }
            }
            Flow::Continue
        }
        ReplCommand::Resume(args) => {
            handle_resume(
                config,
                cwd,
                args,
                &mut *agent,
                &mut *session_state,
                &mut *session_totals,
                tui.as_ref().map(|(s, _)| s),
            )
            .await;
            Flow::Continue
        }
        ReplCommand::New(initial_prompt) => {
            shutdown_hooks(agent.as_ref()).await;
            pending_history.clear();
            *session_totals = SessionTotals::default();
            *session_state = None;
            let mut short_id = String::new();
            let mut new_sid: Option<SessionId> = None;
            if let Some(root) = default_root() {
                let store = JsonlSessionStore::new(root);
                let session_id = SessionId::generate();
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let meta = SessionMeta::new(
                    session_id.clone(),
                    timestamp,
                    cwd.to_path_buf(),
                    config.model.clone().unwrap_or_else(|| "unset".into()),
                );
                if let Err(e) = store.create(meta).await {
                    log::warn!(target: "gray_session", "session create failed: {e}");
                }
                short_id = session_id
                    .as_str()
                    .split('-')
                    .next()
                    .unwrap_or("new")
                    .to_string();
                new_sid = Some(session_id.clone());
                *session_state = Some(SessionState { store, session_id });
            }
            // Build with the new session id so the prompt-cache shard
            // survives future resumes of this session.
            *agent = build_agent(config, cwd, new_sid.as_ref().map(|s| s.as_str()))
                .await
                .ok();

            if let Some((shared, _)) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.reset_usage();
                let detail = if !short_id.is_empty() {
                    Some(format!("({short_id})"))
                } else {
                    None
                };
                t.push_action("New conversation started", detail.as_deref());
            } else {
                if !short_id.is_empty() {
                    println!("✓ New conversation started ({short_id})");
                } else {
                    println!("✓ New conversation started");
                }
            }

            if let Some(prompt_text) = initial_prompt {
                if let Some((shared, _)) = tui {
                    shared.lock().expect("tui lock").push_user_prompt(
                        &prompt_text,
                        &[],
                        !prompt_text.starts_with('/'),
                    );
                } else {
                    println!("❯ {prompt_text}");
                }
                *pending_command = Some(ReplCommand::Prompt(prompt_text));
            }
            Flow::Continue
        }
        ReplCommand::Compact(instructions) => {
            handle_compact(
                config,
                cwd,
                instructions,
                &mut *agent,
                &mut *session_state,
                tui.as_ref().map(|(s, _)| s),
            )
            .await;
            Flow::Continue
        }
        ReplCommand::Thinking(level) => {
            handle_thinking(
                config,
                cwd,
                level,
                &mut *agent,
                tui.as_ref().map(|(s, _)| s),
                &mut *hide_thinking,
            )
            .await;
            Flow::Continue
        }
        ReplCommand::ContextWindow(val) => {
            handle_context_window(config, cwd, agent, val, tui.as_ref().map(|(s, _)| s)).await;
            Flow::Continue
        }
        ReplCommand::Usage => {
            handle_usage(session_totals, config, tui.as_ref().map(|(s, _)| s));
            Flow::Continue
        }
        ReplCommand::Provider => {
            let bg = tui
                .as_ref()
                .map(|(shared, _)| shared.lock().expect("tui lock").snapshot());
            let result = with_modal(
                tui.as_ref().map(|(s, _)| s),
                crate::setup::run_provider_menu(config, bg.as_ref()),
            )
            .await;
            match result {
                Ok(true) => {
                    *unconfigured = false;
                    if let Some((shared, _)) = tui {
                        let mut t = shared.lock().expect("tui lock");
                        if let Some(m) = &config.model {
                            t.set_model(m.clone());
                        }
                        let model_str = config.model.as_deref().unwrap_or("default");
                        let catalog = crate::setup::load_catalog().ok();
                        let prov_name = catalog
                            .as_ref()
                            .and_then(|c| c.values().find(|p| p.base_url == config.base_url))
                            .map(|p| p.name.as_str())
                            .unwrap_or("provider");
                        t.push_dim(format!("└ connected to {prov_name} · {model_str}"));
                        let _ = t.draw();
                    }
                    reload_agent(&mut *agent, config, cwd).await;
                }
                Ok(false) => {
                    if let Some((shared, _)) = tui {
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
                    if let Some((shared, _)) = tui {
                        shared
                            .lock()
                            .expect("tui lock")
                            .push_dim(format!("└ error: {e}"));
                    } else {
                        println!("provider error: {e}");
                    }
                }
            }
            Flow::Continue
        }
        ReplCommand::Skill(_) => {
            // fully expanded into Prompt/Empty by expand_skill_command; defensive no-op
            Flow::Continue
        }
        ReplCommand::Gateway(raw) => {
            let t = tui.as_ref().map(|(s, _)| s);
            // bare /gateway in the TUI opens the interactive picker; the
            // modal returns an equivalent command string to execute
            let cmd = if raw.trim() == "/gateway" && t.is_some() {
                let bg = t.map(|s| s.lock().expect("tui lock").snapshot());
                let running = GATEWAY_HANDLE.lock().map(|g| g.is_some()).unwrap_or(false);
                match with_modal_sync(t, || crate::setup::run_gateway_modal(bg.as_ref(), running)) {
                    Ok(Some(c)) => c,
                    _ => String::new(),
                }
            } else {
                raw.clone()
            };
            if !cmd.is_empty() {
                handle_gateway(&cmd, t).await;
            }
            Flow::Continue
        }
        ReplCommand::Unknown(cmd) => {
            // Protocol v1 `command/run`: a claimed `/cmd` runs on its
            // owning plugin; anything else keeps the unknown message.
            let hooks: Vec<Arc<dyn PluginHooks>> = agent
                .as_ref()
                .map(|a| a.hooks().to_vec())
                .unwrap_or_default();
            let mut handled = false;
            if let Some((name, argv)) = split_plugin_command(&cmd)
                && let Some(outcome) = run_plugin_command(&hooks, &name, argv).await
            {
                match outcome {
                    CommandOutcome::Say(text) => {
                        say(tui.as_ref().map(|(s, _)| s), &text);
                    }
                    // Same path as a typed prompt: the next loop
                    // iteration dispatches `ReplCommand::Prompt` (no
                    // turn-running logic duplicated here).
                    CommandOutcome::Prompt(prompt) => {
                        *pending_command = Some(ReplCommand::Prompt(prompt));
                    }
                }
                handled = true;
            }
            if !handled {
                say(
                    tui.as_ref().map(|(s, _)| s),
                    &format!("unknown command '{cmd}' — type /help for available commands"),
                );
            }
            Flow::Continue
        }
    })
}
