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
    crate::project_trust::assume_trusted_for_test();

    crate::project_trust::assume_trusted_for_test();
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

    let msg = apply_skills_auto_toggle(&cfg, true).unwrap();
    assert!(msg.contains("on"), "{msg}");
    assert!(crate::setup::skills_auto_enabled_at(&cfg));
    assert!(
        crate::setup::load_saved_config_at(&cfg)
            .disabled_skills
            .is_empty()
    );

    std::fs::write(&cfg, r#"{"model":"m"}"#).unwrap();
    assert!(crate::setup::skills_auto_enabled_at(&cfg));
}

#[test]
fn skills_named_on_off_still_invoke_instead_of_toggling_global() {
    crate::project_trust::assume_trusted_for_test();

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

    let err = apply_skill_toggle(&cfg, &discovered, false, "nope").unwrap_err();
    assert!(err.contains("nope"), "{err}");
    assert!(!cfg.exists());

    let msg = apply_skill_toggle(&cfg, &discovered, false, "demo").unwrap();
    assert!(msg.contains("disabled"), "{msg}");
    assert!(
        crate::setup::load_saved_config_at(&cfg)
            .disabled_skills
            .contains("demo")
    );

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
    let text = format_skill_paste("Do things.", "ts", Some("fast"));
    assert!(!text.contains("<skill"), "{text}");
    assert!(!text.contains("</skill>"), "{text}");
    assert!(text.contains("Do things."), "{text}");
    assert!(text.contains("**ARGUMENTS:** fast"), "{text}");

    assert!(text.contains("\"ts\" skill"), "skill not named: {text}");
    assert!(text.contains("binding"), "no binding directive: {text}");
    assert!(text.contains("mid-task"), "no mid-task re-anchor: {text}");
    let bare = format_skill_paste("Do things.", "ts", None);
    assert!(!bare.contains("ARGUMENTS"), "{bare}");
    assert!(!bare.contains("<skill"), "{bare}");
}

#[tokio::test]
async fn reload_agent_failure_preserves_agent() {
    let config = Config {
        fast_mode: None,
        model_parts: Default::default(),
        temperature: None,
        top_p: None,
        model: None,
        base_url: String::new(),
        api_key: None,
        provider_id: String::new(),
        credential_source: String::new(),
        auth_ref: String::new(),
        thinking_effort: None,
        show_reasoning: None,
        context_window: None,
        context_reserve: None,
        context_keep: None,
        exec_prefix: None,
        max_turns: None,
        max_cost_micros: None,
        max_wall_secs: None,
        bare: false,
        lean: false,
        compact_at: None,
    };
    let mut agent = Some(reload_test_agent().with_messages(vec![
        Message::user("keep my question"),
        Message::assistant("keep my answer"),
    ]));
    let files = tempfile::tempdir().unwrap();
    let installed = files.path().join("installed-plugin");
    std::fs::write(&installed, "installed files").unwrap();
    let outcome = reload_agent(
        &mut agent,
        &config,
        std::path::Path::new("/tmp"),
        None,
        None,
    )
    .await;
    assert!(!outcome.rebuilt);
    let agent = agent.unwrap();
    assert_eq!(agent.messages().len(), 2);
    assert_eq!(agent.messages()[0], Message::user("keep my question"));
    assert_eq!(
        std::fs::read_to_string(installed).unwrap(),
        "installed files"
    );
}

fn reload_test_agent() -> Agent {
    struct NoRequests;
    impl gray_core::agent::Provider for NoRequests {
        fn stream(&self, _: gray_core::message::ChatRequest) -> gray_core::agent::ProviderStream {
            panic!("reload must not call a provider")
        }
    }
    Agent::new(
        Box::new(NoRequests),
        Arc::new(gray_tools::Registry::default()),
    )
}

#[tokio::test]
async fn reload_agent_success_keeps_history_and_session_and_rebuilds_once() {
    let mut agent = Some(reload_test_agent().with_messages(vec![Message::user("history")]));
    let mut builds = 0;
    let outcome =
        super::handlers::reload_agent_with(&mut agent, Some("existing-session"), None, |session| {
            builds += 1;
            assert_eq!(session, Some("existing-session"));
            async { Ok(reload_test_agent()) }
        })
        .await;
    assert!(outcome.rebuilt);
    assert_eq!(builds, 1);
    assert_eq!(agent.unwrap().messages(), &[Message::user("history")]);
}

#[tokio::test]
async fn reload_agent_failed_plugin_spawn_warns_and_keeps_installation() {
    // Run the real builder in a subprocess so HOME and its warning queues cannot
    // affect parallel tests or the user's installed plugins.
    const CHILD: &str = "GRAY_ACTIVATION_SPAWN_TEST";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(home.join("plugins/broken")).unwrap();
        std::fs::write(home.join("plugins/broken/kept.txt"), "installed").unwrap();
        std::fs::write(
            home.join("plugins/lock.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": 1,
                "plugins": {
                    "broken": {
                        "ecosystem": "gray-native", "enabled": true,
                        "version": "1.0.0", "hash": "", "source": "fixture",
                        "adapter_version": "", "installed_at": "", "scope": "user",
                        "argv": [home.join("plugins/broken/missing")]
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "repl::handlers::tests::reload_agent_failed_plugin_spawn_warns_and_keeps_installation", "--nocapture"])
            .env(CHILD, "1")
            .env("GRAY_HOME", &home)
            .env("GRAY_USER_HOME", &home)
            .env("HOME", root.path())
            .env_remove("GRAY_PLUGIN_PATH")
            .current_dir(root.path())
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(home.join("plugins/broken/kept.txt")).unwrap(),
            "installed"
        );
        return;
    }
    use clap::Parser;
    let cli = crate::Cli::try_parse_from([
        "gray",
        "--model",
        "offline-test",
        "--base-url",
        "http://127.0.0.1:1/v1",
    ])
    .unwrap();
    let config = Config::resolve_with(&cli, |_| None).unwrap();
    let mut agent = Some(reload_test_agent().with_messages(vec![Message::user("history")]));
    let outcome = reload_agent(
        &mut agent,
        &config,
        &std::env::current_dir().unwrap(),
        Some("existing-session"),
        None,
    )
    .await;
    assert!(outcome.rebuilt);
    assert_eq!(agent.unwrap().messages(), &[Message::user("history")]);
    let warnings = crate::take_profile_warnings();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("broken") && w.contains("failed to spawn")),
        "{warnings:?}"
    );
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

    assert_eq!(parse_on_off("off now"), None);
    assert_eq!(parse_on_off("memory"), None);
    assert_eq!(parse_on_off("on/off"), None);

    for sub in [Subsystem::Gateway, Subsystem::Intro] {
        assert!(parse_on_off("off").is_some());
        assert!(!sub.label().is_empty());
    }
}

#[test]
fn subsystem_toggle_persists_per_subsystem_and_reports_the_manual_path() {
    use super::{Subsystem, apply_subsystem_toggle};
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");

    let msg = apply_subsystem_toggle(&cfg, Subsystem::Gateway, false).unwrap();
    assert!(msg.contains("status/stop still work"), "{msg}");
    assert!(!crate::setup::gw_auto_enabled_at(&cfg));

    let msg = apply_subsystem_toggle(&cfg, Subsystem::Gateway, true).unwrap();
    assert!(msg.contains("gateway on"), "{msg}");
    assert!(crate::setup::gw_auto_enabled_at(&cfg));

    let missing = dir.path().join("nope.json");
    assert!(crate::setup::gw_auto_enabled_at(&missing));
}

#[test]
fn undo_cut_lands_on_the_last_user_turn() {
    let messages = vec![
        Message::user("first"),
        Message::assistant("one"),
        Message::user("second"),
        Message::assistant("two"),
    ];
    assert_eq!(undo_cut(&messages), Some(2));
}

#[test]
fn undo_cut_reports_a_conversation_it_cannot_rewind() {
    assert_eq!(undo_cut(&[]), None);
    assert_eq!(undo_cut(&[Message::assistant("unprompted")]), None);
    assert_eq!(undo_cut(&[Message::user("only")]), Some(0));
}
