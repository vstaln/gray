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
