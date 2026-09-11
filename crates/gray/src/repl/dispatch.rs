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
    acp: &mut Option<AcpSession>,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &TuiOpt,
    session_state: &mut Option<SessionState>,
    session_totals: &mut SessionTotals,
    pending_command: &mut Option<ReplCommand>,
    pending_history: &mut Vec<Message>,
    unconfigured: &mut bool,
    hide_thinking: &mut bool,
    approval_gate: &gray_core::approvals::ApprovalGate,
) -> anyhow::Result<Flow> {
    Ok(match cmd {
        ReplCommand::Empty | ReplCommand::Prompt(_) => Flow::Continue,
        ReplCommand::Quit => {
            shutdown_hooks(agent.as_ref()).await;
            let _ = acp.take(); // AcpSession has no teardown.
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
                session_state.as_ref().map(|s| s.session_id.as_str()),
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
                session_state.as_ref().map(|s| s.session_id.as_str()),
            )
            .await;
            Flow::Continue
        }
        ReplCommand::Help => {
            if let Some((shared, _)) = tui {
                let mut out = String::new();
                for d in REGISTRY {
                    out.push_str(&commands::format_help_line(d));
                    out.push('\n');
                }
                if let Some(a) = agent.as_ref() {
                    for (n, d) in plugin_help_entries(a.hooks()) {
                        out.push_str(&format!("  /{n:<10} {d}\n"));
                    }
                }
                let mut t = shared.lock().expect("tui lock");
                t.push_dim(out.trim_end().to_string());
                t.ensure_gap(1);
            } else {
                println!("{}", crate::rule("commands"));
                for d in REGISTRY {
                    println!("{}", commands::format_help_line(d));
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
            // Sticky ACP mode survives /new on a fresh agent thread.
            if let Some(s) = acp.as_mut() {
                let t = tui.as_ref().map(|(s, _)| s);
                match s.new_session().await {
                    Ok(()) => say(t, &format!("acp:{} new thread", s.agent_key())),
                    Err(e) => say(t, &format!("acp new thread failed: {e:#}")),
                }
            }
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
            // T3.4 lifecycle: the new session saw nothing yet (fresh builds
            // start empty; clear anyway so shared state can't leak across).
            if let Some(ledger) = gray_plugin::builder::current_file_ledger() {
                ledger.clear();
            }

            if let Some((shared, _)) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.reset_usage();
                let detail = if !short_id.is_empty() {
                    Some(format!("({short_id})"))
                } else {
                    None
                };
                t.push_action("New conversation started", detail.as_deref());
                t.ensure_gap(1);
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
                session_state.as_ref().map(|s| s.session_id.as_str()),
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
        ReplCommand::Permissions(mode) => {
            handle_permissions(config, mode, approval_gate, tui.as_ref().map(|(s, _)| s));
            Flow::Continue
        }
        ReplCommand::Feedback(text) => {
            handle_feedback(text, config, session_state, tui.as_ref().map(|(s, _)| s));
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
                        let prov_name = crate::setup::popular_provider_name(&config.base_url)
                            .unwrap_or_else(|| "provider".to_string());
                        t.push_dim(format!("└ connected to {prov_name} · {model_str}"));
                        t.ensure_gap(1);
                        let _ = t.draw();
                    }
                    reload_agent(
                        &mut *agent,
                        config,
                        cwd,
                        session_state.as_ref().map(|s| s.session_id.as_str()),
                        tui.as_ref().map(|(s, _)| s),
                    )
                    .await;
                }
                Ok(false) => {
                    if let Some((shared, _)) = tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.clear_draft();
                        // Dismissed picker leaves the slash card with no
                        // feedback: gap so it doesn't jam the input box.
                        t.ensure_gap(1);
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
        ReplCommand::Acp(raw) => {
            #[cfg(feature = "acp")]
            handle_acp_command(
                &raw,
                cwd,
                tui.as_ref().map(|(s, _)| s),
                &mut *acp,
                config.model.as_deref(),
            )
            .await;
            #[cfg(not(feature = "acp"))]
            {
                let _ = (&raw, &mut *acp, config);
                say(
                    tui.as_ref().map(|(s, _)| s),
                    "acp support is not compiled in this build — rebuild with `--features acp`",
                );
            }
            Flow::Continue
        }
        ReplCommand::Plugin(raw) => {
            handle_plugin_command(&raw, tui.as_ref().map(|(s, _)| s)).await;
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
                // The gateway left the TUI (kept as the `gray gateway` CLI):
                // point muscle memory at it instead of the generic unknown.
                let first = cmd[1..].split_whitespace().next().unwrap_or("");
                if first == "gateway" || first == "gw" {
                    say(
                        tui.as_ref().map(|(s, _)| s),
                        "the TUI gateway is gone — run `gray gateway …` outside gray",
                    );
                } else {
                    say(
                        tui.as_ref().map(|(s, _)| s),
                        &format!("unknown command '{cmd}' — type /help for available commands"),
                    );
                }
            }
            Flow::Continue
        }
    })
}
