//! Key watchers during agent turns: Ctrl-C cancels, Esc cancels, resize/typing handled (split from `repl`).

type Cancel = tokio_util::sync::CancellationToken;
type Stop = std::sync::Arc<std::sync::atomic::AtomicBool>;
type TuiOpt = Option<crate::composer::SharedTui>;

/// Minimal watcher (image turns): cancel/resize/question-overlay/Esc only.
pub(crate) fn spawn_key_watcher(
    watch_cancel: Cancel,
    watcher_stopped: Stop,
    watcher_tui: TuiOpt,
) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};
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
            if let Event::Resize(cols, _) = event {
                if let Some(shared) = watcher_tui.as_ref()
                    && let Ok(mut t) = shared.try_lock()
                {
                    t.pending_resize = Some((
                        cols,
                        std::time::Instant::now() + std::time::Duration::from_millis(75),
                    ));
                }
                continue;
            }
            if let Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = event
            {
                if kind == KeyEventKind::Release {
                    continue;
                }
                if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                    watch_cancel.cancel();
                    return;
                }
                // request_user_input overlay owns the keyboard while active
                if let Some(shared) = watcher_tui.as_ref()
                    && let Ok(mut t) = shared.try_lock()
                    && t.active_question.is_some()
                {
                    crate::composer::handle_question_key(&mut t, code, modifiers);
                    continue;
                }
                if code == KeyCode::Esc {
                    watch_cancel.cancel();
                    return;
                }
            }
        }
    })
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
            match event {
                Event::Resize(cols, _) => {
                    if let Some(shared) = watcher_tui.as_ref()
                        && let Ok(mut t) = shared.try_lock()
                    {
                        t.pending_resize = Some((
                            cols,
                            std::time::Instant::now() + std::time::Duration::from_millis(75),
                        ));
                    }
                }
                Event::Key(KeyEvent {
                    code,
                    modifiers,
                    kind,
                    ..
                }) => {
                    if kind == KeyEventKind::Release {
                        continue;
                    }
                    // Ctrl+C always cancels turn
                    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                        watch_cancel.cancel();
                        return;
                    }
                    // request_user_input overlay owns the keyboard while active
                    if let Some(shared) = watcher_tui.as_ref()
                        && let Ok(mut t) = shared.try_lock()
                        && t.active_question.is_some()
                    {
                        crate::composer::handle_question_key(&mut t, code, modifiers);
                        continue;
                    }
                    // Esc: dismiss popup if open; else if the draft is a
                    // slash command, cancel the turn and run it locally
                    // (echoed as a sent message, never fed to the AI);
                    // else plain cancel.
                    if code == KeyCode::Esc {
                        if let Some(shared) = watcher_tui.as_ref()
                            && let Ok(mut t) = shared.try_lock()
                        {
                            if !t.matches.is_empty() {
                                t.matches.clear();
                                t.sel = 0;
                                let _ = t.draw();
                                continue;
                            }
                            let mut text = t.textarea.text().to_string();
                            for (ph, full) in &t.pending_pastes {
                                text = text.replace(ph, full);
                            }
                            let text = text.trim().to_string();
                            if text.starts_with('/') && !text.contains('\n') {
                                let echo = crate::composer::transcript::redact_command_echo(&text);
                                t.push_user_prompt(&echo, &[], false);
                                t.local_command = Some(text);
                                t.textarea.set_text("");
                                t.attachments.clear();
                                t.pending_pastes.clear();
                                t.history_idx = None;
                                t.sel = 0;
                                t.matches.clear();
                                let _ = t.draw();
                                watch_cancel.cancel();
                                return;
                            }
                        }
                        watch_cancel.cancel();
                        return;
                    }
                    // When a turn is running, allow typing and queue on Enter
                    let Some(shared) = watcher_tui.as_ref() else {
                        continue;
                    };
                    let Ok(mut t) = shared.try_lock() else {
                        continue;
                    };
                    if !t.is_task_running {
                        continue;
                    }
                    if modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(code, KeyCode::Char('v') | KeyCode::Char('V'))
                    {
                        t.try_attach_clipboard_image();
                        // sync matches after clipboard attach may insert placeholder
                        let cur_text = t.textarea.text().to_string();
                        t.matches =
                            crate::repl::completion_matches_dyn(&cur_text, &cwd_for_watcher);
                        if t.sel >= t.matches.len() {
                            t.sel = t.matches.len().saturating_sub(1);
                        }
                        let _ = t.draw();
                        continue;
                    }
                    // Helper to sync matches after text change
                    let sync_matches = |t: &mut crate::composer::Tui| {
                        let cur_text = t.textarea.text().to_string();
                        t.matches =
                            crate::repl::completion_matches_dyn(&cur_text, &cwd_for_watcher);
                        if t.sel >= t.matches.len() {
                            t.sel = t.matches.len().saturating_sub(1);
                        }
                    };
                    // Ctrl editing keys (must check before generic Char handling)
                    if modifiers.contains(KeyModifiers::CONTROL) {
                        match code {
                            KeyCode::Char('p') => {
                                if !t.matches.is_empty() {
                                    t.sel = t.sel.saturating_sub(1);
                                    let _ = t.draw();
                                } else {
                                    // treat as Up history? ignore during task running
                                    let _ = t.draw();
                                }
                                continue;
                            }
                            KeyCode::Char('n') => {
                                if !t.matches.is_empty() {
                                    t.sel = (t.sel + 1).min(t.matches.len().saturating_sub(1));
                                    let _ = t.draw();
                                }
                                continue;
                            }
                            KeyCode::Char('u') => {
                                t.textarea.set_text("");
                                t.history_idx = None;
                                sync_matches(&mut t);
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Char('a') => {
                                t.textarea.set_cursor(0);
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Char('e') => {
                                t.textarea.move_to_end();
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Char('k') => {
                                let cur = t.textarea.cursor();
                                t.textarea.replace_range(cur..usize::MAX, "");
                                sync_matches(&mut t);
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Char('w') | KeyCode::Backspace => {
                                // when popup open, dismiss? mimic input.rs popup swallows word deletes
                                if !t.matches.is_empty() {
                                    t.textarea.delete_word_backward();
                                    t.sync_attachments();
                                    sync_matches(&mut t);
                                    let _ = t.draw();
                                    continue;
                                }
                                t.textarea.delete_word_backward();
                                t.sync_attachments();
                                sync_matches(&mut t);
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Delete => {
                                t.textarea.delete_word_forward();
                                t.sync_attachments();
                                sync_matches(&mut t);
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Left => {
                                if !t.matches.is_empty() {
                                    t.sel = t.sel.saturating_sub(1);
                                    let _ = t.draw();
                                } else {
                                    t.textarea.move_word_left();
                                    let _ = t.draw();
                                }
                                continue;
                            }
                            KeyCode::Right => {
                                if !t.matches.is_empty() {
                                    t.sel = (t.sel + 1).min(t.matches.len().saturating_sub(1));
                                    let _ = t.draw();
                                } else {
                                    t.textarea.move_word_right();
                                    let _ = t.draw();
                                }
                                continue;
                            }
                            _ => {}
                        }
                    }
                    // Alt editing keys
                    if modifiers.contains(KeyModifiers::ALT) {
                        match code {
                            KeyCode::Backspace => {
                                t.textarea.delete_word_backward();
                                t.sync_attachments();
                                sync_matches(&mut t);
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Delete => {
                                t.textarea.delete_word_forward();
                                t.sync_attachments();
                                sync_matches(&mut t);
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Char('d') => {
                                t.textarea.delete_word_forward();
                                sync_matches(&mut t);
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Char('b') | KeyCode::Left => {
                                t.textarea.move_word_left();
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Char('f') | KeyCode::Right => {
                                t.textarea.move_word_right();
                                let _ = t.draw();
                                continue;
                            }
                            _ => {}
                        }
                    }
                    // Popup navigation when >1 match (a lone match leaves
                    // Up/Down free for history recall / cursor movement)
                    if t.matches.len() > 1 {
                        match code {
                            KeyCode::Up => {
                                t.sel = t.sel.saturating_sub(1);
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Down => {
                                t.sel = (t.sel + 1).min(t.matches.len().saturating_sub(1));
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Tab => {
                                if let Some((name, _)) = t.matches.get(t.sel).cloned() {
                                    t.textarea.set_text(&format!("/{name} "));
                                    t.textarea.move_to_end();
                                    sync_matches(&mut t);
                                }
                                let _ = t.draw();
                                continue;
                            }
                            KeyCode::Enter => {
                                let cur_text = t.textarea.text().to_string();
                                if let Some((name, _)) = t.matches.get(t.sel).cloned()
                                    && cur_text != format!("/{name}")
                                    && cur_text != format!("/{name} ")
                                {
                                    t.textarea.set_text(&format!("/{name} "));
                                    t.textarea.move_to_end();
                                    sync_matches(&mut t);
                                    let _ = t.draw();
                                    continue;
                                }
                                // if already completed, fall through to queue logic below
                            }
                            KeyCode::Esc => {
                                t.matches.clear();
                                t.sel = 0;
                                let _ = t.draw();
                                continue;
                            }
                            _ => {}
                        }
                    }
                    match code {
                        KeyCode::Left => {
                            t.textarea.move_left();
                            let _ = t.draw();
                        }
                        KeyCode::Right => {
                            t.textarea.move_right();
                            let _ = t.draw();
                        }
                        KeyCode::Up => {
                            if t.matches.len() > 1 {
                                t.sel = t.sel.saturating_sub(1);
                                let _ = t.draw();
                            } else {
                                t.textarea.move_up();
                                let _ = t.draw();
                            }
                        }
                        KeyCode::Down => {
                            if t.matches.len() > 1 {
                                t.sel = (t.sel + 1).min(t.matches.len().saturating_sub(1));
                                let _ = t.draw();
                            } else {
                                t.textarea.move_down();
                                let _ = t.draw();
                            }
                        }
                        KeyCode::Tab => {
                            if !t.matches.is_empty() {
                                if let Some((name, _)) = t.matches.get(t.sel).cloned() {
                                    t.textarea.set_text(&format!("/{name} "));
                                    t.textarea.move_to_end();
                                    sync_matches(&mut t);
                                }
                                let _ = t.draw();
                            }
                        }
                        KeyCode::Enter => {
                            let is_newline = modifiers.contains(KeyModifiers::SHIFT)
                                || modifiers.contains(KeyModifiers::ALT);
                            if is_newline {
                                t.textarea.insert_str("\n");
                                sync_matches(&mut t);
                                let _ = t.draw();
                                continue;
                            }
                            // if popup open and selection not yet applied, complete first
                            if !t.matches.is_empty()
                                && let Some((name, _)) = t.matches.get(t.sel).cloned()
                            {
                                let cur_text = t.textarea.text().to_string();
                                if cur_text != format!("/{name}") && cur_text != format!("/{name} ")
                                {
                                    t.textarea.set_text(&format!("/{name} "));
                                    t.textarea.move_to_end();
                                    sync_matches(&mut t);
                                    let _ = t.draw();
                                    continue;
                                }
                            }
                            let mut text = t.textarea.text().to_string();
                            for (ph, full) in &t.pending_pastes {
                                text = text.replace(ph, full);
                            }
                            text = text.trim().to_string();
                            let attached_with_ph: Vec<(String, std::path::PathBuf)> =
                                std::mem::take(&mut t.attachments);
                            let attached: Vec<std::path::PathBuf> =
                                attached_with_ph.into_iter().map(|(_, p)| p).collect();
                            // clear pending pastes already handled
                            if text.is_empty() && attached.is_empty() {
                                continue;
                            }
                            // queue it — fleeting, not transcript (becomes real prompt when dequeued)
                            t.queued_inputs.push_back((text.clone(), attached.clone()));
                            t.textarea.set_text("");
                            t.pending_pastes.clear();
                            t.matches.clear();
                            t.sel = 0;
                            let _ = t.draw();
                        }
                        KeyCode::Char(c) => {
                            t.textarea.insert_str(&c.to_string());
                            sync_matches(&mut t);
                            let _ = t.draw();
                        }
                        KeyCode::Backspace => {
                            if modifiers.contains(KeyModifiers::ALT)
                                || modifiers.contains(KeyModifiers::CONTROL)
                            {
                                t.textarea.delete_word_backward();
                            } else {
                                t.textarea.delete_backward(1);
                            }
                            t.sync_attachments();
                            sync_matches(&mut t);
                            let _ = t.draw();
                        }
                        KeyCode::Delete => {
                            if modifiers.contains(KeyModifiers::ALT)
                                || modifiers.contains(KeyModifiers::CONTROL)
                            {
                                t.textarea.delete_word_forward();
                            } else {
                                t.textarea.delete_forward(1);
                            }
                            t.sync_attachments();
                            sync_matches(&mut t);
                            let _ = t.draw();
                        }
                        KeyCode::Esc => {
                            if !t.matches.is_empty() {
                                t.matches.clear();
                                t.sel = 0;
                                let _ = t.draw();
                            } else {
                                t.textarea.set_text("");
                                t.attachments.clear();
                                t.pending_pastes.clear();
                                t.history_idx = None;
                                t.sel = 0;
                                t.matches.clear();
                                sync_matches(&mut t);
                                let _ = t.draw();
                            }
                        }
                        _ => {}
                    }
                }
                Event::Paste(data) => {
                    let Some(shared) = watcher_tui.as_ref() else {
                        continue;
                    };
                    let Ok(mut t) = shared.try_lock() else {
                        continue;
                    };
                    if !t.is_task_running {
                        continue;
                    }
                    t.handle_paste(data);
                    let cur_text = t.textarea.text().to_string();
                    t.matches = crate::repl::completion_matches_dyn(&cur_text, &cwd_for_watcher);
                    if t.sel >= t.matches.len() {
                        t.sel = t.matches.len().saturating_sub(1);
                    }
                    let _ = t.draw();
                }
                _ => {}
            }
        }
    })
}
