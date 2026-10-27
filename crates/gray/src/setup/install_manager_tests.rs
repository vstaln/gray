use super::{EnterAction, ManagerItem, ManagerSpec, enter_action, enter_label, format_error_row};
use crate::skills::Skill;
use gray_pkg::errors::ErrorEntry;

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
