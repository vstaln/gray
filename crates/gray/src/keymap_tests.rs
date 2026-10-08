use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn ev(code: KeyCode, m: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, m)
}

fn chord(s: &str) -> Chord {
    Chord::parse(s).unwrap()
}

#[test]
fn every_default_key_parses_and_ids_round_trip() {
    let km = Keymap::defaults();
    for &a in ALL {
        assert_eq!(Action::from_id(a.id()), Some(a));
        assert_eq!(km.keys_for(a).len(), a.defaults().len());
    }
}

#[test]
fn parse_pi_syntax() {
    let c = chord("ctrl+shift+x");
    assert!(c.ctrl && c.shift && !c.alt);
    assert_eq!(c.code, ChordKey::Char('x'));
    assert_eq!(chord("alt+enter").code, ChordKey::Enter);
    assert_eq!(chord("pageUp").code, ChordKey::PageUp);
    assert_eq!(chord("esc"), chord("escape"));
    assert_eq!(chord("f5").code, ChordKey::F(5));
    assert_eq!(chord("space").code, ChordKey::Char(' '));
    assert_eq!(chord("ctrl++").code, ChordKey::Char('+'));
    assert!(chord("ctrl++").ctrl);
    assert_eq!(chord("super+k").code, ChordKey::Char('k'));
    assert!(chord("super+k").sup);
    assert_eq!(chord("ctrl+x").display(), "ctrl+x");
    assert_eq!(chord("shift+ctrl+x").display(), "ctrl+shift+x");
}

#[test]
fn parse_rejects_garbage() {
    assert!(Chord::parse("").is_err());
    assert!(Chord::parse("hyper+x").is_err());
    assert!(Chord::parse("ctrl+banana").is_err());
    assert!(Chord::parse("f99").is_err());
}

#[test]
fn shifted_symbols_and_capitals_normalize() {
    // Terminals report Shift+A as 'A' (sometimes with SHIFT set too).
    let a = Chord::from_event(&ev(KeyCode::Char('A'), KeyModifiers::NONE)).unwrap();
    assert_eq!(a, chord("shift+a"));
    let a2 = Chord::from_event(&ev(KeyCode::Char('A'), KeyModifiers::SHIFT)).unwrap();
    assert_eq!(a2, chord("shift+a"));
    // '?' arrives with SHIFT on some terminals: the symbol already says it.
    let q = Chord::from_event(&ev(KeyCode::Char('?'), KeyModifiers::SHIFT)).unwrap();
    assert_eq!(q, chord("?"));
    let bt = Chord::from_event(&ev(KeyCode::BackTab, KeyModifiers::SHIFT)).unwrap();
    assert_eq!(bt, chord("shift+tab"));
    let cv = Chord::from_event(&ev(KeyCode::Char('V'), KeyModifiers::CONTROL)).unwrap();
    assert_eq!(cv, chord("ctrl+shift+v"));
}

#[test]
fn resolve_lists_context_candidates_in_priority_order() {
    let km = Keymap::defaults();
    // Up: popup selection first, cursor/history second.
    assert_eq!(
        km.resolve(&chord("up")),
        vec![
            Binding::Action(Action::SelectUp),
            Binding::Action(Action::CursorUp)
        ]
    );
    // Ctrl+D: exit on an empty draft before deleting forward.
    assert_eq!(
        km.resolve(&chord("ctrl+d")),
        vec![
            Binding::Action(Action::Exit),
            Binding::Action(Action::DeleteCharForward)
        ]
    );
    // Esc: close the popup before interrupting.
    assert_eq!(
        km.resolve(&chord("escape")),
        vec![
            Binding::Action(Action::SelectCancel),
            Binding::Action(Action::Interrupt)
        ]
    );
    assert!(km.resolve(&chord("ctrl+shift+q")).is_empty());
}

#[test]
fn user_file_replaces_defaults_and_unbinds() {
    let km = Keymap::from_json(
        r#"{ "tui.editor.cursorWordLeft": "ctrl+y",
             "app.exit": [] }"#,
    );
    assert!(km.warnings.is_empty(), "{:?}", km.warnings);
    assert_eq!(km.keys_for(Action::CursorWordLeft), &[chord("ctrl+y")]);
    assert!(km.keys_for(Action::Exit).is_empty());
    // Defaults gone for the overridden action.
    assert!(
        !km.resolve(&chord("alt+b"))
            .contains(&Binding::Action(Action::CursorWordLeft))
    );
    assert_eq!(km.overridden.len(), 2);
}

#[test]
fn slash_command_bindings_win() {
    let km = Keymap::from_json(r#"{ "/compact": ["ctrl+shift+k", "f2"] }"#);
    assert_eq!(
        km.resolve(&chord("f2")),
        vec![Binding::Command("/compact".into())]
    );
    assert_eq!(km.commands().len(), 2);
}

#[test]
fn ctrl_c_cannot_be_unbound() {
    let km = Keymap::from_json(r#"{ "app.clear": ["ctrl+x"] }"#);
    assert!(km.keys_for(Action::Clear).contains(&chord("ctrl+c")));
    assert!(km.keys_for(Action::Clear).contains(&chord("ctrl+x")));
}

#[test]
fn pi_only_ids_are_silent_unknown_ids_and_bad_keys_warn() {
    let km = Keymap::from_json(
        r#"{ "app.session.tree": "ctrl+shift+t",
             "editor.bogus": "x",
             "tui.input.submit": ["enter", "ctrl+banana"] }"#,
    );
    assert_eq!(km.warnings.len(), 2, "{:?}", km.warnings);
    assert!(km.warnings.iter().any(|w| w.contains("editor.bogus")));
    assert!(km.warnings.iter().any(|w| w.contains("ctrl+banana")));
    // The good key of a partly bad list still binds.
    assert_eq!(km.keys_for(Action::Submit), &[chord("enter")]);
}

#[test]
fn invalid_json_keeps_defaults() {
    let km = Keymap::from_json("{ nope");
    assert_eq!(km.warnings.len(), 1);
    assert_eq!(km.keys_for(Action::Submit), &[chord("enter")]);
}

#[test]
fn load_in_reads_file_or_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let km = load_in(dir.path());
    assert!(km.source.is_none() && km.warnings.is_empty());
    std::fs::write(path_in(dir.path()), r#"{"tui.input.newLine":"ctrl+o"}"#).unwrap();
    let km = load_in(dir.path());
    assert_eq!(km.source.as_deref(), Some(path_in(dir.path()).as_path()));
    assert_eq!(km.keys_for(Action::NewLine), &[chord("ctrl+o")]);
}
