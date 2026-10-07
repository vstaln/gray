use super::*;

#[test]
fn every_role_name_maps_to_a_distinct_field() {
    // All fields are `Color`, so the struct size counts them: a new role
    // without a ROLE_NAMES / role_mut entry fails here.
    let fields = std::mem::size_of::<UiTheme>() / std::mem::size_of::<Color>();
    assert_eq!(ROLE_NAMES.len(), fields);
    for (i, name) in ROLE_NAMES.iter().enumerate() {
        let mut t = GRAY_UI_THEME;
        *t.role_mut(name).unwrap_or_else(|| panic!("{name}")) = Color::Indexed(i as u8);
        let changed = ROLE_NAMES
            .iter()
            .filter(|n| t.role(n) != GRAY_UI_THEME.role(n))
            .count();
        assert_eq!(changed, 1, "{name} must touch exactly one field");
    }
}

#[test]
fn partial_theme_overrides_only_named_roles() {
    let (t, warnings) =
        parse_theme(r##"{"colors":{"accent":"#ff0000","surface_bg":"default"}}"##).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(t.accent, Color::Rgb(255, 0, 0));
    assert_eq!(t.surface_bg, Color::Reset);
    assert_eq!(t.text_body, GRAY_UI_THEME.text_body);
}

#[test]
fn colors_accept_vars_names_indexes_and_short_hex() {
    let (t, warnings) = parse_theme(
        r##"{
          "vars": {"peach": "#f6ad7e", "alias": "peach"},
          "colors": {
            "accent": "alias",
            "info": "lightblue",
            "error": 196,
            "success": "34",
            "rose": "#fab"
          }
        }"##,
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(t.accent, Color::Rgb(246, 173, 126));
    assert_eq!(t.info, Color::LightBlue);
    assert_eq!(t.error, Color::Indexed(196));
    assert_eq!(t.success, Color::Indexed(34));
    assert_eq!(t.rose, Color::Rgb(0xff, 0xaa, 0xbb));
}

#[test]
fn bad_entries_warn_but_the_rest_applies() {
    let (t, warnings) =
        parse_theme(r#"{"colors":{"accnet":"red","info":"blurple","accent":"red"}}"#).unwrap();
    assert_eq!(t.accent, Color::Red);
    assert_eq!(t.info, GRAY_UI_THEME.info);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
}

#[test]
fn var_cycles_are_reported_not_hung() {
    let (_, warnings) =
        parse_theme(r#"{"vars":{"a":"b","b":"a"},"colors":{"accent":"a"}}"#).unwrap();
    assert_eq!(warnings.len(), 1);
}

#[test]
fn structural_errors_fail() {
    assert!(parse_theme("[]").is_err());
    assert!(parse_theme("{}").is_err());
    assert!(parse_theme(r#"{"colors":[]}"#).is_err());
    assert!(parse_theme("{").is_err());
}

#[test]
fn exported_palette_roundtrips() {
    let json = theme_to_json(&GRAY_UI_THEME);
    let (t, warnings) = parse_theme(&json).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(t, GRAY_UI_THEME);
}

#[test]
fn theme_files_are_listed_and_loaded_by_stem() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("b.json"), r#"{"colors":{}}"#).unwrap();
    std::fs::write(dir.path().join("a.json"), r#"{"colors":{"accent":"red"}}"#).unwrap();
    std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
    assert_eq!(list_themes_in(dir.path()), vec!["a", "b"]);
    assert_eq!(load_theme_in(dir.path(), "a").unwrap().0.accent, Color::Red);
    assert!(load_theme_in(dir.path(), "missing").is_err());
    assert!(load_theme_in(dir.path(), "../a").is_err());
}

#[test]
fn builtin_names() {
    assert!(is_builtin_name("gray"));
    assert!(is_builtin_name("Default"));
    assert!(!is_builtin_name("grayish"));
}
