type Cancel = tokio_util::sync::CancellationToken;
type Stop = std::sync::Arc<std::sync::atomic::AtomicBool>;
type TuiOpt = Option<crate::composer::SharedTui>;

pub(crate) struct TurnLocalCommands {
    pub tui: crate::composer::SharedTui,
    pub cwd: std::path::PathBuf,
    pub session_id: Option<String>,
    pub executor: Option<std::sync::Arc<dyn gray_core::agent::ToolExecutor>>,
    pub hooks: Vec<std::sync::Arc<dyn gray_core::agent::PluginHooks>>,
}

fn plugin_turn_local_args(
    text: &str,
    hooks: &[std::sync::Arc<dyn gray_core::agent::PluginHooks>],
) -> Option<Vec<String>> {
    let (name, args) = super::split_plugin_command(text)?;
    let cmd = hooks
        .iter()
        .flat_map(|h| h.commands())
        .find(|c| c.name == name)?;
    let verb = args.first().map(String::as_str).unwrap_or("");
    cmd.turn_local_verbs
        .iter()
        .any(|v| v == "*" || v == verb)
        .then_some(args)
}

pub(crate) fn is_turn_local_command(
    text: &str,
    hooks: &[std::sync::Arc<dyn gray_core::agent::PluginHooks>],
) -> bool {
    match super::parse_command(text) {
        super::ReplCommand::Jobs | super::ReplCommand::Help => true,

        super::ReplCommand::Plugins | super::ReplCommand::Reload | super::ReplCommand::Hehe => true,
        super::ReplCommand::Unknown(_) => plugin_turn_local_args(text, hooks).is_some(),
        _ => false,
    }
}

fn help_text(
    cwd: &std::path::Path,
    hooks: &[std::sync::Arc<dyn gray_core::agent::PluginHooks>],
) -> String {
    let mut out = String::new();
    for d in super::REGISTRY.iter().chain(super::commands::TUI_COMMANDS) {
        out.push_str(&super::commands::format_help_line(d));
        out.push('\n');
    }
    let mut entries = crate::plugin_cli::completions("");
    entries.extend(super::plugin_help_entries(hooks));
    let mut seen: std::collections::HashSet<String> = super::REGISTRY
        .iter()
        .chain(super::commands::TUI_COMMANDS)
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
    out.trim_end().to_string()
}

fn run_turn_local_command(ctx: &TurnLocalCommands, text: &str) {
    let tui = Some(&ctx.tui);
    match super::parse_command(text) {
        super::ReplCommand::Jobs => {
            let job_ctx = super::jobs::session_ctx(&ctx.cwd, ctx.session_id.as_deref());
            let jobs = ctx
                .executor
                .as_ref()
                .map(|exec| exec.background_jobs(&job_ctx))
                .unwrap_or_default();
            let text = crate::setup::gray_home()
                .map(|home| {
                    let wakes = super::jobs::load_wakes(&home, ctx.session_id.as_deref());
                    super::jobs::dashboard(&jobs, &wakes, crate::cron::now_secs())
                })
                .unwrap_or_else(|e| format!("jobs dashboard failed: {e}"));
            super::say(tui, &text);
        }
        super::ReplCommand::Help => {
            let mut t = ctx.tui.lock().expect("tui lock");
            t.push_dim(help_text(&ctx.cwd, &ctx.hooks));
            t.ensure_gap();
        }
        super::ReplCommand::Plugins => {
            let tui = Some(&ctx.tui);
            let bg = ctx.tui.lock().expect("tui lock").snapshot();
            match super::with_modal_sync(tui, || crate::setup::run_plugins_manager(Some(&bg))) {
                Ok(true) => super::say(tui, "plugins updated"),

                Ok(false) => {
                    let _ = ctx.tui.lock().expect("tui lock").draw();
                }
                Err(e) => super::say(tui, &format!("plugins manager failed: {e}")),
            }
        }
        super::ReplCommand::Reload => {
            super::customize::handle_reload(Some(&ctx.tui));
        }
        super::ReplCommand::Hehe => {
            let reverting = crate::mascot::mascot_shown();
            let shown = {
                let mut t = ctx.tui.lock().expect("tui lock");
                if reverting {
                    t.pop_mascot()
                } else {
                    t.push_mascot()
                }
            };
            if !shown && !reverting {
                super::say(
                    Some(&ctx.tui),
                    "graychan needs a truecolor terminal and at least 20x10 cells",
                );
            }
        }
        super::ReplCommand::Unknown(cmd) => {
            let Some((name, argv)) = super::split_plugin_command(&cmd) else {
                return;
            };
            use gray_core::agent::CommandOutcome;
            let outcome = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(super::run_plugin_command(&ctx.hooks, &name, argv))
            });
            match outcome {
                Some(CommandOutcome::Say(text)) => super::say(tui, &text),
                Some(CommandOutcome::Prompt(prompt)) => {
                    let mut t = ctx.tui.lock().expect("tui lock");
                    t.queued_inputs.push_back((prompt, Vec::new()));
                    let _ = t.draw();
                }
                Some(CommandOutcome::ModelPicker(_))
                | Some(CommandOutcome::Panel(_))
                | Some(CommandOutcome::Picker(_))
                | Some(CommandOutcome::Terminal { .. }) => {
                    // Pickers and settings mutate session state and own the
                    // modal surface — rerun through the full dispatch once
                    // the turn ends instead of racing the running turn.
                    let mut t = ctx.tui.lock().expect("tui lock");
                    t.queued_inputs.push_back((cmd.clone(), Vec::new()));
                    let _ = t.draw();
                }
                None => {}
            }
        }
        _ => {}
    }
}

fn try_lock_tui<T>(
    shared: &std::sync::Arc<std::sync::Mutex<T>>,
) -> Option<std::sync::MutexGuard<'_, T>> {
    match shared.try_lock() {
        Ok(g) => Some(g),
        Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

fn lock_tui<T>(shared: &std::sync::Arc<std::sync::Mutex<T>>) -> std::sync::MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn spawn_key_watcher_with_typing(
    watch_cancel: Cancel,
    watcher_stopped: Stop,
    watcher_tui: TuiOpt,
    cwd_for_watcher: std::path::PathBuf,
    turn_locals: Option<TurnLocalCommands>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};

        let _keyboard_enhancement =
            crate::composer::input::LockedEnhancement::push(watcher_tui.clone());
        crate::composer::input::watch_dead_stdin();
        loop {
            if watcher_stopped.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }

            if crate::composer::input::stdin_gone_confirmed() {
                return;
            }
            // A live ask modal (tool consent, request_user_input) reads
            // stdin itself. Polling here too races it for every key and
            // the watcher's `continue` below drops whatever it wins, so
            // Esc/arrows/Enter in the modal were mostly swallowed.
            if crate::ask::is_ask_live() {
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            match poll(std::time::Duration::from_millis(50)) {
                Ok(true) => {}
                _ => continue,
            }
            let Ok(event) = read() else {
                continue;
            };

            let passthrough = matches!(event, Event::Resize(..))
                || matches!(
                    event,
                    Event::Key(KeyEvent {
                        code: KeyCode::Char('c') | KeyCode::Char('C'),
                        modifiers,
                        ..
                    }) if modifiers.contains(KeyModifiers::CONTROL)
                );
            if crate::ask::is_ask_live() && !passthrough {
                continue;
            }
            match event {
                Event::Resize(cols, rows) => {
                    if let Some(shared) = watcher_tui.as_ref()
                        && let Some(mut t) = try_lock_tui(shared)
                        && (cols != t.last_width || rows != t.last_height)
                    {
                        t.pending_resize = Some((
                            cols,
                            std::time::Instant::now() + std::time::Duration::from_millis(75),
                        ));
                    }
                }
                Event::Key(ev) => {
                    if ev.kind == KeyEventKind::Release {
                        continue;
                    }
                    // A decoded Esc with bytes still queued is the head of a
                    // split or unknown escape sequence, not a keypress — drop
                    // it and drain the tail so it can't cancel the turn or
                    // type into the composer. A real Esc arrives alone.
                    if matches!(ev.code, KeyCode::Esc)
                        && crate::composer::input::esc_is_sequence_head()
                    {
                        continue;
                    }

                    if matches!(ev.code, KeyCode::Char('c') | KeyCode::Char('C'))
                        && ev.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        log::info!(target: "gray_repl", "turn cancelled by key {ev:?}");
                        watch_cancel.cancel();
                        return;
                    }
                    let bindings = crate::keymap::resolve_event(&ev);
                    let interrupts = bindings.contains(&crate::keymap::Binding::Action(
                        crate::keymap::Action::Interrupt,
                    ));
                    if interrupts {
                        // Unexplained mid-turn interrupts were reported; name
                        // the exact event so a stray terminal sequence shows up.
                        log::info!(target: "gray_repl", "turn interrupt key {ev:?}");
                    }
                    let Some(shared) = watcher_tui.as_ref() else {
                        if interrupts {
                            watch_cancel.cancel();
                            return;
                        }
                        continue;
                    };

                    let mut t = if interrupts {
                        match try_lock_tui(shared) {
                            Some(t) => t,
                            None => {
                                watch_cancel.cancel();
                                return;
                            }
                        }
                    } else {
                        lock_tui(shared)
                    };
                    if !interrupts && !t.is_task_running {
                        continue;
                    }
                    use crate::composer::input::keys::{KeyOutcome, dispatch_bindings};
                    match dispatch_bindings(&mut t, &ev, &bindings) {
                        KeyOutcome::Edited => {
                            sync_matches(&mut t, &cwd_for_watcher);
                            let _ = t.draw();
                        }
                        KeyOutcome::Ignored | KeyOutcome::Exit => {}
                        KeyOutcome::Clear => {
                            watch_cancel.cancel();
                            return;
                        }
                        KeyOutcome::Interrupt => {
                            let mut text = t.textarea.text().to_string();
                            for (ph, full) in &t.pending_pastes {
                                text = text.replace(ph, full);
                            }
                            let text = text.trim().to_string();
                            if text.starts_with('/') && !text.contains('\n') {
                                t.local_command = Some(text);
                                crate::composer::input::clear_draft(&mut t);
                                let _ = t.draw();
                            } else if !text.is_empty() {
                                // "Cancel the turn, or clear the draft": a
                                // draft is cleared first — only an empty
                                // composer cancels the turn.
                                crate::composer::input::clear_draft(&mut t);
                                let _ = t.draw();
                                continue;
                            }
                            watch_cancel.cancel();
                            return;
                        }
                        KeyOutcome::Command(cmd) => {
                            if t.attachments.is_empty()
                                && let Some(locals) = turn_locals.as_ref()
                                && is_turn_local_command(&cmd, &locals.hooks)
                            {
                                t.flush_markdown();
                                t.end_thinking();
                                t.textarea.set_text("");
                                t.pending_pastes.clear();
                                t.history_idx = None;
                                t.matches.clear();
                                t.sel = 0;
                                let _ = t.draw();
                                drop(t);
                                run_turn_local_command(locals, &cmd);
                                continue;
                            }
                            t.queued_inputs.push_back((cmd, Vec::new()));
                            t.matches.clear();
                            t.sel = 0;
                            let _ = t.draw();
                        }
                        KeyOutcome::Submit => {
                            let mut text = t.textarea.text().to_string();
                            for (ph, full) in &t.pending_pastes {
                                text = text.replace(ph, full);
                            }
                            let text = text.trim().to_string();
                            if text.is_empty() && t.attachments.is_empty() {
                                continue;
                            }
                            let local = t.attachments.is_empty()
                                && turn_locals
                                    .as_ref()
                                    .is_some_and(|l| is_turn_local_command(&text, &l.hooks));
                            if local {
                                t.flush_markdown();
                                t.end_thinking();
                                t.textarea.set_text("");
                                t.pending_pastes.clear();
                                t.history_idx = None;
                                t.matches.clear();
                                t.sel = 0;
                                let _ = t.draw();
                                drop(t);
                                run_turn_local_command(
                                    turn_locals.as_ref().expect("checked above"),
                                    &text,
                                );
                                continue;
                            }
                            crate::composer::input::sync_attachment_markers(&mut t);
                            let attached: Vec<std::path::PathBuf> =
                                std::mem::take(&mut t.attachments);

                            crate::prompt_history::record(&mut t.history, &text);
                            t.queued_inputs.push_back((text, attached));
                            t.textarea.set_text("");
                            t.pending_pastes.clear();
                            t.history_idx = None;
                            t.matches.clear();
                            t.sel = 0;
                            let _ = t.draw();
                        }
                    }
                }
                Event::Paste(data) => {
                    let Some(shared) = watcher_tui.as_ref() else {
                        continue;
                    };
                    let mut t = lock_tui(shared);
                    if !t.is_task_running {
                        continue;
                    }
                    t.search = None;
                    t.handle_paste(data);
                    sync_matches(&mut t, &cwd_for_watcher);
                    let _ = t.draw();
                }
                _ => {}
            }
        }
    })
}

fn sync_matches(t: &mut crate::composer::Tui, cwd: &std::path::Path) {
    crate::composer::input::sync_completion(t, cwd);
}

#[path = "key_watcher_tests.rs"]
#[cfg(test)]
mod tests;
