use super::*;

fn row(name: &str, on: bool) -> crate::plugin_cli::ManagedRow {
    crate::plugin_cli::ManagedRow {
        name: name.to_string(),
        version: "0.2.0".to_string(),
        scope: "user".to_string(),
        ecosystem: "gray-native".to_string(),
        on,
        cli: true,
    }
}

#[test]
fn app_row_shows_plugin_shape_and_toggle_state() {
    let items = app_rows_with(&[row("myapp", true)], None);
    assert_eq!(items.len(), 1);
    assert!(
        items[0].row.starts_with("\u{2713} myapp 0.2.0 (user)"),
        "{}",
        items[0].row
    );
    assert!(items[0].enabled);
    assert!(items[0].lit);
    assert!(!items[0].read_only);
    assert!(!items[0].needs_setup);

    let off = app_rows_with(&[row("myapp", false)], None);
    assert!(off[0].row.starts_with("\u{25cb}"), "{}", off[0].row);
    assert!(off[0].row.contains("[disabled]"), "{}", off[0].row);
    assert!(!off[0].enabled);
}

#[test]
fn app_rows_list_declared_subcommands_from_the_cached_manifest() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("plugins")).unwrap();
    std::fs::write(
        home.path().join("plugins/myapp-manifest.json"),
        r#"{"name":"myapp","completion":["settings","run"]}"#,
    )
    .unwrap();
    let items = app_rows_with(&[row("myapp", true)], Some(home.path()));
    assert!(items[0].row.contains("settings"), "{}", items[0].row);
    assert!(items[0].row.contains("run"), "{}", items[0].row);
}

#[test]
fn an_app_declaring_setup_gets_the_setup_hint() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("plugins")).unwrap();
    std::fs::write(
        home.path().join("plugins/myapp-manifest.json"),
        r#"{"name":"myapp","completion":["setup","run"]}"#,
    )
    .unwrap();
    let items = app_rows_with(&[row("myapp", true)], Some(home.path()));
    assert!(
        items[0].row.contains("gray myapp setup"),
        "{}",
        items[0].row
    );
}

#[test]
fn an_app_without_setup_gets_no_hint_and_no_invented_commands() {
    let home = tempfile::tempdir().unwrap();
    let items = app_rows_with(&[row("slack", true)], Some(home.path()));
    assert_eq!(items[0].row, "\u{2713} slack 0.2.0 (user) [Gray Index]");
    assert!(!items[0].row.contains("setup"));
}

#[test]
fn the_panel_is_apps_only() {
    // The gateway picker lists the apps and nothing else: daemon/cron/memory
    // own their commands (`gray gateway status`, `/cron`, `/memory`) and this
    // panel narrates none of them — no rule, no pointer lines.
    let home = tempfile::tempdir().unwrap();
    let rows = [row("myapp", true)];
    let items = app_rows_with(&rows, Some(home.path()));
    assert_eq!(items.len(), 1);
    assert!(items[0].row.contains("myapp"));

    // With no apps there are no rows either; the hint is the spec's only
    // fallback, and nothing decorative rides alongside it.
    assert!(app_rows_with(&[], Some(home.path())).is_empty());
    assert!(GATEWAY_SPEC.empty_hint.contains("no apps installed"));
    assert!(GATEWAY_SPEC.empty_hint.contains("gray plugin install"));
}
