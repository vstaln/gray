use super::*;

fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let themes = root.path().join("themes");
    let config = root.path().join("config.json");
    (root, themes, config)
}

#[test]
fn list_marks_the_active_theme() {
    let (_root, themes, config) = setup();
    std::fs::create_dir_all(&themes).unwrap();
    std::fs::write(themes.join("dusk.json"), r#"{"colors":{}}"#).unwrap();
    let out = theme_command(None, Some("dusk"), &themes, &config);
    assert!(out.apply.is_none());
    assert!(out.message.contains("● dusk"), "{}", out.message);
    assert!(out.message.contains("  gray (built-in)"), "{}", out.message);
}

#[test]
fn switching_applies_and_persists() {
    let (_root, themes, config) = setup();
    std::fs::create_dir_all(&themes).unwrap();
    std::fs::write(themes.join("dusk.json"), r#"{"colors":{"accent":"red"}}"#).unwrap();
    let out = theme_command(Some("dusk"), None, &themes, &config);
    let (t, name) = out.apply.expect("applies");
    assert_eq!(t.accent, ratatui::style::Color::Red);
    assert_eq!(name.as_deref(), Some("dusk"));
    assert_eq!(
        crate::setup::load_saved_config_at(&config).theme.as_deref(),
        Some("dusk")
    );
    let back = theme_command(Some("gray"), Some("dusk"), &themes, &config);
    assert_eq!(back.apply.unwrap().0, theme::GRAY_UI_THEME);
    assert!(crate::setup::load_saved_config_at(&config).theme.is_none());
}

#[test]
fn missing_theme_changes_nothing() {
    let (_root, themes, config) = setup();
    let out = theme_command(Some("nope"), None, &themes, &config);
    assert!(out.apply.is_none());
    assert!(out.message.starts_with("theme not applied"));
    assert!(!config.exists());
}

#[test]
fn new_writes_an_editable_full_palette_once() {
    let (_root, themes, config) = setup();
    let out = theme_command(Some("new mine"), None, &themes, &config);
    assert!(out.apply.is_none());
    let text = std::fs::read_to_string(themes.join("mine.json")).unwrap();
    let (parsed, warnings) = theme::parse_theme(&text).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(parsed, theme::theme());
    let again = theme_command(Some("new mine"), None, &themes, &config);
    assert!(again.message.contains("already exists"));
    assert!(
        theme_command(Some("new gray"), None, &themes, &config)
            .message
            .contains("can't be")
    );
}

#[test]
fn hotkeys_lists_actions_and_marks_overrides() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        crate::keymap::path_in(dir.path()),
        r#"{"tui.input.newLine":"ctrl+o","/compact":"f2"}"#,
    )
    .unwrap();
    let km = crate::keymap::load_in(dir.path());
    let (msg, apply) = keys_command(None, dir.path(), &km);
    assert!(apply.is_none());
    assert!(msg.contains("tui.editor.cursorWordLeft"));
    assert!(
        msg.lines()
            .any(|l| l.contains("tui.input.newLine") && l.contains('*'))
    );
    assert!(msg.contains("f2") && msg.contains("/compact"));
}

#[test]
fn hotkeys_reload_returns_new_keymap_with_warnings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(crate::keymap::path_in(dir.path()), r#"{"nope.x":"f3"}"#).unwrap();
    let (msg, apply) = keys_command(
        Some("reload"),
        dir.path(),
        &crate::keymap::Keymap::defaults(),
    );
    assert!(apply.is_some());
    assert!(msg.contains("reloaded") && msg.contains("unknown action `nope.x`"));
    let (usage, none) = keys_command(
        Some("bogus"),
        dir.path(),
        &crate::keymap::Keymap::defaults(),
    );
    assert!(none.is_none() && usage.starts_with("usage: /hotkeys"));
}
