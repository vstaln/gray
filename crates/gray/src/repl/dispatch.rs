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
                &mut *hide_thinking,
            )
            .await;
            Flow::Continue
        }
        ReplCommand::Help => {
            // Build the registry lines once; the branches differ only in how
            // they render (scrollback dim block vs stdout).
            let mut out = String::new();
            for d in REGISTRY {
                out.push_str(&commands::format_help_line(d));
                out.push('\n');
            }
            let mut entries = crate::plugin_cli::completions("");
            if let Some(a) = agent.as_ref() {
                entries.extend(plugin_help_entries(a.hooks()));
            }
            let mut seen: std::collections::HashSet<String> =
                REGISTRY.iter().map(|d| d.name.to_string()).collect();
            for (name, description) in entries {
                if seen.insert(name.clone()) {
                    out.push_str(&format!("  /{name:<10} {description}\n"));
                }
            }
            if let Some((shared, _)) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.push_dim(out.trim_end().to_string());
                t.ensure_gap(1);
            } else {
                println!("{}", crate::rule("commands"));
                print!("{out}");
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
                &mut *hide_thinking,
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
                let timestamp = crate::print::now_millis();
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
                *session_state = Some(SessionState {
                    full_save_pending: false,
                    store,
                    session_id,
                });
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
        ReplCommand::CronJobs(arg) => {
            let store = crate::cron::CronStore::open(crate::setup::gray_home()?.join("cron"))?;
            let jobs = store.list()?;
            let now = crate::cron::now_secs();
            // Health is store-level (one ticker serves every job); a read
            // failure only drops the liveness line, never the listing.
            let health = store.health(now).ok();
            let text = match arg {
                None => super::cron::format_cron_dashboard(&jobs, health.as_ref(), now),
                Some(id) => match jobs.into_iter().find(|j| j.id == id || j.name == id) {
                    Some(j) => super::cron::format_cron_dashboard(&[j], health.as_ref(), now),
                    None => format!("unknown cron job {id:?}"),
                },
            };
            say(tui.as_ref().map(|(s, _)| s), &text);
            Flow::Continue
        }
        ReplCommand::Copy => {
            handle_copy(agent, tui.as_ref().map(|(s, _)| s));
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
            let result = with_modal_sync(tui.as_ref().map(|(s, _)| s), || {
                crate::setup::run_connect_modal(config, bg.as_ref())
            });
            match result {
                Ok(true) => {
                    *unconfigured = false;
                    push_provider_connected(config, tui, Some(&mut *hide_thinking));
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
            let Some((name, argv)) = split_plugin_command(&cmd) else {
                say(
                    tui.as_ref().map(|(s, _)| s),
                    "invalid plugin command: check quotes and escapes",
                );
                return Ok(Flow::Continue);
            };
            let mut handled = false;
            if let Some(outcome) = run_plugin_command(&hooks, &name, argv.clone()).await {
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
                match crate::plugin_cli::capture_slash(name.trim_start_matches('/'), &argv).await {
                    Ok(Some(text)) => {
                        say(tui.as_ref().map(|(s, _)| s), text.trim_end());
                        handled = true;
                    }
                    Err(e) => {
                        say(
                            tui.as_ref().map(|(s, _)| s),
                            &format!("plugin command failed: {e}"),
                        );
                        handled = true;
                    }
                    Ok(None) => {}
                }
            }
            if !handled {
                // The gateway left the TUI (native gateway deleted; chat returns as a plugin):
                // point muscle memory at it instead of the generic unknown.
                let first = cmd[1..].split_whitespace().next().unwrap_or("");
                if first == "gateway" || first == "gw" {
                    say(
                        tui.as_ref().map(|(s, _)| s),
                        "the TUI gateway is gone — native chat support was removed",
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
