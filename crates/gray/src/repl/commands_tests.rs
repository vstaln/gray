use super::{ReplCommand, parse_command};
#[test]
fn slash_name_with_slash_is_plain_prompt_like_codex() {
    // Reported bug: pasting Rust `///` doc comments said "unknown command".
    let pasted = "/// Default system prompt, shipped as markdown and materialized to `~/.gray/sys.md`\n/// on first run.";
    assert!(matches!(parse_command(pasted), ReplCommand::Prompt(_)));
    assert!(matches!(
        parse_command("// comment"),
        ReplCommand::Prompt(_)
    ));
    assert!(matches!(parse_command("/tmp/foo"), ReplCommand::Prompt(_)));
    assert!(matches!(parse_command("/"), ReplCommand::Prompt(_)));
    // Genuinely unknown single-token commands still error.
    assert!(matches!(
        parse_command("/boguscmd"),
        ReplCommand::Unknown(_)
    ));
    // Known commands unaffected.
    assert!(matches!(parse_command("/help"), ReplCommand::Help));
    assert!(matches!(parse_command("/model foo"), ReplCommand::Model(_)));
}

#[test]
fn context_renamed_no_window_alias() {
    assert!(matches!(
        parse_command("/context"),
        ReplCommand::ContextWindow(None)
    ));
    assert!(matches!(
        parse_command("/context 128k"),
        ReplCommand::ContextWindow(Some(_))
    ));
    assert!(matches!(
        parse_command("/context-window"),
        ReplCommand::Unknown(_)
    ));
}

#[test]
fn usage_command_and_cost_alias() {
    assert!(matches!(parse_command("/usage"), ReplCommand::Usage));
    assert!(matches!(parse_command("/copy"), ReplCommand::Copy));
    assert!(matches!(parse_command("/doctor"), ReplCommand::Unknown(_)));
    assert!(matches!(parse_command("/cost"), ReplCommand::Usage));
    use std::path::Path;
    let cwd = Path::new(".");
    assert!(
        super::completion_matches_dyn("/us", cwd)
            .iter()
            .any(|(n, _)| n == "usage")
    );
    // `cost` resolves through the alias table
    assert!(
        super::completion_matches("cost")
            .iter()
            .any(|(n, _)| *n == "usage")
    );
}

#[test]
fn thinking_effort_and_reasoning_aliases() {
    assert!(matches!(
        parse_command("/thinking"),
        ReplCommand::Thinking(None)
    ));
    assert!(matches!(
        parse_command("/effort"),
        ReplCommand::Thinking(None)
    ));
    assert!(matches!(
        parse_command("/reasoning"),
        ReplCommand::Thinking(None)
    ));
    assert!(matches!(
        parse_command("/reasoning max"),
        ReplCommand::Thinking(Some(_))
    ));
    // `reasoning` resolves through the alias table
    assert!(
        super::completion_matches("reasoning")
            .iter()
            .any(|(n, _)| *n == "thinking")
    );
}

#[test]
fn empty_prompt_hides_slash_popup_like_codex() {
    // codex `command_under_cursor`: empty text / no leading slash / cursor
    // past the command name → no popup. Deleting `/` must close it, not
    // strand stale matches (ghost popup + double footer + scrollback growth).
    use std::path::Path;
    let cwd = Path::new(".");
    assert!(super::completion_matches_dyn("", cwd).is_empty());
    assert!(super::completion_matches_dyn("hello", cwd).is_empty());
    assert!(super::completion_matches_dyn("/ ", cwd).is_empty());
    // bare `/` opens the popup with every command.
    assert_eq!(
        super::completion_matches_dyn("/", cwd).len(),
        super::REGISTRY.len()
    );
}

#[test]
fn registry_resolve_canonical_and_aliases() {
    for name in [
        "connect", "model", "thinking", "context", "resume", "new", "compact", "usage", "feedback",
        "agentsmd", "skills", "plugin", "help", "quit",
    ] {
        let d = super::resolve(name).unwrap_or_else(|| panic!("resolve {name}"));
        assert_eq!(d.name, name);
        assert_eq!(super::resolve(&format!("/{name}")).unwrap().name, name);
        assert_eq!(super::resolve(&name.to_uppercase()).unwrap().name, name);
    }
    for (alias, target) in [
        ("clear", "new"),
        ("reset", "new"),
        ("exit", "quit"),
        ("keys", "connect"),
        ("key", "connect"),
        ("providers", "connect"),
        ("provider", "connect"),
        ("login", "login"),
        ("whoami", "whoami"),
        ("logout", "logout"),
        ("effort", "thinking"),
        ("reasoning", "thinking"),
        ("compress", "compact"),
        ("sys", "agentsmd"),
        ("cost", "usage"),
        ("plugins", "plugin"),
    ] {
        assert_eq!(super::resolve(alias).unwrap().name, target, "alias {alias}");
        assert_eq!(super::resolve(&format!("/{alias}")).unwrap().name, target);
    }
    assert!(super::resolve("yolo").is_none());
    assert!(super::resolve("/yolo").is_none());
    assert!(super::resolve("boguscmd").is_none());
    assert!(super::resolve("/boguscmd").is_none());
    assert!(super::resolve("").is_none());
    assert!(super::resolve("/").is_none());
}

#[test]
fn registry_completion_covers_aliases() {
    for (alias, target) in [
        ("clear", "new"),
        ("reset", "new"),
        ("exit", "quit"),
        ("keys", "connect"),
        ("key", "connect"),
        ("providers", "connect"),
        ("provider", "connect"),
        ("login", "login"),
        ("whoami", "whoami"),
        ("logout", "logout"),
        ("effort", "thinking"),
        ("reasoning", "thinking"),
        ("compress", "compact"),
        ("sys", "agentsmd"),
        ("cost", "usage"),
        ("plugins", "plugin"),
    ] {
        assert!(
            super::completion_matches(alias)
                .iter()
                .any(|(n, _)| *n == target),
            "completion {alias} -> {target}"
        );
    }
    // `/plug` surfaces `plugin`; bare `/plugin ` leads with itself + all 7 subcommands.
    use std::path::Path;
    let cwd = Path::new(".");
    assert!(
        super::completion_matches_dyn("/plug", cwd)
            .iter()
            .any(|(n, _)| n == "plugin")
    );
    let plugin_all = super::complete_command_args("plugin", "", cwd);
    assert_eq!(plugin_all.len(), 8);
    assert_eq!(plugin_all[0].0, "plugin");
}

#[test]
fn registry_parse_uses_canonical() {
    assert!(matches!(parse_command("/cost"), ReplCommand::Usage));
    assert!(matches!(parse_command("/COST"), ReplCommand::Usage));
    assert!(matches!(
        parse_command("/plugin list"),
        ReplCommand::Plugin(_)
    ));
    assert!(matches!(
        parse_command("/plugins list"),
        ReplCommand::Plugin(_)
    ));
    assert!(matches!(
        parse_command("/PLUGIN list"),
        ReplCommand::Plugin(_)
    ));
    assert!(matches!(
        parse_command("/marketplace"),
        ReplCommand::Unknown(_)
    ));
    assert!(matches!(parse_command("/exit"), ReplCommand::Quit));
    // gateway left the TUI: /gateway and /gw are unknown (the deleted
    // native gateway no longer provides any chat surface).
    assert!(matches!(parse_command("/gw"), ReplCommand::Unknown(_)));
    assert!(matches!(
        parse_command("/gateway status"),
        ReplCommand::Unknown(_)
    ));
    assert!(matches!(parse_command("/keys foo"), ReplCommand::Provider));
    assert!(matches!(
        parse_command("/connect foo"),
        ReplCommand::Provider
    ));
    assert!(matches!(parse_command("/key foo"), ReplCommand::Provider));
    assert!(matches!(
        parse_command("/provider openrouter"),
        ReplCommand::Provider
    ));
    assert!(matches!(
        parse_command("/login openrouter"),
        ReplCommand::Login(Some(code)) if code == "openrouter"
    ));
    assert!(matches!(parse_command("/login"), ReplCommand::Login(None)));
    assert!(matches!(parse_command("/whoami"), ReplCommand::Whoami));
    assert!(matches!(parse_command("/logout"), ReplCommand::Logout));
    assert!(matches!(
        parse_command("/skills foo"),
        ReplCommand::Skill(Some(_))
    ));
    assert!(matches!(
        parse_command("/skill foo"),
        ReplCommand::Skill(Some(_))
    ));
    assert!(matches!(parse_command("/skills"), ReplCommand::Skill(None)));
    assert!(matches!(parse_command("/skill"), ReplCommand::Skill(None)));
    // The `/skills:<name>` colon form is gone: now an unknown command.
    assert!(matches!(
        parse_command("/skills:commit"),
        ReplCommand::Unknown(_)
    ));
}

#[test]
fn permission_commands_are_gone() {
    for cmd in [
        "/permissions",
        "/permissions full",
        "/perms read-only",
        "/access",
        "/access full",
        "/yolo",
    ] {
        assert!(
            matches!(parse_command(cmd), ReplCommand::Unknown(_)),
            "{cmd} must be unknown"
        );
    }
}

#[test]
fn feedback_parses_with_and_without_text() {
    assert!(matches!(
        parse_command("/feedback"),
        ReplCommand::Feedback(None)
    ));
    assert!(matches!(
        parse_command("/feedback broken x"),
        ReplCommand::Feedback(Some(_))
    ));
    assert!(matches!(
        parse_command("/FEEDBACK hi"),
        ReplCommand::Feedback(Some(_))
    ));
}

#[test]
fn context_arg_completion_levels() {
    use super::{complete_command_args, completion_matches_dyn};
    use std::path::Path;
    let cwd = Path::new(".");
    // bare suffix lists everything, led by the command itself
    let all = complete_command_args("context", "", cwd);
    assert_eq!(all[0].0, "context");
    assert!(all.iter().any(|(n, _)| n == "context reserve"));
    assert!(all.iter().any(|(n, _)| n == "context auto"));
    // filtered L1 and L2 pages list suffixes only (bare row would wipe args on fill)
    let r = complete_command_args("context", "r", cwd);
    assert!(!r.iter().any(|(n, _)| n == "context"));
    assert!(r.iter().any(|(n, _)| n == "context reserve"));
    // L2 after `reserve `
    let r2 = complete_command_args("context", "reserve ", cwd);
    assert!(!r2.iter().any(|(n, _)| n == "context"));
    assert!(r2.iter().any(|(n, _)| n == "context reserve 16k"));
    // unknown command has no suffixes (universal hook default)
    assert!(complete_command_args("boguscmd", "", cwd).is_empty());
    // dyn dispatch through the composer entry point
    let dyn_all = completion_matches_dyn("/context ", cwd);
    assert_eq!(dyn_all[0].0, "context");
    assert!(dyn_all.iter().any(|(n, _)| n == "context reserve"));
    // command-name path unaffected
    assert!(
        completion_matches_dyn("/cont", cwd)
            .iter()
            .any(|(n, _)| n == "context")
    );
}

fn temp_skill_cwd(name: &str) -> tempfile::TempDir {
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
fn top_level_query_never_surfaces_skills() {
    use super::completion_matches_dyn;
    let dir = temp_skill_cwd("commit");
    let cwd = dir.path();
    // sanity: the skill is discoverable
    assert!(
        crate::skills::discover_skills(cwd)
            .skills
            .iter()
            .any(|s| s.name == "commit")
    );
    // top-level `/` completion must not surface it…
    let top = completion_matches_dyn("/com", cwd);
    assert!(
        !top.iter().any(|(n, _)| n == "commit"),
        "skill must not appear in / completion: {top:?}"
    );
    // …but `/skills ` still completes it
    let scoped = completion_matches_dyn("/skills com", cwd);
    assert!(
        scoped.iter().any(|(n, _)| n == "skills commit"),
        "skill must complete under /skills : {scoped:?}"
    );
}

#[test]
fn thinking_effort_arg_completion() {
    use super::complete_command_args;
    use std::path::Path;
    let cwd = Path::new(".");
    for cmd in ["thinking", "effort", "reasoning"] {
        let all = complete_command_args(cmd, "", cwd);
        for level in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
            assert!(
                all.iter().any(|(n, _)| n == &format!("{cmd} {level}")),
                "{cmd} must complete level {level}: {all:?}"
            );
        }
        let f = complete_command_args(cmd, "hi", cwd);
        assert!(f.iter().any(|(n, _)| n == &format!("{cmd} high")));
        assert!(f.iter().any(|(n, _)| n == &format!("{cmd} xhigh")));
    }
}

#[test]
fn resume_and_agentsmd_arg_completion() {
    use super::complete_command_args;
    use std::path::Path;
    let cwd = Path::new(".");
    let r = complete_command_args("resume", "", cwd);
    assert!(r.iter().any(|(n, _)| n == "resume --last"));
    assert!(r.iter().any(|(n, _)| n == "resume --all"));
    for cmd in ["agentsmd", "sys"] {
        let a = complete_command_args(cmd, "", cwd);
        assert!(a.iter().any(|(n, _)| n == &format!("{cmd} show")));
        assert!(a.iter().any(|(n, _)| n == &format!("{cmd} reset")));
    }
}

#[test]
fn model_completes_cached_ids() {
    use super::complete_command_args;
    use std::path::Path;
    let cwd = Path::new(".");
    // Discovery, not the global context/pricing cache, supplies completions.
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", server.local_addr().unwrap());
    let response = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut stream, _) = server.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        stream.read(&mut request).unwrap();
        let body = r#"{"data":[{"id":"test-completion-model-xyz"}]}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    let runtime = tokio::runtime::Runtime::new().unwrap();
    crate::setup::set_active_model_provider(&base);
    runtime.block_on(async {
        let models = crate::setup::fetch_live_provider_models(&base, None);
        assert_eq!(models.len(), 1);
    });
    response.join().unwrap();
    let rows = complete_command_args("model", "", cwd);
    assert!(
        rows.iter()
            .any(|(n, _)| n == "model test-completion-model-xyz"),
        "cached model id must complete: {rows:?}"
    );
    let filtered = complete_command_args("model", "xyz", cwd);
    assert!(
        filtered
            .iter()
            .any(|(n, _)| n == "model test-completion-model-xyz")
    );
    // an impossible filter still yields nothing (deterministic even
    // when other tests pollute the process-global cache)
    assert!(complete_command_args("model", "no-such-model-xyz-123", cwd).is_empty());
    // Switching to an unfetched/offline provider clears the visible list.
    crate::setup::set_active_model_provider("http://127.0.0.1:1/v1");
    assert!(complete_command_args("model", "xyz", cwd).is_empty());
    // Trailing slashes do not create a different connection scope.
    crate::setup::set_active_model_provider(&format!("{base}/"));
    assert!(
        complete_command_args("models", "xyz", cwd)
            .iter()
            .any(|(n, _)| n == "models test-completion-model-xyz")
    );
    crate::setup::set_active_model_provider("");
}

#[test]
fn skill_singular_is_alias_for_skills_space() {
    use super::super::handlers::expand_skill_command;
    use super::completion_matches_dyn;
    let dir = temp_skill_cwd("commit");
    let cwd = dir.path();
    // Parse parity: identical payloads.
    assert_eq!(
        parse_command("/skill commit"),
        parse_command("/skills commit")
    );
    assert_eq!(
        parse_command("/skill commit extra"),
        parse_command("/skills commit extra")
    );
    assert_eq!(parse_command("/skill"), parse_command("/skills"));
    assert_eq!(parse_command("/SKILL"), parse_command("/skills"));
    assert_eq!(
        parse_command("/SKILL commit"),
        parse_command("/skills commit")
    );
    // Expansion parity: same Prompt out.
    let a = expand_skill_command(parse_command("/skill commit"), cwd, None, false);
    let b = expand_skill_command(parse_command("/skills commit"), cwd, None, false);
    assert!(matches!(a, ReplCommand::Prompt(_)));
    assert_eq!(a, b);
    // Bad args fail identically (skill takes no args): both expand to Empty.
    let a = expand_skill_command(parse_command("/skill commit bogus-arg"), cwd, None, false);
    let b = expand_skill_command(parse_command("/skills commit bogus-arg"), cwd, None, false);
    assert_eq!(a, ReplCommand::Empty);
    assert_eq!(a, b);
    // Unknown skill fails identically.
    let a = expand_skill_command(parse_command("/skill nope"), cwd, None, false);
    let b = expand_skill_command(parse_command("/skills nope"), cwd, None, false);
    assert_eq!(a, ReplCommand::Empty);
    assert_eq!(a, b);
    // `/skill <partial>` completes installed skill names…
    let rows = completion_matches_dyn("/skill com", cwd);
    assert!(
        rows.iter().any(|(n, _)| n == "skill commit"),
        "skill must complete under /skill : {rows:?}"
    );
    let rows = completion_matches_dyn("/skill ", cwd);
    assert!(rows.iter().any(|(n, _)| n == "skill commit"));
    // …and so does the plural space form.
    let rows = completion_matches_dyn("/skills com", cwd);
    assert!(
        rows.iter().any(|(n, _)| n == "skills commit"),
        "skill must complete under /skills : {rows:?}"
    );
    let rows = completion_matches_dyn("/skills ", cwd);
    assert!(rows.iter().any(|(n, _)| n == "skills commit"));
    // The colon form is gone.
    assert!(matches!(
        parse_command("/skills:commit"),
        ReplCommand::Unknown(_)
    ));
}

#[test]
fn cron_parses_bare_and_single_arg() {
    assert!(matches!(
        parse_command("/cron"),
        ReplCommand::CronJobs(None)
    ));
    let ReplCommand::CronJobs(Some(id)) = parse_command("/cron abc123") else {
        panic!("expected CronJobs(Some)");
    };
    assert_eq!(id, "abc123");
}

#[test]
fn models_alias_opens_picker_and_preserves_direct_argument() {
    assert!(matches!(parse_command("/models"), ReplCommand::Model(None)));
    assert!(
        matches!(parse_command("/models deepseek-v4.1-flash"), ReplCommand::Model(Some(id)) if id == "deepseek-v4.1-flash")
    );
    assert!(matches!(
        parse_command("/modelxyz"),
        ReplCommand::Unknown(_)
    ));
}

#[test]
fn model_completion_does_not_offer_global_context_catalog() {
    crate::setup::cache_model_context("unconnected-provider/unique-foreign-model", 128000);
    let rows =
        super::complete_command_args("model", "unique-foreign-model", std::path::Path::new("."));
    assert!(rows.is_empty(), "unconnected models leaked: {rows:?}");
}
