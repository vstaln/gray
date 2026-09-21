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
    let items = app_rows_with(&[row("discord", true)], None);
    assert_eq!(items.len(), 1);
    assert!(
        items[0].row.starts_with("\u{2713} discord 0.2.0 (user)"),
        "{}",
        items[0].row
    );
    assert!(items[0].enabled);
    assert!(items[0].lit);
    assert!(!items[0].read_only);

    let off = app_rows_with(&[row("discord", false)], None);
    assert!(off[0].row.starts_with("\u{25cb}"), "{}", off[0].row);
    assert!(off[0].row.contains("[disabled]"), "{}", off[0].row);
    assert!(!off[0].enabled);
}

#[test]
fn missing_config_marks_the_app_as_needing_setup() {
    let home = tempfile::tempdir().unwrap();
    let items = app_rows_with(&[row("discord", true)], Some(home.path()));
    assert!(
        items[0]
            .row
            .contains("needs setup \u{2014} gray discord setup"),
        "{}",
        items[0].row
    );

    // An existing config file is enough: the contents are never read.
    let cfg = home.path().join(".config/gray-discord");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("config.json"), "{}").unwrap();
    let items = app_rows_with(&[row("discord", true)], Some(home.path()));
    assert!(!items[0].row.contains("needs setup"), "{}", items[0].row);
}

#[test]
fn unknown_apps_get_no_setup_probe_and_no_invented_commands() {
    let home = tempfile::tempdir().unwrap();
    let items = app_rows_with(&[row("slack", true)], Some(home.path()));
    assert_eq!(items[0].row, "\u{2713} slack 0.2.0 (user) [Gray Index]");
}

#[test]
fn items_end_with_a_rule_and_the_command_pointers() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".config/gray-discord")).unwrap();
    std::fs::write(home.path().join(".config/gray-discord/config.json"), "{}").unwrap();
    let rows = [row("discord", true)];
    let mut items = app_rows_with(&rows, Some(home.path()));
    items.push(separator());
    for (label, detail) in POINTERS {
        items.push(ManagerItem {
            name: String::new(),
            row: format!("{label} \u{2014} {detail}"),
            lit: false,
            enabled: false,
            read_only: true,
        });
    }
    // Apps first, then the break, then the pointers — none of them toggleable.
    assert!(items[0].row.contains("discord"));
    assert!(items[1].row.chars().all(|c| c == '\u{2500}'));
    assert!(items[1].read_only);
    let labels: Vec<&str> = items[2..].iter().map(|i| i.row.as_str()).collect();
    assert!(
        labels[0].starts_with("daemon \u{2014} gray gateway status"),
        "{labels:?}"
    );
    assert!(labels[1].starts_with("cron \u{2014} /cron"));
    assert!(labels[2].starts_with("memory \u{2014} /memory"));
    assert!(items[2..].iter().all(|i| i.read_only));
}

#[test]
fn spec_is_a_toggle_listing_without_removal_or_errors() {
    assert_eq!(GATEWAY_SPEC.title, "Connections");
    const {
        assert!(GATEWAY_SPEC.supports_toggle);
    }
    // Removal belongs to /plugin; package errors belong to /plugin too.
    const {
        assert!(!GATEWAY_SPEC.supports_remove);
    }
    const {
        assert!(!GATEWAY_SPEC.errors_tab);
    }
}
