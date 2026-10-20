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
                say(None, &crate::rule("commands"));
                say(None, out.trim_end());
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
                t.start_fresh_conversation();
                let detail = if !short_id.is_empty() {
                    Some(format!("({short_id})"))
                } else {
                    None
                };
                t.push_action("New conversation started", detail.as_deref());
                t.ensure_gap();
            } else {
                if !short_id.is_empty() {
                    say(None, &format!("✓ New conversation started ({short_id})"));
                } else {
                    say(None, "✓ New conversation started");
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
                    say(
                        None,
                        &format!("{} {prompt_text}", crate::tui::arrow_glyph()),
                    );
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
            crate::quality_of_life::setup::handle(tui, arg).await;
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
                    | Some(CommandOutcome::Panel(_))
                    | Some(CommandOutcome::Picker(_))
                    | Some(CommandOutcome::Terminal { .. })
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
                        super::notice::push_notice(
                            &mut shared.lock().expect("tui lock"),
                            "Provider error",
                            &e.to_string(),
                            true,
                        );
                    } else {
                        say(None, &format!("provider error: {e}"));
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
            let mut outcome = run_plugin_command(&hooks, &name, argv.clone()).await;
            while let Some(oc) = outcome.take() {
                match oc {
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
                                            "{} inside {name} — prompts continue this agent's session",
                                            crate::tui::glyph("\u{2b22}", "#")
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

                    CommandOutcome::Terminal {
                        argv: cmd_argv,
                        text,
                    } => {
                        let shared = tui.as_ref().map(|(s, _)| s);
                        use std::io::IsTerminal as _;
                        match cmd_argv.split_first() {
                            Some((prog, rest))
                                if std::io::stdin().is_terminal()
                                    && std::io::stdout().is_terminal() =>
                            {
                                let before = installed_plugin_names();
                                let status = with_modal_sync(shared, || {
                                    super::maintenance::with_terminal(shared, || {
                                        let mut child = std::process::Command::new(prog);
                                        child.args(rest);
                                        if let Ok(exe) = crate::exe_watch::on_disk_exe() {
                                            child.env("GRAY_EXE", exe);
                                        }
                                        child.status()
                                    })
                                });
                                match status {
                                    Ok(s) if s.success() => {}
                                    Ok(s) => say(shared, &format!("{name} exited: {s}")),
                                    Err(e) => say(shared, &format!("{name}: {e}")),
                                }
                                if installed_plugin_names() != before {
                                    let sid = session_state.as_ref().map(|s| s.session_id.as_str());
                                    if reload_agent(agent, config, cwd, sid, shared).await {
                                        say(shared, "plugins changed, reloaded");
                                    }
                                }
                            }
                            _ => say(
                                shared,
                                &text.unwrap_or_else(|| {
                                    format!("{name} needs an interactive terminal")
                                }),
                            ),
                        }
                    }

                    CommandOutcome::Panel(spec) => {
                        let tui_shared = tui.as_ref().map(|(s, _)| s);
                        if let Some(shared) = tui_shared {
                            let pages = super::pager::pages_from_spec(&spec);
                            if pages.is_empty() {
                                say_lines(tui_shared, super::pager::spec_lines(&spec));
                            } else {
                                let bg = shared.lock().expect("tui lock").snapshot();
                                let mut refresh: Option<
                                    Box<dyn FnMut() -> Vec<super::pager::PagerPage>>,
                                > = spec.refreshable.then(|| {
                                    let hooks = hooks.clone();
                                    let name = name.clone();
                                    let argv = argv.clone();
                                    Box::new(move || {
                                        let out = tokio::task::block_in_place(|| {
                                            tokio::runtime::Handle::current().block_on(
                                                run_plugin_command(&hooks, &name, argv.clone()),
                                            )
                                        });
                                        super::pager::pages_from_outcome(out)
                                    })
                                        as Box<dyn FnMut() -> Vec<super::pager::PagerPage>>
                                });
                                let r = {
                                    let refresh = refresh.as_deref_mut();
                                    with_modal_sync(tui_shared, move || {
                                        super::pager::run_pager(pages, refresh, &bg)
                                    })
                                };
                                if r.is_err() {
                                    say_lines(tui_shared, super::pager::spec_lines(&spec));
                                }
                            }
                        } else {
                            say_lines(None, super::pager::spec_lines(&spec));
                        }
                    }

                    CommandOutcome::Picker(spec) => {
                        let tui_shared = tui.as_ref().map(|(s, _)| s);
                        if let Some(shared) = tui_shared {
                            if spec.items.is_empty() {
                                say_lines(tui_shared, super::picker::spec_lines(&spec));
                            } else {
                                let bg = shared.lock().expect("tui lock").snapshot();
                                let sel = spec
                                    .selected_id
                                    .as_deref()
                                    .and_then(|id| spec.items.iter().position(|i| i.id == id))
                                    .unwrap_or(0);
                                let trailing = |i: &gray_core::agent::PickerItem| {
                                    i.description
                                        .as_deref()
                                        .filter(|d| !d.is_empty())
                                        .map(|d| {
                                            vec![ratatui::text::Span::styled(
                                                d.to_string(),
                                                ratatui::style::Style::default()
                                                    .fg(crate::theme::theme().text_dim),
                                            )]
                                        })
                                        .unwrap_or_default()
                                };
                                let mut picker = super::picker::ListPicker {
                                    title: spec.title.as_str(),
                                    items: &spec.items,
                                    label: Box::new(|i: &gray_core::agent::PickerItem| {
                                        i.label.as_str()
                                    }),
                                    trailing: Some(Box::new(trailing)),
                                    footer: None,
                                    on_highlight: None,
                                    selected: sel,
                                };
                                match with_modal_sync(tui_shared, || {
                                    super::picker::run_list_picker(&mut picker, &bg)
                                }) {
                                    Ok(Some(idx)) => {
                                        outcome = run_plugin_command(
                                            &hooks,
                                            &name,
                                            super::picker::pick_argv(&argv, &spec.items[idx].id),
                                        )
                                        .await;
                                    }
                                    Ok(None) => {}
                                    Err(_) => {
                                        say_lines(tui_shared, super::picker::spec_lines(&spec));
                                    }
                                }
                            }
                        } else {
                            say_lines(None, super::picker::spec_lines(&spec));
                        }
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

/// Installed plugin names from the user lock file, so a terminal handoff that
/// installs or removes plugins (the account plugin's `/marketplace`) can
/// activate them without a restart.
fn installed_plugin_names() -> Option<std::collections::BTreeSet<String>> {
    let home = gray_core::paths::gray_home()?;
    let lock = gray_plugin::lock::LockFile::load(&gray_plugin::lock::lock_path(&home)).ok()?;
    Some(lock.plugins.into_keys().collect())
}
