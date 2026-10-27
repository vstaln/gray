use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::Tui;
use crate::keymap::{Action, Binding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyOutcome {
    Edited,

    Ignored,

    Submit,

    Clear,

    Exit,

    Interrupt,

    Command(String),
}

pub(crate) fn dispatch(tui: &mut Tui, ev: &KeyEvent) -> KeyOutcome {
    let bindings = crate::keymap::resolve_event(ev);
    dispatch_bindings(tui, ev, &bindings)
}

pub(crate) fn dispatch_bindings(tui: &mut Tui, ev: &KeyEvent, bindings: &[Binding]) -> KeyOutcome {
    if tui.search.is_some()
        && let Some(out) = dispatch_in_search(tui, ev, bindings)
    {
        return out;
    }
    for b in bindings {
        match b {
            Binding::Command(cmd) => return KeyOutcome::Command(cmd.clone()),
            Binding::Action(a) => {
                if let Some(out) = apply(tui, *a) {
                    return out;
                }
            }
        }
    }

    if let KeyCode::Char(c) = ev.code
        && !ev
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        tui.textarea.insert_str(&c.to_string());
        tui.history_idx = None;
        tui.sel = 0;
        return KeyOutcome::Edited;
    }
    KeyOutcome::Ignored
}

fn apply(tui: &mut Tui, a: Action) -> Option<KeyOutcome> {
    use KeyOutcome::*;
    let popup = !tui.matches.is_empty();
    let multi = tui.matches.len() > 1;
    let edited = Some(Edited);
    match a {
        Action::SelectUp => {
            if !multi {
                return None;
            }
            tui.sel = tui.sel.saturating_sub(1);
            edited
        }
        Action::SelectDown => {
            if !multi {
                return None;
            }
            tui.sel = (tui.sel + 1).min(tui.matches.len().saturating_sub(1));
            edited
        }
        Action::SelectCancel => {
            if !popup {
                return None;
            }
            tui.matches.clear();
            tui.sel = 0;
            tui.muted_text = Some(tui.textarea.text().to_string());
            edited
        }
        Action::Tab => {
            let (name, _) = tui.matches.get(tui.sel)?.clone();
            fill_completion(tui, &name);
            edited
        }
        Action::Clear => Some(Clear),
        Action::Exit => (!super::has_draft(tui)).then_some(Exit),
        Action::Interrupt => Some(Interrupt),
        Action::PasteClipboard => {
            super::paste_from_system_clipboard(tui);
            tui.sel = 0;
            edited
        }
        Action::NewLine => {
            tui.textarea.insert_str("\n");
            edited
        }
        Action::Submit => {
            if let Some((name, _)) = tui.matches.get(tui.sel).cloned() {
                let cur = tui.textarea.text();
                if cur != format!("/{name}") && cur != format!("/{name} ") {
                    fill_completion(tui, &name);
                    return edited;
                }
            }
            Some(Submit)
        }
        Action::HistoryPrevious => {
            history_prev(tui);
            edited
        }
        Action::HistoryNext => {
            history_next(tui);
            edited
        }
        Action::HistorySearch => {
            enter_search(tui);
            edited
        }
        Action::CursorUp => {
            let has_multiline = tui.textarea.text().contains('\n');
            // While browsing history, Up keeps walking older entries even
            // when the recalled one is multi-line (Down already mirrors this);
            // otherwise a tall entry traps the cursor inside it.
            let at_top = tui.history_idx.is_some() || tui.textarea.cursor() == 0 || !has_multiline;
            if at_top && !tui.history.is_empty() {
                history_prev(tui);
            } else {
                tui.textarea.move_up();
            }
            edited
        }
        Action::CursorDown => {
            if tui.history_idx.is_some() {
                history_next(tui);
            } else {
                tui.textarea.move_down();
            }
            edited
        }
        Action::CursorLeft => {
            tui.textarea.move_left();
            edited
        }
        Action::CursorRight => {
            tui.textarea.move_right();
            edited
        }
        Action::CursorWordLeft => {
            tui.textarea.move_word_left();
            edited
        }
        Action::CursorWordRight => {
            tui.textarea.move_word_right();
            edited
        }
        Action::CursorLineStart => {
            let p = line_start(tui.textarea.text(), tui.textarea.cursor());
            tui.textarea.set_cursor(p);
            edited
        }
        Action::CursorLineEnd => {
            let p = line_end(tui.textarea.text(), tui.textarea.cursor());
            tui.textarea.set_cursor(p);
            edited
        }
        Action::DeleteCharBackward => edit(tui, |t| t.textarea.delete_backward()),
        Action::DeleteCharForward => edit(tui, |t| t.textarea.delete_forward()),
        Action::DeleteWordBackward => edit(tui, |t| t.textarea.delete_word_backward()),
        Action::DeleteWordForward => edit(tui, |t| t.textarea.delete_word_forward()),
        Action::DeleteToLineStart => edit(tui, |t| {
            let cur = t.textarea.cursor();
            let start = line_start(t.textarea.text(), cur);

            let start = if start == cur && cur > 0 {
                cur - 1
            } else {
                start
            };
            t.textarea.replace_range(start..cur, "");
        }),
        Action::DeleteToLineEnd => edit(tui, |t| {
            let cur = t.textarea.cursor();
            let end = line_end(t.textarea.text(), cur);
            let end = if end == cur && cur < t.textarea.text().len() {
                cur + 1
            } else {
                end
            };
            t.textarea.replace_range(cur..end, "");
        }),
        Action::Yank => yank_killed(tui),
    }
}

/// ctrl-y: restore the draft last wiped by esc/ctrl-c (one-slot kill ring).
/// Empty composer restores the draft as it was; otherwise the killed text
/// is inserted at the cursor instead of clobbering the current draft.
fn yank_killed(tui: &mut Tui) -> Option<KeyOutcome> {
    let killed = tui.killed_draft.take()?;
    tui.hint = None;
    tui.history_idx = None;
    if tui.textarea.is_empty() {
        tui.textarea.set_text(&killed.text);
        tui.textarea.move_to_end();
        tui.pending_pastes = killed.pending_pastes;
        tui.attachments = killed.attachments;
    } else {
        tui.textarea.insert_str(&killed.text);
        tui.pending_pastes.extend(killed.pending_pastes);
        for path in killed.attachments {
            if !tui.attachments.contains(&path) {
                tui.attachments.push(path);
            }
        }
    }
    super::attach::sync_attachment_markers(tui);
    tui.sel = 0;
    Some(KeyOutcome::Edited)
}

fn edit(tui: &mut Tui, f: impl FnOnce(&mut Tui)) -> Option<KeyOutcome> {
    f(tui);
    super::attach::sync_attachment_markers(tui);
    tui.sel = 0;
    Some(KeyOutcome::Edited)
}

fn fill_completion(tui: &mut Tui, name: &str) {
    let fill = crate::repl::completion_fill(name);
    tui.textarea.set_text(&fill);
    tui.textarea.move_to_end();
}

fn history_prev(tui: &mut Tui) {
    if tui.history.is_empty() {
        return;
    }
    if tui.history_idx.is_none() {
        tui.draft = tui.textarea.text().to_string();
        tui.history_idx = Some(tui.history.len());
    }
    if let Some(idx) = tui.history_idx
        && idx > 0
    {
        tui.history_idx = Some(idx - 1);
        let h = tui.history[idx - 1].clone();
        tui.textarea.set_text(&h);
        tui.textarea.move_to_end();
    }
}

fn history_next(tui: &mut Tui) {
    let Some(idx) = tui.history_idx else {
        return;
    };
    if idx + 1 >= tui.history.len() {
        let draft = std::mem::take(&mut tui.draft);
        tui.textarea.set_text(&draft);
        tui.history_idx = None;
    } else {
        tui.history_idx = Some(idx + 1);
        let h = tui.history[idx + 1].clone();
        tui.textarea.set_text(&h);
    }
    tui.textarea.move_to_end();
}

/// ctrl-r entry point: stash the draft, preview the newest history entry
/// (empty query matches everything), and hand the textarea over to the
/// match preview until the search is accepted or cancelled.
fn enter_search(tui: &mut Tui) {
    if tui.search.is_some() {
        return;
    }
    let saved = tui.textarea.text().to_string();
    let saved_cursor = tui.textarea.cursor();
    tui.search = Some(super::super::HistSearch {
        query: String::new(),
        saved,
        saved_cursor,
        idx: None,
    });
    tui.matches.clear();
    tui.sel = 0;
    tui.history_idx = None;
    search_newest(tui);
}

/// Exit search keeping the previewed match (or the stashed draft when no
/// history entry matched).
fn accept_search(tui: &mut Tui) {
    tui.search = None;
}

/// Exit search restoring the draft exactly as it was before ctrl-r.
fn cancel_search(tui: &mut Tui) {
    let Some(s) = tui.search.take() else {
        return;
    };
    tui.textarea.set_text(&s.saved);
    tui.textarea.set_cursor(s.saved_cursor);
}

/// Show `history[idx]` as the match preview, or the stashed draft on None.
fn set_match(tui: &mut Tui, idx: Option<usize>) {
    let Some(s) = &mut tui.search else {
        return;
    };
    s.idx = idx;
    let preview = idx
        .map(|i| tui.history[i].clone())
        .unwrap_or_else(|| s.saved.clone());
    tui.textarea.set_text(&preview);
    tui.textarea.move_to_end();
}

fn search_newest(tui: &mut Tui) {
    let Some(s) = &tui.search else {
        return;
    };
    let q = s.query.clone();
    let idx = tui.history.iter().rposition(|h| h.contains(&q));
    set_match(tui, idx);
}

fn search_older(tui: &mut Tui) {
    let Some(s) = &tui.search else {
        return;
    };
    let q = s.query.clone();
    let start = s.idx.unwrap_or(tui.history.len());
    if let Some(i) = tui.history[..start].iter().rposition(|h| h.contains(&q)) {
        set_match(tui, Some(i));
    }
}

fn search_newer(tui: &mut Tui) {
    let Some(s) = &tui.search else {
        return;
    };
    let Some(cur) = s.idx else {
        return;
    };
    let q = s.query.clone();
    let idx = tui.history[cur + 1..]
        .iter()
        .position(|h| h.contains(&q))
        .map(|o| cur + 1 + o);
    if let Some(i) = idx {
        set_match(tui, Some(i));
    }
}

/// Key handling while a history search is live. `Some(outcome)` means the
/// key was consumed by the search; `None` means "accept the match and let
/// the key fall through to normal editing" (readline-style).
fn dispatch_in_search(tui: &mut Tui, ev: &KeyEvent, bindings: &[Binding]) -> Option<KeyOutcome> {
    // The first binding decides: bindings arrive ordered (custom commands,
    // then actions in keymap order), exactly like normal dispatch.
    if let Some(b) = bindings.first() {
        match b {
            Binding::Command(_) => {
                accept_search(tui);
                return None;
            }
            Binding::Action(a) => match a {
                Action::SelectCancel | Action::Interrupt | Action::Clear => {
                    cancel_search(tui);
                    return Some(KeyOutcome::Edited);
                }
                Action::Submit | Action::NewLine => {
                    accept_search(tui);
                    return Some(KeyOutcome::Edited);
                }
                Action::HistorySearch
                | Action::SelectUp
                | Action::HistoryPrevious
                | Action::CursorUp => {
                    search_older(tui);
                    return Some(KeyOutcome::Edited);
                }
                Action::SelectDown | Action::HistoryNext | Action::CursorDown => {
                    search_newer(tui);
                    return Some(KeyOutcome::Edited);
                }
                Action::DeleteCharBackward => {
                    if let Some(s) = &mut tui.search {
                        s.query.pop();
                    }
                    search_newest(tui);
                    return Some(KeyOutcome::Edited);
                }
                Action::DeleteWordBackward => {
                    if let Some(s) = &mut tui.search {
                        let q = &mut s.query;
                        while q.chars().last().is_some_and(|c| c.is_whitespace()) {
                            q.pop();
                        }
                        while q.chars().last().is_some_and(|c| !c.is_whitespace()) {
                            q.pop();
                        }
                    }
                    search_newest(tui);
                    return Some(KeyOutcome::Edited);
                }
                _ => {
                    accept_search(tui);
                    return None;
                }
            },
        }
    }
    if let KeyCode::Char(c) = ev.code
        && !ev
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        && let Some(s) = &mut tui.search
    {
        s.query.push(c);
        search_newest(tui);
        return Some(KeyOutcome::Edited);
    }
    accept_search(tui);
    None
}

pub(crate) fn line_start(text: &str, cur: usize) -> usize {
    let cur = cur.min(text.len());
    text[..cur].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

pub(crate) fn line_end(text: &str, cur: usize) -> usize {
    let cur = cur.min(text.len());
    text[cur..]
        .find('\n')
        .map(|i| i + cur)
        .unwrap_or(text.len())
}
