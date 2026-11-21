use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use gray_core::agent::{KeyAction, KeyPress, KeyReply};

use super::super::Tui;
use super::keys::{self, KeyOutcome};
use crate::keymap::Action;

fn code_of(code: KeyCode) -> Option<String> {
    Some(match code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BackTab".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Insert => "Insert".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        _ => return None,
    })
}

pub(crate) fn dispatch(tui: &mut Tui, ev: &KeyEvent) -> Option<KeyOutcome> {
    if ev.kind == KeyEventKind::Release || !gray_plugin::has_input_key_hook() {
        return None;
    }
    let runtime = tokio::runtime::Handle::try_current()
        .ok()
        .filter(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)?;
    let press = KeyPress {
        text: tui.textarea.text().to_string(),
        cursor: tui.textarea.cursor(),
        completing: !tui.matches.is_empty(),
        code: code_of(ev.code)?,
        ctrl: ev
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER),
        alt: ev.modifiers.contains(KeyModifiers::ALT),
        shift: ev.modifiers.contains(KeyModifiers::SHIFT),
    };
    let reply = tokio::task::block_in_place(|| {
        runtime.block_on(async {
            for hook in gray_plugin::input_key_hooks() {
                if let Some(reply) = hook.input_key(&press).await {
                    return Some(reply);
                }
            }
            None
        })
    })?;
    Some(apply(tui, reply))
}

fn apply(tui: &mut Tui, reply: KeyReply) -> KeyOutcome {
    if let Some(action) = reply.action {
        let action = match action {
            KeyAction::HistoryPrevious => Action::HistoryPrevious,
            KeyAction::HistoryNext => Action::HistoryNext,
        };
        if let Some(out) = keys::apply(tui, action) {
            return out;
        }
    }
    let retyped = reply
        .text
        .as_ref()
        .is_some_and(|text| text != tui.textarea.text());
    if let Some(text) = reply.text.as_ref().filter(|_| retyped) {
        tui.textarea.set_text(text);
    }
    let moved = reply.cursor.is_some();
    if let Some(cursor) = reply.cursor {
        tui.textarea.set_cursor(cursor);
    }
    if retyped || moved {
        super::sync_attachment_markers(tui);
        tui.history_idx = None;
        tui.sel = 0;
    }
    if reply.clear_badge {
        tui.input_badge = None;
    } else if let Some(badge) = reply.badge {
        tui.input_badge = Some(badge);
    }
    KeyOutcome::Edited
}
