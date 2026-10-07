//! Input handling for composer — extracted from composer/mod.rs:759-1120
//! Keeps raw-mode lifecycle in `super` (mod.rs owns enable_raw_mode / Drop).
//! This module owns attachment helpers and key-dispatch with popup short-circuit.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::Tui;

mod attach;
mod clipboard;
pub(crate) mod keys;

use keys::KeyOutcome;

pub(crate) use attach::{sync_attachments, try_attach_clipboard_image, try_attach_image_paste};
pub(crate) use clipboard::paste_from_system_clipboard;

pub(crate) fn handle_paste(tui: &mut Tui, pasted: String) -> bool {
    let pasted = strip_escape_sequences(&clipboard::normalize_paste(&pasted));
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

/// True when the prompt holds anything Ctrl-C/Esc would throw away.
pub(crate) fn has_draft(tui: &Tui) -> bool {
    !tui.textarea.is_empty() || !tui.attachments.is_empty() || !tui.pending_pastes.is_empty()
}

/// Empties the draft: text, attachments, collapsed pastes, history walk.
pub(crate) fn clear_draft(tui: &mut Tui) {
    tui.textarea.set_text("");
    tui.attachments.clear();
    tui.pending_pastes.clear();
    tui.history_idx = None;
    tui.sel = 0;
    tui.matches.clear();
}

// ---------------------------------------------------------------------------
// read_line — main loop, verbatim from mod.rs 887-1120 with dispatch split
// ---------------------------------------------------------------------------

/// While the prompt is live, ask the terminal to report modified keys so
/// Shift+Enter (and Alt+Enter) arrive as themselves instead of a bare Enter
/// (submit). Popped on drop, covering every `read_line` exit. Terminals
/// without support ignore the sequence (same precedent as
/// `EnableBracketedPaste` below, re-asserted every turn because full-screen
/// children clear it).
///
/// The negotiation itself lives in [`crate::term_keys`]: which flags to ask
/// for, and which terminals must be excluded, is not a prompt concern.
pub(crate) use crate::term_keys::KeyboardEnhancementGuard;

/// Drops escape sequences from a pasted string: CSI runs (`ESC [ <params>
/// <final>`, which includes both bracketed-paste markers `ESC [ 200~` and
/// `ESC [ 201~`), OSC runs, and a lone `ESC`.
///
/// A paste can reach us still wrapped in the bracketed-paste markers when
/// gray is the inner terminal of a nested one (an outer app wrapped the
/// paste before we saw it), and it can drag along CSI/OSC debris copied out
/// of a rendered page. Neither can be meant as text, so delete them instead
/// of inserting `^[[200~` junk into the draft — only escape *sequences* go:
/// newlines and tabs, which a pasted code block legitimately carries, stay.
pub(crate) fn strip_escape_sequences(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            // A lone ESC and the byte after it (ESC =): drop the introducer,
            // keep the byte - deleting a character the user pasted would lose
            // text the escape never explained.
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Reads one submitted line, redrawing on each keystroke. The TUI lock is
/// held only per phase — never across the input wait — so background
/// painters (boot watcher, footer ticker) can draw while idling at the
/// prompt. Keystrokes queue in the pty and nothing else reads stdin, so no
/// event is lost between the unlocked poll and the locked read.
pub(crate) fn read_line(
    shared: &super::SharedTui,
) -> anyhow::Result<Option<(String, Vec<PathBuf>)>> {
    use crossterm::event::{Event, KeyEventKind, poll, read};

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
    loop {
        // Phase 1 (locked): resize deadlines, completion recompute, draw.
        // The guard drops at the end of this block, freeing the lock while
        // we wait for input below so background painters can draw.
        let (timeout, empty_draft) = {
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

            let timeout = if let Some((_, deadline)) = tui.pending_resize {
                let now = std::time::Instant::now();
                if deadline > now {
                    deadline - now
                } else {
                    Duration::from_millis(0)
                }
            } else {
                Duration::from_millis(250)
            };
            let empty_draft = tui.textarea.is_empty()
                && tui.attachments.is_empty()
                && tui.pending_pastes.is_empty();
            (timeout, empty_draft)
        };
        // Phase 2 (unlocked): wait. Keystrokes queue in the pty and nothing
        // else reads stdin, so no event is lost before the locked read below.
        if !poll(timeout)? {
            // Idle wake (cron delivery, finished background job, say line):
            // hand the empty prompt back so the REPL can act. Never while the
            // user has a draft — their typing wins, the wake waits.
            if empty_draft && crate::host::wake_requested() {
                return Ok(Some((String::new(), Vec::new())));
            }
            continue;
        }
        needs_draw = true;
        // Phase 3 (locked): consume + handle exactly one event.
        let mut guard = shared.lock().expect("tui lock");
        let tui: &mut super::Tui = &mut guard;
        let ev = read()?;
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
            Event::Key(ev) if ev.kind != KeyEventKind::Release => {
                match keys::dispatch(tui, &ev) {
                    KeyOutcome::Edited | KeyOutcome::Ignored => {}
                    KeyOutcome::Clear => {
                        // Ctrl-C clears a draft; on an already-empty prompt a
                        // single press exits (the repl SIGINT task exits the
                        // same way — raw mode keeps this a key event, so it
                        // only sees cooked presses after the TUI shut down).
                        if has_draft(tui) {
                            clear_draft(tui);
                            continue;
                        }
                        return Ok(None);
                    }
                    KeyOutcome::Exit => return Ok(None),
                    KeyOutcome::Interrupt => clear_draft(tui),
                    KeyOutcome::Command(cmd) => {
                        tui.matches.clear();
                        tui.sel = 0;
                        if tui.is_task_running {
                            tui.queued_inputs.push_back((cmd, Vec::new()));
                            let _ = tui.draw();
                            continue;
                        }
                        tui.push_user_prompt(&cmd, &[], false);
                        return Ok(Some((cmd, Vec::new())));
                    }
                    KeyOutcome::Submit => {
                        let mut text = tui.textarea.text().to_string();
                        for (ph, full) in &tui.pending_pastes {
                            text = text.replace(ph, full);
                        }
                        tui.pending_pastes.clear();
                        let trimmed = text.trim().to_string();
                        if trimmed.is_empty()
                            && tui.attachments.is_empty()
                            && !tui.allow_empty_submit
                        {
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
                        // Empty continue submits carry no card here — the REPL
                        // paints the `continue` prompt card when it resumes.
                        if trimmed.is_empty() && attached.is_empty() {
                            return Ok(Some((trimmed, attached)));
                        }
                        // Slash commands hug their feedback: no trailing gap, say() output follows directly.
                        tui.push_user_prompt(&trimmed, &attached, !trimmed.starts_with('/'));
                        return Ok(Some((trimmed, attached)));
                    }
                }
            }
            _ => {}
        }
    }
}

#[path = "mod_tests.rs"]
#[cfg(test)]
mod tests;
