use super::{
    EnterAction, ManagerItem, ManagerSpec, enter_action, enter_label, format_error_row,
    format_plugin_row_parts,
};
use crate::skills::Skill;
use gray_pkg::errors::ErrorEntry;
use gray_pkg::ops::LockEntry;

fn format_plugin_row(name: &str, entry: &LockEntry) -> String {
    format_plugin_row_parts(
        name,
        &entry.version,
        &entry.scope,
        &entry.ecosystem,
        entry.enabled,
    )
}

fn discovered(name: &str, description: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: description.to_string(),
        file_path: std::path::PathBuf::from("/tmp/skills")
            .join(name)
            .join("SKILL.md"),
        base_dir: std::path::PathBuf::from("/tmp/skills").join(name),
        disable_model_invocation: false,
        source: "user".to_string(),
        args: Vec::new(),
    }
}

fn entry(ecosystem: &str, enabled: bool) -> LockEntry {
    LockEntry {
        ecosystem: ecosystem.to_string(),
        version: "1.2.3".to_string(),
        scope: "user".to_string(),
        enabled,
        ..LockEntry::default()
    }
}

#[test]
fn manager_specs_differ_only_where_expected() {
    let skills = &super::SKILLS_SPEC;
    assert_eq!(skills.title, "Skills");
    assert_eq!(skills.empty_hint, "no skills discovered");
    assert_eq!(skills.error_verb, "remove failed");
    assert!(!skills.supports_toggle);
    assert!(skills.enter_invokes);
    assert!(skills.supports_remove);
    assert!(skills.errors_tab);
    assert!(!skills.keep_stale_on_relist_error);

    let plugins = &super::PLUGINS_SPEC;
    assert_eq!(plugins.title, "Plugins");
    assert_eq!(
        plugins.empty_hint,
        "no plugins installed — /plugin install <spec>"
    );
    assert_eq!(plugins.error_verb, "toggle failed");
    assert!(plugins.supports_toggle);
    assert!(!plugins.enter_invokes);
    assert!(plugins.supports_remove);
    assert!(plugins.errors_tab);
    assert!(plugins.keep_stale_on_relist_error);
}

#[test]
fn only_plugin_wrapper_adds_marketplace() {
    assert_eq!(
        super::manager_tab_names(true, false),
        vec!["Installed", "Errors"]
    );
    assert_eq!(
        super::manager_tab_names(true, true),
        vec!["Installed", "Marketplace", "Errors"]
    );
    assert_eq!(super::manager_tab_names(false, false), vec!["Installed"]);
}

#[test]
fn plugin_arrow_keys_cycle_tabs_without_changing_other_manager_tabs() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let names = super::manager_tab_names(true, true);
    let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
    let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(super::manager_tab_for_key(0, &names, right, true), Some(1));
    assert_eq!(super::manager_tab_for_key(1, &names, right, true), Some(2));
    assert_eq!(super::manager_tab_for_key(2, &names, right, true), Some(0));
    assert_eq!(super::manager_tab_for_key(0, &names, left, true), Some(2));
    assert_eq!(super::manager_panel_tab(&names, 2), super::Tab::Errors);
    assert_eq!(super::manager_panel_tab(&names, 1), super::Tab::Installed);
    let shared = super::manager_tab_names(true, false);
    assert_eq!(shared, ["Installed", "Errors"]);
    assert_eq!(super::manager_tab_for_key(0, &shared, right, false), None);
    for (code, modifiers, expected) in [
        (KeyCode::Tab, KeyModifiers::NONE, 1),
        (KeyCode::BackTab, KeyModifiers::SHIFT, 2),
        (KeyCode::Char('n'), KeyModifiers::CONTROL, 1),
        (KeyCode::Char('p'), KeyModifiers::CONTROL, 2),
    ] {
        assert_eq!(
            super::manager_tab_for_key(0, &names, KeyEvent::new(code, modifiers), true),
            Some(expected)
        );
    }
}

#[test]
fn row_shows_name_and_description() {
    assert_eq!(
        crate::skills::format_discovered_skill_row(&discovered("demo-skill", "Do demo things")),
        "demo-skill — Do demo things"
    );
}

#[test]
fn row_falls_back_to_name_without_description() {
    assert_eq!(
        crate::skills::format_discovered_skill_row(&discovered("plain", "")),
        "plain"
    );
}

#[test]
fn row_caps_long_descriptions() {
    let long = "x".repeat(500);
    let row = crate::skills::format_discovered_skill_row(&discovered("big", &long));
    assert!(row.starts_with("big — "), "row: {row:?}");
    assert!(row.chars().count() <= "big — ".len() + 100, "row: {row:?}");
}

#[test]
fn error_row_holds_for_varied_entries() {
    for (source, item, message) in [
        ("skills", "demo", "boom"),
        ("index", "some-plugin", "fetch failed: 404"),
        ("registry", "", ""),
    ] {
        let row = format_error_row(&ErrorEntry {
            ts_secs: 0,
            source: source.to_string(),
            item: item.to_string(),
            message: message.to_string(),
        });
        assert_eq!(row, format!("{source} {item}: {message}"));
    }
}

#[test]
fn unknown_ecosystem_uses_raw_string() {
    let row = format_plugin_row("demo", &entry("url", true));
    assert!(row.contains("[url]"), "raw ecosystem: {row:?}");
}

#[test]
fn enabled_row_exact_string() {
    assert_eq!(
        format_plugin_row("demo", &entry("gray-native", true)),
        "✓ demo 1.2.3 (user) [Gray Index]"
    );
}

#[test]
fn disabled_row_exact_string() {
    assert_eq!(
        format_plugin_row("demo", &entry("gray-native", false)),
        "○ demo 1.2.3 (user) [Gray Index] [disabled]"
    );
}

#[test]
fn cli_ecosystem_labels_plugin_commands() {
    assert_eq!(
        format_plugin_row("demo", &entry("gray-cli", true)),
        "✓ demo 1.2.3 (user) [Plugin command]"
    );

    assert_eq!(
        format_plugin_row_parts("demo", "1.2.3", "user", "gray-cli", true),
        format_plugin_row("demo", &entry("gray-cli", true))
    );
    assert_eq!(
        format_plugin_row_parts("demo", "1.2.3", "user", "gray-cli", false),
        format_plugin_row("demo", &entry("gray-cli", false))
    );
}

fn setup_row(needs_setup: bool) -> ManagerItem {
    ManagerItem {
        name: "discord".to_string(),
        row: "discord — needs setup".to_string(),
        lit: false,
        enabled: true,
        read_only: false,
        needs_setup,
    }
}

fn toggle_spec() -> ManagerSpec {
    ManagerSpec {
        title: "Connections",
        empty_hint: "none",
        error_verb: "toggle failed",
        supports_toggle: true,
        enter_invokes: false,
        supports_remove: false,
        errors_tab: false,
        keep_stale_on_relist_error: true,
    }
}

#[test]
fn enter_routes_to_setup_only_for_flagged_rows_with_a_flow() {
    let spec = toggle_spec();
    assert!(matches!(
        enter_action(Some(&setup_row(true)), true, &spec),
        EnterAction::Setup
    ));

    assert!(matches!(
        enter_action(Some(&setup_row(true)), false, &spec),
        EnterAction::Toggle
    ));

    assert!(matches!(
        enter_action(Some(&setup_row(false)), true, &spec),
        EnterAction::Toggle
    ));
}

#[test]
fn enter_ignores_rows_that_never_carried_an_action() {
    let spec = toggle_spec();
    let separator = ManagerItem {
        read_only: true,
        ..setup_row(false)
    };
    assert!(matches!(
        enter_action(Some(&separator), true, &spec),
        EnterAction::Ignore
    ));
    assert!(matches!(
        enter_action(None, true, &spec),
        EnterAction::Ignore
    ));

    let remove_only = ManagerSpec {
        supports_toggle: false,
        supports_remove: true,
        ..toggle_spec()
    };
    assert!(matches!(
        enter_action(Some(&setup_row(false)), false, &remove_only),
        EnterAction::Ignore
    ));
}

#[test]
fn enter_invokes_skill_rows() {
    let spec = &super::SKILLS_SPEC;
    let item = ManagerItem {
        name: "demo".to_string(),
        row: "demo — Do demo things".to_string(),
        lit: true,
        enabled: true,
        read_only: false,
        needs_setup: false,
    };
    assert!(matches!(
        enter_action(Some(&item), false, spec),
        EnterAction::Invoke
    ));
    assert_eq!(enter_label(Some(&item), false, spec), "invoke \u{b7} ");
}

#[test]
fn enter_label_announces_the_setup_flow() {
    let spec = toggle_spec();
    assert_eq!(
        enter_label(Some(&setup_row(true)), true, &spec),
        "set up \u{b7} "
    );
    assert_eq!(
        enter_label(Some(&setup_row(false)), true, &spec),
        "toggle \u{b7} "
    );
    assert_eq!(enter_label(None, true, &spec), "");
}
