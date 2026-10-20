use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::Tui;

mod attach;
mod clipboard;
pub(crate) mod keys;

use keys::KeyOutcome;

pub(crate) use attach::{
    sync_attachment_markers, try_attach_clipboard_image, try_attach_image_paste,
};
pub(crate) use clipboard::paste_from_system_clipboard;

#[cfg(unix)]
pub(crate) fn stdin_gone() -> bool {
    let mut pfd = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };

    let r = unsafe { libc::poll(&mut pfd, 1, 0) };
    r > 0 && pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
}

#[cfg(not(unix))]
pub(crate) fn stdin_gone() -> bool {
    false
}

pub(crate) fn watch_dead_stdin() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) || !std::io::IsTerminal::is_terminal(&std::io::stdin())
    {
        return;
    }
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_millis(500));
            if stdin_gone() {
                #[cfg(unix)]
                unsafe {
                    libc::raise(libc::SIGHUP)
                };
                std::thread::sleep(Duration::from_secs(2));
                std::process::exit(129);
            }
        }
    });
}

pub(crate) fn handle_paste(tui: &mut Tui, pasted: String) -> bool {
    let pasted = strip_escape_sequences(&clipboard::normalize_paste(&pasted));

    if pasted.trim().is_empty() {
        return clipboard::paste_from_system_clipboard(tui);
    }
    if try_attach_image_paste(tui, &pasted) {
        return true;
    }
    let n = pasted.chars().count();
    let line_count = pasted.split('\n').count();

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

pub(crate) fn has_draft(tui: &Tui) -> bool {
    !tui.textarea.is_empty() || !tui.attachments.is_empty() || !tui.pending_pastes.is_empty()
}

pub(crate) fn clear_draft(tui: &mut Tui) {
    if has_draft(tui) {
        tui.killed_draft = Some(super::KilledDraft {
            text: tui.textarea.text().to_string(),
            pending_pastes: std::mem::take(&mut tui.pending_pastes),
            attachments: std::mem::take(&mut tui.attachments),
        });
        tui.set_hint("draft cleared — ctrl-y to yank back");
    }
    tui.textarea.set_text("");
    tui.attachments.clear();
    tui.pending_pastes.clear();
    tui.history_idx = None;
    tui.sel = 0;
    tui.matches.clear();
    tui.muted_text = None;
    tui.search = None;
}

/// A lone ctrl-c/ctrl-d on an empty composer used to quit outright. Arm
/// instead: show a "<key> again to quit" hint and exit only on a second
/// press of the same key inside this window; a later press re-arms.
const QUIT_ARM_WINDOW: Duration = Duration::from_millis(1500);

type QuitArm = Option<(std::time::Instant, crate::keymap::Chord)>;

/// True when `ev` is a fresh press of the armed quit key inside the
/// window. Held-key Repeat events never count as the second press.
fn quit_armed(arm: QuitArm, ev: &crossterm::event::KeyEvent) -> bool {
    if ev.kind != crossterm::event::KeyEventKind::Press {
        return false;
    }
    match arm {
        Some((at, chord)) if at.elapsed() < QUIT_ARM_WINDOW => {
            crate::keymap::Chord::from_event(ev) == Some(chord)
        }
        _ => false,
    }
}

/// First (or stale-window) press of a quit key on the empty composer:
/// arm it and show the "<key> again to quit" hint for the pressed key.
fn arm_quit(tui: &mut Tui, arm: &mut QuitArm, ev: &crossterm::event::KeyEvent) {
    *arm = match ev.kind {
        crossterm::event::KeyEventKind::Press => {
            crate::keymap::Chord::from_event(ev).map(|c| (std::time::Instant::now(), c))
        }
        _ => None,
    };
    let label = crate::keymap::Chord::from_event(ev)
        .map(|c| c.display().replace('+', "-"))
        .unwrap_or_else(|| "same key".to_string());
    let hint = format!("{label} again to quit");
    if !tail_line_is(tui, &hint) {
        tui.push_dim(hint);
    }
}

/// Newest transcript entry already reads `text`, so stacking another
/// identical hint line would add nothing.
fn tail_line_is(tui: &Tui, text: &str) -> bool {
    match tui.history_entries.last() {
        Some(crate::composer::TranscriptEntry::StyledLines { lines, .. }) => {
            lines.len() == 1
                && lines[0]
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    == text
        }
        _ => false,
    }
}

/// Recompute the slash-completion popup from the draft. While a history
/// search is live the popup stays closed, and `muted_text` keeps it closed
/// for the exact draft it was dismissed on (esc) until the text changes.
pub(crate) fn sync_completion(tui: &mut Tui, cwd: &Path) {
    let cur_text = tui.textarea.text().to_string();
    if tui.search.is_some() || tui.muted_text.as_deref() == Some(cur_text.as_str()) {
        tui.matches.clear();
        tui.sel = 0;
        return;
    }
    tui.muted_text = None;
    tui.matches = crate::repl::completion_matches_dyn(&cur_text, cwd);
    if tui.sel >= tui.matches.len() {
        tui.sel = tui.matches.len().saturating_sub(1);
    }
}

pub(crate) use crate::term_keys::KeyboardEnhancementGuard;

/// The keyboard-enhancement push/pop writes raw escapes to stdout; on this
/// thread they race the composer's draws and a torn `\x1b[>5u` leaks its
/// tail as literal text. Run both under the tui lock.
pub(crate) struct LockedEnhancement {
    inner: Option<crate::term_keys::KeyboardEnhancementGuard>,
    tui: Option<super::SharedTui>,
}

impl LockedEnhancement {
    pub(crate) fn push(tui: Option<super::SharedTui>) -> Self {
        let inner = {
            let _tui = tui
                .as_ref()
                .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()));
            crate::term_keys::KeyboardEnhancementGuard::push()
        };
        Self {
            inner: Some(inner),
            tui,
        }
    }
}

impl Drop for LockedEnhancement {
    fn drop(&mut self) {
        let _tui = self
            .tui
            .as_ref()
            .map(|s| s.lock().unwrap_or_else(|e| e.into_inner()));
        drop(self.inner.take());
    }
}

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

            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

pub(crate) fn read_line(
    shared: &super::SharedTui,
) -> anyhow::Result<Option<(String, Vec<PathBuf>)>> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read};

    {
        let _tui = shared.lock().unwrap_or_else(|e| e.into_inner());
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Show)?;
    }
    let _keyboard_enhancement = LockedEnhancement::push(Some(shared.clone()));

    watch_dead_stdin();
    let mut needs_draw = true;
    let mut quit_arm: QuitArm = None;
    loop {
        if stdin_gone() {
            return Ok(None);
        }

        let (timeout, empty_draft) = {
            let mut guard = shared.lock().expect("tui lock");
            let tui: &mut super::Tui = &mut guard;
            if tui.ask_modal.is_some() || tui.modal_open {
                drop(guard);
                std::thread::sleep(Duration::from_millis(30));
                needs_draw = true;
                continue;
            }
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
                let cwd = std::path::PathBuf::from(&tui.cwd);
                sync_completion(tui, &cwd);
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

        if !poll(timeout)? {
            if empty_draft && crate::host::wake_requested() {
                return Ok(Some((String::new(), Vec::new())));
            }
            continue;
        }
        needs_draw = true;

        let mut guard = shared.lock().expect("tui lock");
        let tui: &mut super::Tui = &mut guard;
        let ev = read()?;
        if tui.intro_started.is_some() {
            tui.end_intro();
            let swallow = match ev {
                Event::Key(key) => {
                    matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' '))
                        || key.modifiers.contains(KeyModifiers::CONTROL)
                }
                Event::Resize(..) => true,
                _ => false,
            };
            if swallow {
                continue;
            }
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
                tui.search = None;
                handle_paste(tui, data);
            }
            Event::Key(ev) if ev.kind != KeyEventKind::Release => match keys::dispatch(tui, &ev) {
                KeyOutcome::Edited | KeyOutcome::Ignored => {
                    quit_arm = None;
                }
                KeyOutcome::Clear => {
                    if has_draft(tui) {
                        quit_arm = None;
                        clear_draft(tui);
                        continue;
                    }
                    if quit_armed(quit_arm, &ev) {
                        return Ok(None);
                    }
                    arm_quit(tui, &mut quit_arm, &ev);
                }
                KeyOutcome::Exit => {
                    if quit_armed(quit_arm, &ev) {
                        return Ok(None);
                    }
                    arm_quit(tui, &mut quit_arm, &ev);
                }
                KeyOutcome::Interrupt => {
                    quit_arm = None;
                    clear_draft(tui);
                }
                KeyOutcome::Command(cmd) => {
                    quit_arm = None;
                    tui.matches.clear();
                    tui.sel = 0;
                    if tui.is_task_running {
                        tui.queued_inputs.push_back((cmd, Vec::new()));
                        let _ = tui.draw();
                        continue;
                    }
                    echo_submit(tui, &cmd, &[]);
                    return Ok(Some((cmd, Vec::new())));
                }
                KeyOutcome::Submit => {
                    quit_arm = None;
                    let mut text = tui.textarea.text().to_string();
                    for (ph, full) in &tui.pending_pastes {
                        text = text.replace(ph, full);
                    }
                    tui.pending_pastes.clear();
                    let trimmed = text.trim().to_string();
                    if trimmed.is_empty() && tui.attachments.is_empty() && !tui.allow_empty_submit {
                        continue;
                    }
                    crate::prompt_history::record(&mut tui.history, &trimmed);
                    tui.history_idx = None;
                    tui.draft.clear();
                    attach::sync_attachment_markers(tui);
                    let attached: Vec<PathBuf> = std::mem::take(&mut tui.attachments);
                    tui.textarea.set_text("");
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

                    if trimmed.is_empty() && attached.is_empty() {
                        return Ok(Some((trimmed, attached)));
                    }

                    echo_submit(tui, &trimmed, &attached);
                    return Ok(Some((trimmed, attached)));
                }
            },
            _ => {}
        }
    }
}

fn echo_submit(tui: &mut Tui, text: &str, attached: &[PathBuf]) {
    if !crate::repl::parse_command(text).runs_local() {
        tui.push_user_prompt(text, attached, !text.starts_with('/'));
    }
}

#[path = "mod_tests.rs"]
#[cfg(test)]
mod tests;
