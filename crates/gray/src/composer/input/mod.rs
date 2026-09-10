//! Input handling for composer — extracted from composer/mod.rs:759-1120
//! Keeps raw-mode lifecycle in `super` (mod.rs owns enable_raw_mode / Drop).
//! This module owns attachment helpers and key-dispatch with popup short-circuit.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::Tui;

mod attach;
mod clipboard;

pub(crate) use attach::{sync_attachments, try_attach_clipboard_image, try_attach_image_paste};
pub(crate) use clipboard::paste_from_system_clipboard;

/// Shift+Tab cycles ALL permission modes: Ask(auto) → ReadOnly → Full → Ask.
/// Pure helper so the prompt loop and the mid-turn key watcher share one
/// source of truth.
pub(crate) fn next_permission_mode(current: &str) -> &'static str {
    use gray_core::approvals::{MODE_AUTO, MODE_FULL, MODE_READ_ONLY, normalize_mode};
    match normalize_mode(current).unwrap_or(MODE_AUTO) {
        MODE_READ_ONLY => MODE_FULL,
        MODE_FULL => MODE_AUTO,
        _ => MODE_READ_ONLY,
    }
}

/// Ctrl-C at the prompt never exits on the first press: it clears the draft.
/// Only a second press on an already-empty prompt within the window exits
/// (mirrors the SIGINT policy in `repl`; exit also via /quit or Ctrl-D).
pub(crate) const CTRL_C_EXIT_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) fn ctrl_c_should_exit(
    has_draft: bool,
    last_empty_press: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    if has_draft {
        return false;
    }
    match last_empty_press {
        Some(t) => now.duration_since(t) <= CTRL_C_EXIT_WINDOW,
        None => false,
    }
}

pub(crate) fn handle_paste(tui: &mut Tui, pasted: String) -> bool {
    let pasted = clipboard::normalize_paste(&pasted);
    // opencode parity: some terminals surface an image-only (or otherwise
    // unreadable) clipboard as an EMPTY bracketed paste. Fall back to an
    // explicit OS clipboard read instead of inserting nothing.
    if pasted.trim().is_empty() {
        return clipboard::paste_from_system_clipboard(tui);
    }
    if try_attach_image_paste(tui, &pasted) {
        return true;
    }
    let n = pasted.chars().count();
    let line_count = pasted.split('\n').count();
    // match reference/prime-agent: collapse if >10 lines or >1000 chars
    if line_count > 10 || n > 1000 {
        let max_id = tui
            .pending_pastes
            .iter()
            .filter_map(|(ph, _)| {
                ph.strip_prefix("[paste #")
                    .and_then(|s| s.split([' ', ']']).next())
                    .and_then(|num| num.parse::<usize>().ok())
            })
            .max()
            .unwrap_or(0);
        let id = max_id + 1;
        let placeholder = if line_count > 10 {
            format!("[paste #{id} +{line_count} lines]")
        } else {
            format!("[paste #{id} {n} chars]")
        };
        tui.textarea.insert_element(&placeholder);
        tui.pending_pastes.push((placeholder, pasted));
    } else {
        tui.textarea.insert_str(&pasted);
    }
    let _ = tui.draw();
    true
}

// ---------------------------------------------------------------------------
// Key dispatch with popup short-circuit
// ---------------------------------------------------------------------------

/// Returns true if popup consumed the key. Caller should short-circuit further handling.
pub(crate) fn handle_popup_key(
    tui: &mut Tui,
    code: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) -> bool {
    if tui.matches.is_empty() {
        return false;
    }
    use crossterm::event::{KeyCode, KeyModifiers};
    match code {
        // Popup navigates only when there is something to navigate (>1 match).
        // With a single match, Up/Down fall through to history recall /
        // cursor movement instead of being swallowed by the selector.
        KeyCode::Up if tui.matches.len() > 1 => {
            tui.sel = tui.sel.saturating_sub(1);
            return true;
        }
        KeyCode::Down if tui.matches.len() > 1 => {
            tui.sel = (tui.sel + 1).min(tui.matches.len().saturating_sub(1));
            return true;
        }
        KeyCode::Up | KeyCode::Down => return false,
        KeyCode::Esc => {
            tui.matches.clear();
            tui.sel = 0;
            return true;
        }
        _ => {}
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('p') => {
                tui.sel = tui.sel.saturating_sub(1);
                return true;
            }
            KeyCode::Char('n') => {
                tui.sel = (tui.sel + 1).min(tui.matches.len().saturating_sub(1));
                return true;
            }
            _ => {}
        }
        // While popup is open, Ctrl word-moves are swallowed (short-circuit)
        if matches!(
            code,
            KeyCode::Left
                | KeyCode::Right
                | KeyCode::Char('b')
                | KeyCode::Char('f')
                | KeyCode::Char('d')
                | KeyCode::Char('w')
                | KeyCode::Backspace
                | KeyCode::Delete
        ) {
            return true;
        }
    }
    if modifiers.contains(KeyModifiers::ALT) {
        // Alt word moves swallowed when popup visible
        return true;
    }
    // Block generic word/char moves that would corrupt completion context
    // Enter/Tab are handled separately in main loop but we don't consume them here
    // so caller can apply completion.
    false
}

/// Non-popup key handling — word moves, char insert, deletes, cursor moves, history.
/// Called only when popup did not short-circuit.
pub(crate) fn handle_key_event_without_popup(
    tui: &mut Tui,
    code: crossterm::event::KeyCode,
    modifiers: crossterm::event::KeyModifiers,
) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    match code {
        KeyCode::Char(c) => {
            // plain char insert at cursor (codex textarea insert_str)
            tui.textarea.insert_str(&c.to_string());
            tui.history_idx = None;
            tui.sel = 0;
            true
        }
        KeyCode::Backspace => {
            if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) {
                tui.textarea.delete_word_backward();
            } else {
                tui.textarea.delete_backward(1);
            }
            sync_attachments(tui);
            tui.sel = 0;
            true
        }
        KeyCode::Delete => {
            if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) {
                tui.textarea.delete_word_forward();
            } else {
                tui.textarea.delete_forward(1);
            }
            sync_attachments(tui);
            tui.sel = 0;
            true
        }
        KeyCode::Left => {
            if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) {
                tui.textarea.move_word_left();
            } else {
                tui.textarea.move_left();
            }
            true
        }
        KeyCode::Right => {
            if modifiers.contains(KeyModifiers::ALT) || modifiers.contains(KeyModifiers::CONTROL) {
                tui.textarea.move_word_right();
            } else {
                tui.textarea.move_right();
            }
            true
        }
        KeyCode::Up => {
            // history navigation when at top or single-line; otherwise move cursor up (codex)
            let has_multiline = tui.textarea.text().contains('\n');
            let at_top = tui.textarea.cursor() == 0 || !has_multiline;
            if at_top && !tui.history.is_empty() {
                if tui.history_idx.is_none() {
                    tui.draft = tui.textarea.text().to_string();
                    tui.history_idx = Some(tui.history.len());
                }
                if let Some(idx) = tui.history_idx.as_mut()
                    && *idx > 0
                {
                    *idx -= 1;
                    let h = tui.history[*idx].clone();
                    tui.textarea.set_text(&h);
                    tui.textarea.move_to_end();
                }
            } else {
                tui.textarea.move_up();
            }
            true
        }
        KeyCode::Down => {
            if tui.history_idx.is_some() {
                let idx = tui.history_idx.unwrap();
                if idx + 1 >= tui.history.len() {
                    tui.textarea.set_text(&tui.draft);
                    tui.textarea.move_to_end();
                    tui.history_idx = None;
                } else {
                    tui.history_idx = Some(idx + 1);
                    let h = tui.history[idx + 1].clone();
                    tui.textarea.set_text(&h);
                    tui.textarea.move_to_end();
                }
            } else {
                tui.textarea.move_down();
            }
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// read_line — main loop, verbatim from mod.rs 887-1120 with dispatch split
// ---------------------------------------------------------------------------

/// Pushes Kitty `DISAMBIGUATE_ESCAPE_CODES` while the prompt is live so
/// Shift+Enter (and Alt+Enter) arrive with their modifiers instead of a
/// bare Enter (submit) — tmux otherwise collapses Shift+Enter to `\r`.
/// Popped on drop, covering every `read_line` exit. Terminals without
/// support ignore the sequence (same precedent as `EnableBracketedPaste`
/// below, re-asserted every turn because full-screen children clear it).
struct KeyboardEnhancementGuard;
impl KeyboardEnhancementGuard {
    fn push() -> Self {
        use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
        let _ = crossterm::execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        Self
    }
}
impl Drop for KeyboardEnhancementGuard {
    fn drop(&mut self) {
        use crossterm::event::PopKeyboardEnhancementFlags;
        let _ = crossterm::execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
}

/// Reads one submitted line, redrawing on each keystroke. The TUI lock is
/// held only per phase — never across the input wait — so background
/// painters (boot watcher, footer ticker) can draw while idling at the
/// prompt. Keystrokes queue in the pty and nothing else reads stdin, so no
/// event is lost between the unlocked poll and the locked read.
pub(crate) fn read_line(
    shared: &super::SharedTui,
) -> anyhow::Result<Option<(String, Vec<PathBuf>)>> {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll, read};

    // raw-mode is owned by mod.rs (Tui::new / Drop), but we ensure it here for
    // interactive loop; mod.rs remains canonical owner.
    // Bracketed paste is terminal-global mode 2004: re-assert every prompt
    // turn. Full-screen children ($EDITOR, pagers) commonly clear it on
    // exit, which silently downgrades later pastes to raw keystrokes
    // (multi-line paste then submits on the first Enter). Idempotent.
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
    let _keyboard_enhancement = KeyboardEnhancementGuard::push();
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(std::io::stdout(), crossterm::cursor::Show)?;

    let mut needs_draw = true;
    // Bare Ctrl-C never quits on first press: an empty-prompt press arms a
    // 5 s window, a second press inside it exits (read repl Ctrl-C policy).
    let mut last_ctrl_c_empty: Option<std::time::Instant> = None;
    loop {
        // Phase 1 (locked): resize deadlines, completion recompute, draw.
        // The guard drops at the end of this block, freeing the lock while
        // we wait for input below so background painters can draw.
        let timeout = {
            let mut guard = shared.lock().expect("tui lock");
            let tui: &mut super::Tui = &mut guard;
            if let Some((cols, deadline)) = tui.pending_resize
                && std::time::Instant::now() >= deadline
            {
                tui.pending_resize = None;
                let (live_cols, live_rows) =
                    crossterm::terminal::size().unwrap_or((cols, tui.last_height));
                if live_cols != tui.last_width || live_rows != tui.last_height {
                    tui.reflow_on_resize(live_cols);
                    needs_draw = false;
                }
            }

            if needs_draw {
                let cur_text = tui.textarea.text().to_string();
                tui.matches =
                    crate::repl::completion_matches_dyn(&cur_text, std::path::Path::new(&tui.cwd));
                if tui.sel >= tui.matches.len() {
                    tui.sel = tui.matches.len().saturating_sub(1);
                }
                tui.draw()?;
                needs_draw = false;
            }

            if let Some((_, deadline)) = tui.pending_resize {
                let now = std::time::Instant::now();
                if deadline > now {
                    deadline - now
                } else {
                    Duration::from_millis(0)
                }
            } else {
                Duration::from_millis(250)
            }
        };
        // Phase 2 (unlocked): wait. Keystrokes queue in the pty and nothing
        // else reads stdin, so no event is lost before the locked read below.
        if !poll(timeout)? {
            continue;
        }
        needs_draw = true;
        // Phase 3 (locked): consume + handle exactly one event.
        let mut guard = shared.lock().expect("tui lock");
        let tui: &mut super::Tui = &mut guard;
        let ev = read()?;
        if tui.active_question.is_some() {
            if let Event::Key(KeyEvent {
                code,
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) = ev
            {
                // Ctrl-C on the question overlay cancels via the overlay
                // handler — it must not quit the app (same as mid-turn).
                crate::composer::handle_question_key(tui, code, modifiers);
            }
            continue;
        }
        match ev {
            Event::Resize(cols, rows) => {
                if cols != tui.last_width || rows != tui.last_height {
                    tui.pending_resize = Some((
                        cols,
                        std::time::Instant::now() + std::time::Duration::from_millis(75),
                    ));
                }
                needs_draw = false;
            }
            Event::Paste(data) => {
                handle_paste(tui, data);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Insert,
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) if modifiers.contains(KeyModifiers::SHIFT) => {
                // Shift+Insert reaches us as a key (not bracketed paste) on
                // terminals that pass it through — same opencode prompt.paste.
                paste_from_system_clipboard(tui);
                tui.sel = 0;
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) if modifiers.contains(KeyModifiers::CONTROL) => {
                let has_draft = !tui.textarea.is_empty()
                    || !tui.attachments.is_empty()
                    || !tui.pending_pastes.is_empty();
                if has_draft {
                    tui.textarea.set_text("");
                    tui.attachments.clear();
                    tui.pending_pastes.clear();
                    tui.history_idx = None;
                    tui.sel = 0;
                    last_ctrl_c_empty = None;
                    continue;
                }
                let now = std::time::Instant::now();
                if ctrl_c_should_exit(false, last_ctrl_c_empty, now) {
                    return Ok(None);
                }
                last_ctrl_c_empty = Some(now);
                continue;
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char('d'),
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) if modifiers.contains(KeyModifiers::CONTROL) && tui.textarea.is_empty() => {
                return Ok(None);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc,
                kind,
                ..
            }) if kind == KeyEventKind::Press || kind == KeyEventKind::Repeat => {
                if !tui.matches.is_empty() {
                    tui.matches.clear();
                    tui.sel = 0;
                } else {
                    tui.textarea.set_text("");
                    tui.attachments.clear();
                    tui.pending_pastes.clear();
                    tui.history_idx = None;
                    tui.sel = 0;
                }
            }
            Event::Key(KeyEvent {
                code,
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) if modifiers.contains(KeyModifiers::ALT) => {
                // popup short-circuit: word moves swallowed when completion visible
                if code == KeyCode::Enter {
                    // Alt+Enter inserts a newline (same as Shift+Enter/Ctrl-J),
                    // bypassing the popup exactly like the Shift+Enter path.
                    // Must precede handle_popup_key, which swallows every ALT
                    // key while the popup is open.
                    tui.textarea.insert_str("\n");
                } else if handle_popup_key(tui, code, modifiers) {
                    // consumed
                } else {
                    match code {
                        KeyCode::Backspace => {
                            tui.textarea.delete_word_backward();
                            sync_attachments(tui);
                            tui.sel = 0;
                        }
                        KeyCode::Delete => {
                            tui.textarea.delete_word_forward();
                            sync_attachments(tui);
                            tui.sel = 0;
                        }
                        KeyCode::Char('d') => {
                            tui.textarea.delete_word_forward();
                            tui.sel = 0;
                        }
                        KeyCode::Char('b') | KeyCode::Left => tui.textarea.move_word_left(),
                        KeyCode::Char('f') | KeyCode::Right => tui.textarea.move_word_right(),
                        _ => {}
                    }
                }
            }
            Event::Key(KeyEvent {
                code,
                modifiers,
                kind: KeyEventKind::Press,
                ..
            }) if modifiers.contains(KeyModifiers::CONTROL) => match code {
                KeyCode::Char('p') => {
                    if !handle_popup_key(tui, code, modifiers) {
                        tui.sel = tui.sel.saturating_sub(1);
                    }
                }
                KeyCode::Char('n') => {
                    if !handle_popup_key(tui, code, modifiers) {
                        tui.sel = (tui.sel + 1).min(tui.matches.len().saturating_sub(1));
                    }
                }
                KeyCode::Char('u') => {
                    tui.textarea.set_text("");
                    tui.history_idx = None;
                }
                KeyCode::Char('a') => tui.textarea.set_cursor(0),
                KeyCode::Char('e') => tui.textarea.move_to_end(),
                KeyCode::Char('k') => {
                    let cur = tui.textarea.cursor();
                    tui.textarea.replace_range(cur..usize::MAX, "");
                }
                KeyCode::Char('v') | KeyCode::Char('V') => {
                    // opencode `prompt.paste`: image attach first, then OS
                    // clipboard text — Ctrl+V must paste text even on builds
                    // without the `clipboard` feature.
                    paste_from_system_clipboard(tui);
                    tui.sel = 0;
                }
                KeyCode::Char('w') | KeyCode::Backspace => {
                    if handle_popup_key(tui, code, modifiers) {
                    } else {
                        tui.textarea.delete_word_backward();
                        sync_attachments(tui);
                        tui.sel = 0;
                    }
                }
                KeyCode::Delete => {
                    if handle_popup_key(tui, code, modifiers) {
                    } else {
                        tui.textarea.delete_word_forward();
                        sync_attachments(tui);
                        tui.sel = 0;
                    }
                }
                KeyCode::Left => {
                    if handle_popup_key(tui, code, modifiers) {
                    } else {
                        tui.textarea.move_word_left();
                    }
                }
                KeyCode::Right => {
                    if handle_popup_key(tui, code, modifiers) {
                    } else {
                        tui.textarea.move_word_right();
                    }
                }
                KeyCode::Char('j') | KeyCode::Char('m') => {
                    tui.textarea.insert_str("\n");
                }
                _ => {}
            },
            Event::Key(KeyEvent {
                code,
                kind,
                modifiers,
                ..
            }) if kind == KeyEventKind::Press || kind == KeyEventKind::Repeat => {
                match code {
                    KeyCode::Enter => {
                        let is_newline = modifiers.contains(KeyModifiers::SHIFT)
                            || modifiers.contains(KeyModifiers::ALT);
                        if is_newline {
                            tui.textarea.insert_str("\n");
                            continue;
                        }
                        let cur_text = tui.textarea.text().to_string();
                        if !tui.matches.is_empty()
                            && let Some((name, _)) = tui.matches.get(tui.sel)
                            && cur_text != format!("/{name}")
                            && cur_text != format!("/{name} ")
                        {
                            // bare `/skills` opens the interactive skill picker
                            let fill = crate::repl::completion_fill(name);
                            tui.textarea.set_text(&fill);
                            tui.textarea.move_to_end();
                            continue;
                        }
                        let mut text = tui.textarea.text().to_string();
                        for (ph, full) in &tui.pending_pastes {
                            text = text.replace(ph, full);
                        }
                        tui.pending_pastes.clear();
                        let trimmed = text.trim().to_string();
                        if trimmed.is_empty() && tui.attachments.is_empty() {
                            continue;
                        }
                        if !trimmed.is_empty() {
                            tui.history.push(trimmed.clone());
                            if tui.history.len() > 100 {
                                tui.history.remove(0);
                            }
                        }
                        tui.history_idx = None;
                        tui.draft.clear();
                        tui.textarea.set_text("");
                        let attached_with_ph: Vec<(String, PathBuf)> =
                            std::mem::take(&mut tui.attachments);
                        let attached: Vec<PathBuf> =
                            attached_with_ph.into_iter().map(|(_, p)| p).collect();
                        if tui.is_task_running {
                            tui.queued_inputs
                                .push_back((trimmed.clone(), attached.clone()));
                            tui.matches.clear();
                            tui.sel = 0;
                            let _ = tui.draw();
                            continue;
                        }
                        tui.matches.clear();
                        tui.sel = 0;
                        // Slash commands hug their feedback: no trailing gap, say() output follows directly.
                        tui.push_user_prompt(&trimmed, &attached, !trimmed.starts_with('/'));
                        return Ok(Some((trimmed, attached)));
                    }
                    KeyCode::BackTab => {
                        let next = next_permission_mode(tui.permission_mode());
                        tui.set_permission_mode(next.to_string());
                        tui.pending_permission_mode = Some(next.to_string());
                        let _ = tui.draw();
                    }
                    KeyCode::Tab => {
                        if let Some((name, _)) = tui.matches.get(tui.sel) {
                            let fill = crate::repl::completion_fill(name);
                            tui.textarea.set_text(&fill);
                            tui.textarea.move_to_end();
                        }
                    }
                    KeyCode::Char(_)
                    | KeyCode::Backspace
                    | KeyCode::Delete
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down => {
                        // Single dispatch: popup nav first, then the shared
                        // editing/history behavior — no inline duplicates.
                        // (Plain Char/Backspace/Delete never trigger the popup
                        // short-circuit; they just update the filter.)
                        if !handle_popup_key(tui, code, modifiers) {
                            handle_key_event_without_popup(tui, code, modifiers);
                        }
                    }
                    _ => {
                        // fallback to helper for any uncovered keys when popup closed
                        handle_key_event_without_popup(tui, code, modifiers);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_tab_cycles_all_three_modes() {
        use gray_core::approvals::{MODE_AUTO, MODE_FULL, MODE_READ_ONLY};
        // Ask(auto) → ReadOnly → Full → Ask
        assert_eq!(next_permission_mode(MODE_AUTO), MODE_READ_ONLY);
        assert_eq!(next_permission_mode("ask"), MODE_READ_ONLY);
        assert_eq!(next_permission_mode(MODE_READ_ONLY), MODE_FULL);
        assert_eq!(next_permission_mode(MODE_FULL), MODE_AUTO);
        assert_eq!(next_permission_mode("full-access"), MODE_AUTO);
        // Full cycle returns to start (the stuck-two-mode bug never hit Full).
        let mut m = MODE_AUTO;
        for _ in 0..3 {
            m = next_permission_mode(m);
        }
        assert_eq!(m, MODE_AUTO);
        // Unknown falls back to auto → next is read-only.
        assert_eq!(next_permission_mode("bogus"), MODE_READ_ONLY);
    }

    #[test]
    fn ctrl_c_first_press_clears_second_within_window_exits() {
        let now = std::time::Instant::now();
        // Draft present → never exit (first press clears).
        assert!(!ctrl_c_should_exit(true, None, now));
        assert!(!ctrl_c_should_exit(true, Some(now), now));
        // Empty, no prior press → arm, don't exit.
        assert!(!ctrl_c_should_exit(false, None, now));
        // Empty, second press inside 5 s → exit.
        let first = now - std::time::Duration::from_secs(2);
        assert!(ctrl_c_should_exit(false, Some(first), now));
        // Empty, prior press expired → don't exit.
        let stale = now - std::time::Duration::from_secs(30);
        assert!(!ctrl_c_should_exit(false, Some(stale), now));
    }
}
