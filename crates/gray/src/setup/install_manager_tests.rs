use super::{format_error_row, format_plugin_row};
use crate::skills::Skill;
use gray_pkg::errors::ErrorEntry;
use gray_pkg::ops::LockEntry;

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
    // Contract pin: the two managers share one loop; any new divergence
    // must update this test deliberately.
    let skills = &super::SKILLS_SPEC;
    assert_eq!(skills.title, "Skills");
    assert_eq!(skills.empty_hint, "no skills discovered");
    assert_eq!(skills.error_verb, "remove failed");
    assert!(!skills.supports_toggle);
    assert!(!skills.keep_stale_on_relist_error);

    let plugins = &super::PLUGINS_SPEC;
    assert_eq!(plugins.title, "Plugins");
    assert_eq!(
        plugins.empty_hint,
        "no plugins installed — /plugin install <spec>"
    );
    assert_eq!(plugins.error_verb, "toggle failed");
    assert!(plugins.supports_toggle);
    assert!(plugins.keep_stale_on_relist_error);
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
fn error_row_matches_plugins_format() {
    let row = crate::skills::format_discovered_skill_row(&discovered("x", "Does x things"));
    assert_eq!(row, "x — Does x things");
    let err = format_error_row(&ErrorEntry {
        ts_secs: 0,
        source: "skills".to_string(),
        item: "demo".to_string(),
        message: "boom".to_string(),
    });
    assert_eq!(err, "skills demo: boom");
}

#[test]
fn error_row_holds_for_varied_entries() {
    // The old skills modal delegated to the plugins formatter; the
    // single shared renderer must keep parity for any input.
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
fn enabled_row_shows_check_and_source_label() {
    let row = format_plugin_row("demo", &entry("gray-native", true));
    assert!(row.starts_with("✓ "), "enabled marker: {row:?}");
    assert!(row.contains("demo 1.2.3 (user)"), "body: {row:?}");
    assert!(row.contains("[Gray Index]"), "source label: {row:?}");
    assert!(!row.contains("[disabled]"), "no dim marker: {row:?}");
}

#[test]
fn disabled_row_shows_circle_and_disabled_marker() {
    let row = format_plugin_row("demo", &entry("pi-gallery", false));
    assert!(row.starts_with("○ "), "disabled marker: {row:?}");
    assert!(row.contains("[disabled]"), "dim marker text: {row:?}");
    assert!(
        row.contains("[Pi Gallery (preview)]"),
        "source label: {row:?}"
    );
}

#[test]
fn unknown_ecosystem_uses_raw_string() {
    let row = format_plugin_row("demo", &entry("url", true));
    assert!(row.contains("[url]"), "raw ecosystem: {row:?}");
}

#[test]
fn error_row_shows_source_item_and_message() {
    let row = format_error_row(&ErrorEntry {
        ts_secs: 0,
        source: "index".to_string(),
        item: "demo".to_string(),
        message: "boom".to_string(),
    });
    assert_eq!(row, "index demo: boom");
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
        format_plugin_row("demo", &entry("pi-gallery", false)),
        "○ demo 1.2.3 (user) [Pi Gallery (preview)] [disabled]"
    );
}
