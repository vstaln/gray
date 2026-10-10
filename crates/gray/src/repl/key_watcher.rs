//! Key watchers during agent turns: Ctrl-C cancels, Esc cancels, resize/typing handled (split from `repl`).

type Cancel = tokio_util::sync::CancellationToken;
type Stop = std::sync::Arc<std::sync::atomic::AtomicBool>;
type TuiOpt = Option<crate::composer::SharedTui>;

/// Local state a turn-time slash command may read. Nothing here borrows the
/// running `Agent`, so handling these commands cannot interrupt or join the
/// turn the way the normal queued-command path does.
pub(crate) struct TurnLocalCommands {
    pub tui: crate::composer::SharedTui,
    pub cwd: std::path::PathBuf,
    pub session_id: Option<String>,
    pub config: crate::config::Config,
    pub totals: super::SessionTotals,
    pub executor: Option<std::sync::Arc<dyn gray_core::agent::ToolExecutor>>,
    pub hooks: Vec<std::sync::Arc<dyn gray_core::agent::PluginHooks>>,
}

/// `/subagents` verbs that only read run state. Mutating verbs stay queued:
/// they can wait for the turn instead of acting through a hidden side door.
fn subagents_view_args(text: &str) -> Option<Vec<String>> {
    let (name, args) = super::split_plugin_command(text)?;
    if !name.eq_ignore_ascii_case("/subagents") {
        return None;
    }
    const MUTATING: &[&str] = &[
        "run", "steer", "chat", "stop", "settings", "setup", "widget",
    ];
    (!args
        .first()
        .is_some_and(|verb| MUTATING.contains(&verb.as_str())))
    .then_some(args)
}

/// Slash commands that are pure local readouts. Everything else keeps the
/// existing behavior: queue it as a follow-up or cancel on Esc.
fn is_turn_local_command(text: &str) -> bool {
    match super::parse_command(text) {
        super::ReplCommand::Usage | super::ReplCommand::Jobs | super::ReplCommand::Help => true,
        super::ReplCommand::CronJobs(arg) => arg
            .as_deref()
            .and_then(super::handlers::parse_on_off)
            .is_none(),
        super::ReplCommand::Unknown(_) => subagents_view_args(text).is_some(),
        _ => false,
    }
}

fn help_text(
    cwd: &std::path::Path,
    hooks: &[std::sync::Arc<dyn gray_core::agent::PluginHooks>],
) -> String {
    let mut out = String::new();
    for d in super::REGISTRY {
        out.push_str(&super::commands::format_help_line(d));
        out.push('\n');
    }
    let mut entries = crate::plugin_cli::completions("");
    entries.extend(super::plugin_help_entries(hooks));
    let mut seen: std::collections::HashSet<String> =
        super::REGISTRY.iter().map(|d| d.name.to_string()).collect();
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
        super::ReplCommand::Usage => {
            super::status::handle_turn_usage(&ctx.totals, &ctx.config, tui);
        }
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
        super::ReplCommand::CronJobs(arg) => {
            let text = crate::setup::gray_home()
                .and_then(|home| {
                    let store = crate::cron::CronStore::open(home.join("cron"))?;
                    let jobs = store.list()?;
                    let now = crate::cron::now_secs();
                    let health = store.health(now).ok();
                    Ok(match arg {
                        None => super::cron::format_cron_dashboard(&jobs, health.as_ref(), now),
                        Some(id) => match jobs.into_iter().find(|j| j.id == id || j.name == id) {
                            Some(j) => {
                                super::cron::format_cron_dashboard(&[j], health.as_ref(), now)
                            }
                            None => format!("unknown cron job {id:?}"),
                        },
                    })
                })
                .unwrap_or_else(|e: anyhow::Error| format!("cron dashboard failed: {e}"));
            super::say(tui, &text);
        }
        super::ReplCommand::Help => {
            let mut t = ctx.tui.lock().expect("tui lock");
            t.push_dim(help_text(&ctx.cwd, &ctx.hooks));
            t.ensure_gap();
        }
        super::ReplCommand::Unknown(_) => {
            let Some(args) = subagents_view_args(text) else {
                return;
            };
            if args.is_empty() {
                let bg = ctx.tui.lock().expect("tui lock").snapshot();
                let action = super::with_modal_sync(tui, || {
                    super::agents_panel::run_agents_viewer("subagents", Some(&bg))
                });
                match action {
                    Ok(Some(super::agents_panel::PanelAction::Say(text))) => {
                        super::say(tui, &text);
                    }
                    Ok(Some(super::agents_panel::PanelAction::Resume { .. })) => {
                        super::say(tui, "that agent can be resumed after the current turn");
                    }
                    Ok(None) => {
                        ctx.tui.lock().expect("tui lock").ensure_gap();
                    }
                    Err(e) => super::say(tui, &format!("agents panel error: {e}")),
                }
            } else {
                // Plugin subprocesses can be slower than a keypress. Keep the
                // watcher consuming Ctrl-C/typing; write the result when the
                // lookup returns.
                let shared = ctx.tui.clone();
                std::thread::spawn(move || {
                    let tui = Some(&shared);
                    match super::agents_panel::run_subagents_view(&args) {
                        Some(text) => super::say(tui, &text),
                        None => super::say(
                            tui,
                            "subagents: no CLI in the plugin lock — install it first",
                        ),
                    }
                });
            }
        }
        _ => {}
    }
}

/// Non-blocking TUI lock that survives a poisoned mutex: a panic elsewhere
/// must not permanently wedge the watcher (question overlay / typing dead).
/// `WouldBlock` still yields `None` (skip this tick). Ctrl-C/Esc paths never
/// touch the lock, so a bad mutex can never swallow a cancel.
fn try_lock_tui<T>(
    shared: &std::sync::Arc<std::sync::Mutex<T>>,
) -> Option<std::sync::MutexGuard<'_, T>> {
    match shared.try_lock() {
        Ok(g) => Some(g),
        Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

/// Once read() consumes input, renderer contention must not discard it.
/// This watcher runs on a blocking thread, so waiting here does not stall Tokio.
fn lock_tui<T>(shared: &std::sync::Arc<std::sync::Mutex<T>>) -> std::sync::MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// Full watcher (prompt turns): typing queues follow-ups, clipboard paste, popups.
pub(crate) fn spawn_key_watcher_with_typing(
    watch_cancel: Cancel,
    watcher_stopped: Stop,
    watcher_tui: TuiOpt,
    cwd_for_watcher: std::path::PathBuf,
    turn_locals: Option<TurnLocalCommands>,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
        // The turn-time watcher owns stdin exactly like the idle prompt
        // does, so it negotiates the same keyboard enhancement: without it
        // Shift+Enter typed *during* a turn arrives as a bare Enter and
        // submits instead of breaking the line. Popped with the task.
        let _keyboard_enhancement = crate::term_keys::KeyboardEnhancementGuard::push();
        loop {
            if watcher_stopped.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            match poll(std::time::Duration::from_millis(50)) {
                Ok(true) => {}
                _ => continue,
            }
            let Ok(event) = read() else {
                continue;
            };
            // An ask modal owns the keys while live (digits/Tab/Enter/Esc
            // belong to the question, not the turn): only resize + Ctrl-C
            // pass through — Ctrl-C still cancels the turn (the ask
            // resolves empty via the same token).
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
                    // Ctrl+C always cancels the turn, whatever the keymap.
                    if matches!(ev.code, KeyCode::Char('c') | KeyCode::Char('C'))
                        && ev.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        watch_cancel.cancel();
                        return;
                    }
                    let bindings = crate::keymap::resolve_event(&ev);
                    let interrupts = bindings.contains(&crate::keymap::Binding::Action(
                        crate::keymap::Action::Interrupt,
                    ));
                    let Some(shared) = watcher_tui.as_ref() else {
                        if interrupts {
                            watch_cancel.cancel();
                            return;
                        }
                        continue;
                    };
                    // Interrupt never waits on a busy renderer: a contended
                    // lock still cancels, it just skips the popup/slash step.
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
                            // A slash-command draft: cancel the turn and run
                            // it locally (echoed as a sent message, never fed
                            // to the AI); else plain cancel.
                            let mut text = t.textarea.text().to_string();
                            for (ph, full) in &t.pending_pastes {
                                text = text.replace(ph, full);
                            }
                            let text = text.trim().to_string();
                            if text.starts_with('/') && !text.contains('\n') {
                                t.push_user_prompt(&text, &[], false);
                                t.local_command = Some(text);
                                crate::composer::input::clear_draft(&mut t);
                                let _ = t.draw();
                            }
                            watch_cancel.cancel();
                            return;
                        }
                        KeyOutcome::Command(cmd) => {
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
                                && is_turn_local_command(&text)
                                && turn_locals.is_some();
                            if local {
                                // Echo the command card now, clear the draft,
                                // then drop the TUI lock before running it.
                                t.flush_markdown();
                                t.end_thinking();
                                t.push_user_prompt(&text, &[], false);
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
                            let attached: Vec<std::path::PathBuf> =
                                std::mem::take(&mut t.attachments)
                                    .into_iter()
                                    .map(|(_, p)| p)
                                    .collect();
                            // Queued — fleeting, not transcript (becomes a
                            // real prompt when dequeued).
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
                    t.handle_paste(data);
                    sync_matches(&mut t, &cwd_for_watcher);
                    let _ = t.draw();
                }
                _ => {}
            }
        }
    })
}

/// Refreshes the completion popup after the draft changed.
fn sync_matches(t: &mut crate::composer::Tui, cwd: &std::path::Path) {
    let cur_text = t.textarea.text().to_string();
    t.matches = crate::repl::completion_matches_dyn(&cur_text, cwd);
    if t.sel >= t.matches.len() {
        t.sel = t.matches.len().saturating_sub(1);
    }
}

#[path = "key_watcher_tests.rs"]
#[cfg(test)]
mod tests;
