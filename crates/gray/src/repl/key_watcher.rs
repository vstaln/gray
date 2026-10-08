//! Key watchers during agent turns: Ctrl-C cancels, Esc cancels, resize/typing handled (split from `repl`).

type Cancel = tokio_util::sync::CancellationToken;
type Stop = std::sync::Arc<std::sync::atomic::AtomicBool>;
type TuiOpt = Option<crate::composer::SharedTui>;

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
