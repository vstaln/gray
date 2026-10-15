use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Flow {
    Continue,
    Break,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_command(
    cmd: ReplCommand,
    agent: &mut Option<Agent>,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &TuiOpt,
    session_state: &mut Option<SessionState>,
    session_totals: &mut SessionTotals,
    pending_command: &mut Option<ReplCommand>,
    pending_history: &mut std::sync::Arc<Vec<Message>>,
    unconfigured: &mut bool,
    hide_thinking: &mut bool,
) -> anyhow::Result<Flow> {
    Ok(match cmd {
        ReplCommand::Empty | ReplCommand::Prompt(_) => Flow::Continue,
        ReplCommand::Quit => {
            let sid = session_state.as_ref().map(|s| s.session_id.as_str());
            let ctx = super::jobs::session_ctx(cwd, sid);
            let exec = agent.as_ref().map(|a| a.executor_handle());
            if let Some(warning) = super::jobs::confirm_quit(exec.as_deref(), &ctx) {
                say(tui.as_ref().map(|(s, _)| s), &warning);
                return Ok(Flow::Continue);
            }
            if let Some((shared, stop)) = tui {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                shared.lock().expect("tui lock").shutdown();
            }
            print_exit_hint(session_state);
            crate::ask::shutdown();
            if let Some(exec) = &exec {
                super::jobs::stop_all(exec.as_ref(), &ctx).await;
            }
            shutdown_hooks(agent.as_ref()).await;
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
                direct.into(),
                &mut *agent,
                tui.as_ref().map(|(s, _)| s),
                session_state.as_ref().map(|s| s.session_id.as_str()),
                &mut *hide_thinking,
            )
            .await;
            Flow::Continue
        }
        ReplCommand::ModelPanes => {
            if let Some(id) = super::handlers::handle_model_panes(
                config,
                cwd,
                &mut *agent,
                tui.as_ref().map(|(s, _)| s),
                session_state.as_ref().map(|s| s.session_id.as_str()),
                &mut *hide_thinking,
            )
            .await
            {
                *pending_command = Some(ReplCommand::ProviderLogin(id));
            }
            Flow::Continue
        }
        ReplCommand::ModelFocus(row) => {
            handle_model(
                config,
                cwd,
                super::handlers::ModelArg::Picker { focus: Some(row) },
                &mut *agent,
                tui.as_ref().map(|(s, _)| s),
                session_state.as_ref().map(|s| s.session_id.as_str()),
                &mut *hide_thinking,
            )
            .await;
            Flow::Continue
        }
        ReplCommand::Help => {
            let mut out = String::new();
            for d in REGISTRY.iter().chain(commands::TUI_COMMANDS) {
                out.push_str(&commands::format_help_line(d));
                out.push('\n');
            }
            let mut entries = crate::plugin_cli::completions("");
            if let Some(a) = agent.as_ref() {
                entries.extend(plugin_help_entries(a.hooks()));
            }
            let mut seen: std::collections::HashSet<String> = REGISTRY
                .iter()
                .chain(commands::TUI_COMMANDS)
                .map(|d| d.name.to_string())
                .collect();
            for (name, description) in entries {
                if seen.insert(name.clone()) {
                    out.push_str(&format!("  /{name:<10} {description}\n"));
                }
            }
            let templates = crate::prompt_templates::discover(cwd);
            if !templates.is_empty() {
                out.push_str("prompt templates:\n");
                for t in templates {
                    if seen.insert(t.name.clone()) {
                        out.push_str(&format!("  /{:<10} {}\n", t.name, t.description));
                    }
                }
            }
            if let Some((shared, _)) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.push_dim(out.trim_end().to_string());
                t.ensure_gap();
            } else {
                println!("{}", crate::rule("commands"));
                print!("{out}");
            }
            Flow::Continue
        }
        ReplCommand::Hehe => {
            let tui_shared = tui.as_ref().map(|(s, _)| s);
            let reverting = crate::mascot::mascot_shown();
            let shown = match tui_shared {
                Some(shared) => {
                    let mut t = shared.lock().expect("tui lock");
                    if reverting {
                        t.pop_mascot()
                    } else {
                        t.push_mascot()
                    }
                }
                None => crate::mascot::print_banner(),
            };
            if !shown && !reverting {
                say(
                    tui_shared,
                    "graychan needs a truecolor terminal and at least 20x10 cells",
                );
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
            *pending_history = std::sync::Arc::new(Vec::new());
            *session_totals = SessionTotals::default();
            *session_state = None;
            let mut short_id = String::new();
            let mut new_sid: Option<SessionId> = None;
            if let Some(root) = default_root() {
                let store = JsonlSessionStore::new(root);
                let session_id = store.fresh_id().await;
                let timestamp = crate::print::now_millis();
                let meta = SessionMeta::new(
                    session_id.clone(),
                    timestamp,
                    cwd.to_path_buf(),
                    config.model.clone().unwrap_or_else(|| "unset".into()),
                )
                .with_origin(crate::session_store::session_origin_from_env());
                if let Err(e) = store.create(meta).await {
                    log::warn!(target: "gray_session", "session create failed: {e}");
                }

                let open_guard = match store.acquire_open(&session_id).await {
                    Ok(guard) => Some(guard),
                    Err(e) => {
                        log::warn!(target: "gray_session", "session open-lock failed: {e}");
                        None
                    }
                };
                short_id = crate::resume::short_id(&session_id);
                new_sid = Some(session_id.clone());
                super::session::export_session_env(session_id.as_str());
                *session_state = Some(SessionState {
                    full_save_pending: false,
                    store,
                    session_id,
                    _open_guard: open_guard,
                });
            }

            *agent = build_agent(config, cwd, new_sid.as_ref().map(|s| s.as_str()))
                .await
                .ok();

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
                t.ensure_gap();
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
        ReplCommand::Undo | ReplCommand::Retry => {
            if let Some(text) = handle_undo(
                matches!(cmd, ReplCommand::Retry),
                &mut *agent,
                session_state,
                pending_history,
                tui.as_ref().map(|(s, _)| s),
            )
            .await
            {
                *pending_command = Some(ReplCommand::Prompt(text));
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
                pending_history,
                tui.as_ref().map(|(s, _)| s),
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
        ReplCommand::Jobs => {
            let tui_shared = tui.as_ref().map(|(s, _)| s);
            let sid = session_state
                .as_ref()
                .map(|s| s.session_id.as_str().to_string());
            let ctx = super::jobs::session_ctx(cwd, sid.as_deref());
            let home = crate::setup::gray_home()?;
            match (agent.as_ref().map(|a| a.executor_handle()), tui_shared) {
                (Some(exec), Some(shared)) => {
                    let bg = shared.lock().expect("tui lock").snapshot();
                    let changed = with_modal_sync(tui_shared, || {
                        super::jobs::run_jobs_modal(Some(&bg), &home, exec, ctx)
                    });
                    match changed {
                        Ok(true) => say(tui_shared, "background work updated"),
                        Ok(false) => {}
                        Err(e) => say(tui_shared, &format!("jobs picker failed: {e}")),
                    }
                }
                (exec, _) => {
                    let jobs = exec.map(|e| e.background_jobs(&ctx)).unwrap_or_default();
                    let wakes = super::jobs::load_wakes(&home, sid.as_deref());
                    say(
                        tui_shared,
                        &super::jobs::dashboard(&jobs, &wakes, crate::cron::now_secs()),
                    );
                }
            }
            Flow::Continue
        }

        ReplCommand::CronJobs(arg) => {
            let tui_shared = tui.as_ref().map(|(s, _)| s);
            if let Some(on) = arg.as_deref().and_then(handlers::parse_on_off) {
                let text = handlers::toggle_subsystem(handlers::Subsystem::Cron, on);
                say(tui_shared, &text);
                Flow::Continue
            } else if arg.is_none() && tui_shared.is_some() {
                let home = crate::setup::gray_home()?;
                let bg = tui_shared.map(|s| s.lock().expect("tui lock").snapshot());
                let changed = with_modal_sync(tui_shared, || {
                    super::cron::run_cron_modal(bg.as_ref(), &home)
                });
                match changed {
                    Ok(true) => say(tui_shared, "cron jobs updated"),
                    Ok(false) => {}
                    Err(e) => say(tui_shared, &format!("cron picker failed: {e}")),
                }
                Flow::Continue
            } else {
                let store = crate::cron::CronStore::open(crate::setup::gray_home()?.join("cron"))?;
                let jobs = store.list()?;
                let now = crate::cron::now_secs();

                let health = store.health(now).ok();
                let text = match arg {
                    None => super::cron::format_cron_dashboard(&jobs, health.as_ref(), now),
                    Some(id) => match jobs.into_iter().find(|j| j.id == id || j.name == id) {
                        Some(j) => super::cron::format_cron_dashboard(&[j], health.as_ref(), now),
                        None => format!("unknown cron job {id:?}"),
                    },
                };
                say(tui_shared, &text);
                Flow::Continue
            }
        }

        ReplCommand::Update(arg) => {
            super::maintenance::handle_update(tui, arg).await;
            Flow::Continue
        }

        ReplCommand::Restart => {
            super::maintenance::handle_restart(tui).await;
            Flow::Continue
        }

        ReplCommand::Gateway(arg) => {
            let tui_shared = tui.as_ref().map(|(s, _)| s);
            if let Some(on) = arg.as_deref().and_then(handlers::parse_on_off) {
                let text = handlers::toggle_subsystem(handlers::Subsystem::Gateway, on);
                say(tui_shared, &text);
            } else if tui_shared.is_none() {
                say(tui_shared, &super::gateway_panel::format_text());
            } else {
                let bg = tui_shared.map(|s| s.lock().expect("tui lock").snapshot());
                let changed = with_modal_sync(tui_shared, || {
                    super::gateway_panel::run_gateway_modal(bg.as_ref())
                });
                match changed {
                    Ok(true) => say(tui_shared, "connections updated"),

                    Ok(false) => say(tui_shared, &super::gateway_panel::format_text()),
                    Err(e) => say(tui_shared, &format!("connections picker failed: {e}")),
                }
            }
            Flow::Continue
        }

        ReplCommand::Copy => {
            handle_copy(agent, tui.as_ref().map(|(s, _)| s));
            Flow::Continue
        }
        ReplCommand::Theme(arg) => {
            super::customize::handle_theme(arg, tui.as_ref().map(|(s, _)| s));
            Flow::Continue
        }
        ReplCommand::Setup(arg) => {
            crate::qol::setup::handle(tui, arg).await;
            Flow::Continue
        }
        ReplCommand::Intro(arg) => {
            let tui_shared = tui.as_ref().map(|(s, _)| s);
            let text = match arg.as_deref().and_then(handlers::parse_on_off) {
                Some(on) => handlers::toggle_subsystem(handlers::Subsystem::Intro, on),
                None if arg.is_none() => format!(
                    "logo intro is {}",
                    if crate::setup::intro_enabled() {
                        "on"
                    } else {
                        "off"
                    }
                ),
                None => "usage: /intro [on|off]".to_string(),
            };
            say(tui_shared, &text);
            Flow::Continue
        }
        ReplCommand::Keys(arg) => {
            super::customize::handle_keys(arg, tui.as_ref().map(|(s, _)| s));
            Flow::Continue
        }
        ReplCommand::Reload => {
            super::customize::handle_reload(tui.as_ref().map(|(s, _)| s));
            Flow::Continue
        }
        ReplCommand::Feedback(text) => {
            handle_feedback(text, config, session_state, tui.as_ref().map(|(s, _)| s));
            Flow::Continue
        }
        ReplCommand::ProviderCommand {
            cmd,
            argv,
            provider_id,
        } => {
            *pending_command = Some(
                match super::plugin_cmds::provider_command_outcome(&provider_id, &cmd, argv).await {
                    Some(CommandOutcome::ModelPicker(row)) => ReplCommand::ModelFocus(row),
                    Some(CommandOutcome::Prompt(prompt)) => ReplCommand::Prompt(prompt),
                    Some(CommandOutcome::AgentPicker(_))
                    | Some(CommandOutcome::SubagentSettings(_))
                    | None => ReplCommand::ProviderLogin(provider_id),
                    Some(CommandOutcome::Say(text)) => {
                        say(tui.as_ref().map(|(s, _)| s), &text);
                        return Ok(Flow::Continue);
                    }
                },
            );
            Flow::Continue
        }
        ReplCommand::ProviderLogin(id) => {
            let preselect = Some(id);
            let bg = tui
                .as_ref()
                .map(|(shared, _)| shared.lock().expect("tui lock").snapshot());
            let result = with_modal_sync(tui.as_ref().map(|(s, _)| s), || {
                crate::setup::run_connect_modal_for(config, bg.as_ref(), preselect.as_deref())
            });
            match result {
                Ok(crate::setup::ConnectOutcome::Connected) => {
                    *unconfigured = false;
                    push_provider_connected(config, tui, Some(&mut *hide_thinking));
                    super::handlers::switch_agent_model(
                        &mut *agent,
                        config,
                        cwd,
                        session_state.as_ref().map(|s| s.session_id.as_str()),
                        tui.as_ref().map(|(s, _)| s),
                    )
                    .await;
                }

                Ok(crate::setup::ConnectOutcome::Removed(name)) => {
                    reload_agent(
                        &mut *agent,
                        config,
                        cwd,
                        session_state.as_ref().map(|s| s.session_id.as_str()),
                        tui.as_ref().map(|(s, _)| s),
                    )
                    .await;
                    say(tui.as_ref().map(|(s, _)| s), &format!("removed {name}"));
                }
                Ok(crate::setup::ConnectOutcome::Dismissed) => {
                    if let Some((shared, _)) = tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.clear_draft();

                        t.ensure_gap();
                        let _ = t.draw();
                    }
                }
                Err(e) => {
                    if let Some((shared, _)) = tui {
                        super::cron::push_notice(
                            &mut shared.lock().expect("tui lock"),
                            "Provider error",
                            &e.to_string(),
                            true,
                        );
                    } else {
                        println!("provider error: {e}");
                    }
                }
            }
            Flow::Continue
        }
        ReplCommand::Skill(_) => Flow::Continue,
        ReplCommand::Plugin(raw) => {
            handle_plugin_command(&raw, tui.as_ref().map(|(s, _)| s)).await;
            Flow::Continue
        }
        ReplCommand::Plugins => {
            let tui_shared = tui.as_ref().map(|(s, _)| s);
            if tui_shared.is_none() {
                say(tui_shared, &crate::plugin_status::table().await);
            } else {
                let bg = tui_shared.map(|s| s.lock().expect("tui lock").snapshot());
                let changed = with_modal_sync(tui_shared, || {
                    crate::setup::run_plugins_manager(bg.as_ref())
                });
                match changed {
                    Ok(true) => say(tui_shared, "plugins updated"),
                    Ok(false) => {
                        if let Some(shared) = tui_shared {
                            let mut t = shared.lock().expect("tui lock");
                            t.clear_draft();
                            if !t.retract_slash_echo("/plugins") {
                                let _ = t.draw();
                            }
                        }
                    }
                    Err(e) => say(tui_shared, &format!("plugins manager failed: {e}")),
                }
            }
            Flow::Continue
        }
        ReplCommand::Unknown(cmd) => {
            if agent.is_none() && config.model.is_some() {
                super::session::ensure_session_state(session_state, config, cwd).await;
                let sid = session_state.as_ref().map(|s| s.session_id.as_str());
                match build_agent(config, cwd, sid).await {
                    Ok(built) => {
                        *agent =
                            Some(built.with_messages(super::take_pending_history(pending_history)));
                    }
                    Err(e) => {
                        say(tui.as_ref().map(|(s, _)| s), &format!("{e:#}"));
                        return Ok(Flow::Continue);
                    }
                }
            }
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

                    CommandOutcome::Prompt(prompt) => {
                        *pending_command = Some(ReplCommand::Prompt(prompt));
                    }

                    CommandOutcome::ModelPicker(row) => {
                        *pending_command = Some(ReplCommand::ModelFocus(row));
                    }

                    CommandOutcome::AgentPicker(plugin) => {
                        let bg = tui
                            .as_ref()
                            .map(|(s, _)| s.lock().expect("tui lock").snapshot());
                        let action = with_modal_sync(tui.as_ref().map(|(s, _)| s), || {
                            super::agents_panel::run_agents_picker(&plugin, bg.as_ref())
                        });
                        match action {
                            Ok(Some(super::agents_panel::PanelAction::Resume {
                                session: sid,
                                name,
                            })) => {
                                handle_resume(
                                    config,
                                    cwd,
                                    ResumeArgs {
                                        target: Some(sid.clone()),
                                        last: false,
                                        all: true,
                                        dismiss: false,
                                    },
                                    &mut *agent,
                                    &mut *session_state,
                                    &mut *session_totals,
                                    tui.as_ref().map(|(s, _)| s),
                                    &mut *hide_thinking,
                                )
                                .await;

                                let inside = session_state
                                    .as_ref()
                                    .is_some_and(|s| s.session_id.as_str() == sid);
                                if inside {
                                    say(
                                        tui.as_ref().map(|(s, _)| s),
                                        &format!(
                                            "⬢ inside {name} — prompts continue this agent's session"
                                        ),
                                    );
                                }
                            }
                            Ok(Some(super::agents_panel::PanelAction::Say(text))) => {
                                say(tui.as_ref().map(|(s, _)| s), &text);
                            }
                            Ok(None) => {
                                if let Some((shared, _)) = tui {
                                    shared.lock().expect("tui lock").ensure_gap();
                                }
                            }
                            Err(e) => {
                                say(
                                    tui.as_ref().map(|(s, _)| s),
                                    &format!("agents panel error: {e}"),
                                );
                            }
                        }
                    }

                    CommandOutcome::SubagentSettings(plugin) => {
                        let msg = match tui.as_ref().map(|(s, _)| s) {
                            Some(shared) => {
                                super::agents_panel::run_subagent_settings(&plugin, shared)
                                    .unwrap_or_else(|| "cancelled — nothing changed".to_string())
                            }
                            None => "subagents settings needs the TUI".to_string(),
                        };
                        say(tui.as_ref().map(|(s, _)| s), &msg);
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
                say(
                    tui.as_ref().map(|(s, _)| s),
                    &format!("unknown command '{cmd}' — type /help for available commands"),
                );
            }
            Flow::Continue
        }
    })
}
