use super::*;

fn temp_skill_cwd_for_handlers_test(name: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join(".gray").join("skills").join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: Temp skill for completion tests\n---\n# temp\n",
    )
    .unwrap();
    dir
}

#[test]
fn skill_paste_is_what_the_model_gets() {
    // The visible paste and the model turn must be the same string:
    // what you see in chat is what the model gets.
    let dir = temp_skill_cwd_for_handlers_test("paste-me");
    let cwd = dir.path();
    let out = expand_skill_command(parse_command("/skills paste-me"), cwd, None, false);
    let ReplCommand::Prompt(expanded) = out else {
        panic!("expected Prompt, got {out:?}");
    };
    assert!(!expanded.contains("<skill"), "envelope leaked: {expanded}");
    assert!(
        !expanded.contains("</skill>"),
        "envelope leaked: {expanded}"
    );
    assert!(expanded.contains("# temp"), "body missing: {expanded}");
    // Args ride along in the same text.
    let dir2 = temp_skill_cwd_for_handlers_test("paste-args");
    let skill_dir = dir2.path().join(".gray").join("skills").join("paste-args");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: Temp skill with args\nargs: env\n---\n# temp $ARGUMENTS\n",
    )
    .unwrap();
    let out = expand_skill_command(
        parse_command("/skills paste-args env"),
        dir2.path(),
        None,
        false,
    );
    let ReplCommand::Prompt(expanded) = out else {
        panic!("expected Prompt, got {out:?}");
    };
    assert!(
        expanded.contains("**ARGUMENTS:** env"),
        "args missing: {expanded}"
    );
}

fn test_skill_entry(name: &str) -> crate::skills::Skill {
    crate::skills::Skill {
        name: name.to_string(),
        description: "test skill".to_string(),
        file_path: std::path::PathBuf::from("/tmp/SKILL.md"),
        base_dir: std::path::PathBuf::from("/tmp"),
        disable_model_invocation: false,
        source: "path".to_string(),
        args: vec![],
    }
}

#[test]
fn skill_toggle_verb_parses_enable_disable() {
    assert_eq!(
        parse_skill_toggle("disable foo"),
        Some((false, "foo".to_string()))
    );
    assert_eq!(
        parse_skill_toggle("enable foo"),
        Some((true, "foo".to_string()))
    );
    assert_eq!(
        parse_skill_toggle("DISABLE foo"),
        Some((false, "foo".to_string()))
    );
    assert_eq!(parse_skill_toggle("foo"), None);
    assert_eq!(parse_skill_toggle("foo bar"), None);
    assert_eq!(parse_skill_toggle(""), None);
    assert_eq!(parse_skill_toggle("enable"), None);
    assert_eq!(parse_skill_toggle("disable"), None);
    assert_eq!(parse_skill_toggle("on"), None);
    assert_eq!(parse_skill_toggle("off"), None);
}

#[test]
fn skills_auto_toggle_parses_bare_switch_words() {
    assert_eq!(parse_skills_auto_toggle("on"), Some(true));
    assert_eq!(parse_skills_auto_toggle("off"), Some(false));
    assert_eq!(parse_skills_auto_toggle("ON"), Some(true));
    assert_eq!(parse_skills_auto_toggle("OFF"), Some(false));
    assert_eq!(parse_skills_auto_toggle("enable"), Some(true));
    assert_eq!(parse_skills_auto_toggle("disable"), Some(false));
    assert_eq!(parse_skills_auto_toggle(""), None);
    assert_eq!(parse_skills_auto_toggle("foo"), None);
    assert_eq!(parse_skills_auto_toggle("on extra"), None);
    assert_eq!(parse_skills_auto_toggle("enable foo"), None);
    assert_eq!(parse_skills_auto_toggle("disable foo"), None);
}

#[test]
fn skills_auto_toggle_persists_and_on_clears_disabled_set() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");
    assert!(crate::setup::skills_auto_enabled_at(&cfg));
    // Off persists as an explicit false; per-skill disables survive it.
    let mut saved = crate::setup::load_saved_config_at(&cfg);
    saved.disabled_skills.insert("demo".to_string());
    crate::setup::save_saved_config_at(&cfg, &saved).unwrap();
    let msg = apply_skills_auto_toggle(&cfg, false).unwrap();
    assert!(msg.contains("off"), "{msg}");
    assert!(!crate::setup::skills_auto_enabled_at(&cfg));
    assert!(
        crate::setup::load_saved_config_at(&cfg)
            .disabled_skills
            .contains("demo")
    );
    // On clears the flag (back to missing = default) and the disabled set.
    let msg = apply_skills_auto_toggle(&cfg, true).unwrap();
    assert!(msg.contains("on"), "{msg}");
    assert!(crate::setup::skills_auto_enabled_at(&cfg));
    assert!(
        crate::setup::load_saved_config_at(&cfg)
            .disabled_skills
            .is_empty()
    );
    // Missing key reads as enabled.
    std::fs::write(&cfg, r#"{"model":"m"}"#).unwrap();
    assert!(crate::setup::skills_auto_enabled_at(&cfg));
}

#[test]
fn skills_named_on_off_still_invoke_instead_of_toggling_global() {
    // A skill literally named `on`/`enable` stays invokable: the global
    // branch yields to an exact discovery hit.
    let dir = temp_skill_cwd_for_handlers_test("on");
    let out = expand_skill_command(parse_command("/skills on"), dir.path(), None, false);
    assert!(
        matches!(out, ReplCommand::Prompt(_)),
        "skill named 'on' must invoke, got {out:?}"
    );
}

#[test]
fn skill_toggle_persists_and_validates_against_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");
    let discovered = vec![test_skill_entry("demo")];
    // Unknown name errors and stores nothing.
    let err = apply_skill_toggle(&cfg, &discovered, false, "nope").unwrap_err();
    assert!(err.contains("nope"), "{err}");
    assert!(!cfg.exists());
    // Disable persists.
    let msg = apply_skill_toggle(&cfg, &discovered, false, "demo").unwrap();
    assert!(msg.contains("disabled"), "{msg}");
    assert!(
        crate::setup::load_saved_config_at(&cfg)
            .disabled_skills
            .contains("demo")
    );
    // Enable removes.
    let msg = apply_skill_toggle(&cfg, &discovered, true, "demo").unwrap();
    assert!(msg.contains("enabled"), "{msg}");
    assert!(
        crate::setup::load_saved_config_at(&cfg)
            .disabled_skills
            .is_empty()
    );
}

#[test]
fn format_skill_paste_body_and_args() {
    let text = format_skill_paste("Do things.", Some("fast"));
    assert!(!text.contains("<skill"), "{text}");
    assert!(!text.contains("</skill>"), "{text}");
    assert!(text.contains("Do things."), "{text}");
    assert!(text.contains("**ARGUMENTS:** fast"), "{text}");
    let bare = format_skill_paste("Do things.", None);
    assert!(!bare.contains("ARGUMENTS"), "{bare}");
    assert!(!bare.contains("<skill"), "{bare}");
}

// UNRUN (cargo test banned under X): run in TTY/CI.
// reload_agent with no model configured fails soft through say()
// (headless println path) and preserves the previous agent.
#[tokio::test]
async fn reload_agent_failure_preserves_agent() {
    let config = Config {
        temperature: None,
        top_p: None,
        model: None,
        base_url: String::new(),
        api_key: None,
        thinking_effort: None,
        show_reasoning: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
    };
    let mut agent: Option<Agent> = None;
    reload_agent(
        &mut agent,
        &config,
        std::path::Path::new("/tmp"),
        None,
        None,
    )
    .await;
    assert!(agent.is_none());
}

#[test]
fn subsystem_toggle_parses_bare_switch_words_only() {
    use super::{Subsystem, parse_on_off};
    assert_eq!(parse_on_off("on"), Some(true));
    assert_eq!(parse_on_off("off"), Some(false));
    assert_eq!(parse_on_off("ON"), Some(true));
    assert_eq!(parse_on_off("Disable"), Some(false));
    assert_eq!(parse_on_off(""), None);
    assert_eq!(parse_on_off("  off  "), Some(false));
    // Extras and names are not switches — each command keeps its own shape.
    assert_eq!(parse_on_off("off now"), None);
    assert_eq!(parse_on_off("memory"), None);
    assert_eq!(parse_on_off("on/off"), None);
    // All three subsystems share the parser.
    for sub in [Subsystem::Memory, Subsystem::Cron, Subsystem::Gateway] {
        assert!(parse_on_off("off").is_some());
        assert_eq!(sub.label().is_empty(), false);
    }
}

#[test]
fn subsystem_toggle_persists_per_subsystem_and_reports_the_manual_path() {
    use super::{Subsystem, apply_subsystem_toggle};
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");
    // Off persists as an explicit false and names the manual path.
    let msg = apply_subsystem_toggle(&cfg, Subsystem::Memory, false).unwrap();
    assert!(msg.contains("memory off"), "{msg}");
    assert!(msg.contains("gray memory still saves"), "{msg}");
    assert!(!crate::setup::memory_auto_enabled_at(&cfg));
    // The other two switches are independent.
    assert!(crate::setup::cron_auto_enabled_at(&cfg));
    assert!(crate::setup::gw_auto_enabled_at(&cfg));
    let msg = apply_subsystem_toggle(&cfg, Subsystem::Cron, false).unwrap();
    assert!(msg.contains("/cron still runs them"), "{msg}");
    let msg = apply_subsystem_toggle(&cfg, Subsystem::Gateway, false).unwrap();
    assert!(msg.contains("status/stop still work"), "{msg}");
    assert!(!crate::setup::cron_auto_enabled_at(&cfg));
    assert!(!crate::setup::gw_auto_enabled_at(&cfg));
    // Memory stays off — flipping cron did not touch it.
    assert!(!crate::setup::memory_auto_enabled_at(&cfg));
    // On clears the flag (back to missing = default on).
    let msg = apply_subsystem_toggle(&cfg, Subsystem::Memory, true).unwrap();
    assert!(msg.contains("memory on"), "{msg}");
    assert!(crate::setup::memory_auto_enabled_at(&cfg));
    // A missing config reads as enabled for all three.
    let missing = dir.path().join("nope.json");
    assert!(crate::setup::memory_auto_enabled_at(&missing));
    assert!(crate::setup::cron_auto_enabled_at(&missing));
    assert!(crate::setup::gw_auto_enabled_at(&missing));
}
