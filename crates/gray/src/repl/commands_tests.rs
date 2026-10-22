use super::{ReplCommand, parse_command};
#[test]
fn continue_payload_routes_to_the_model() {
    assert!(matches!(super::parse_command(""), ReplCommand::Empty));
    assert!(matches!(
        super::parse_command(super::super::CONTINUE_PROMPT),
        ReplCommand::Prompt(_)
    ));
}

#[test]
fn slash_name_with_slash_is_plain_prompt_like_codex() {
    let pasted = "/// Default system prompt, shipped as markdown and materialized to `~/.gray/sys.md`\n/// on first run.";
    assert!(matches!(parse_command(pasted), ReplCommand::Prompt(_)));
    assert!(matches!(
        parse_command("// comment"),
        ReplCommand::Prompt(_)
    ));
    assert!(matches!(parse_command("/tmp/foo"), ReplCommand::Prompt(_)));
    assert!(matches!(parse_command("/"), ReplCommand::Prompt(_)));

    assert!(matches!(
        parse_command("/boguscmd"),
        ReplCommand::Unknown(_)
    ));

    assert!(matches!(parse_command("/help"), ReplCommand::Help));
    assert!(matches!(
        parse_command("/models foo"),
        ReplCommand::Model(_)
    ));
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
fn moved_commands_parse_as_unknown_for_plugin_routing() {
    for name in [
        "/usage", "/copy", "/cost", "/theme", "/hotkeys", "/intro", "/gateway", "/gw",
    ] {
        assert!(
            matches!(parse_command(name), ReplCommand::Unknown(_)),
            "{name}"
        );
    }
    assert!(matches!(parse_command("/doctor"), ReplCommand::Unknown(_)));
    assert!(matches!(parse_command("/session"), ReplCommand::Unknown(_)));
}

#[test]
fn empty_prompt_hides_slash_popup_like_codex() {
    use std::path::Path;
    let cwd = Path::new(".");
    assert!(super::completion_matches_dyn("", cwd).is_empty());
    assert!(super::completion_matches_dyn("hello", cwd).is_empty());
    assert!(super::completion_matches_dyn("/ ", cwd).is_empty());

    let dyn_matches = super::completion_matches_dyn("/", cwd);
    for d in super::REGISTRY {
        assert!(
            dyn_matches.iter().any(|(name, _)| name == d.name),
            "missing /{} in popup",
            d.name
        );
    }
    assert!(dyn_matches.len() >= super::REGISTRY.len());
}

#[test]
fn registry_resolve_canonical_and_aliases() {
    for name in [
        "models", "context", "sessions", "new", "compact", "hehe", "agentsmd", "skills", "plugins",
        "help", "quit",
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
        ("compress", "compact"),
        ("sys", "agentsmd"),
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
        ("compress", "compact"),
        ("sys", "agentsmd"),
    ] {
        assert!(
            super::completion_matches(alias)
                .iter()
                .any(|(n, _)| *n == target),
            "completion {alias} -> {target}"
        );
    }

    use std::path::Path;
    let cwd = Path::new(".");
    assert!(
        super::completion_matches_dyn("/plug", cwd)
            .iter()
            .any(|(n, _)| n == "plugins")
    );
    let plugin_all = super::complete_command_args("plugin", "", cwd);
    assert_eq!(plugin_all.len(), 8);
    assert_eq!(plugin_all[0].0, "plugin");
}

#[test]
fn registry_parse_uses_canonical() {
    assert!(matches!(
        parse_command("/plugin list"),
        ReplCommand::Plugin(_)
    ));
    assert!(matches!(parse_command("/plugins"), ReplCommand::Plugins));
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

    for gone in ["/keys foo", "/key foo", "/browse", "/fast"] {
        assert!(
            matches!(parse_command(gone), ReplCommand::Unknown(_)),
            "{gone}"
        );
    }
    assert!(matches!(
        parse_command("/login openrouter"),
        ReplCommand::Unknown(_)
    ));
    assert!(matches!(parse_command("/whoami"), ReplCommand::Unknown(_)));
    assert!(matches!(parse_command("/logout"), ReplCommand::Unknown(_)));
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
fn context_arg_completion_levels() {
    use super::{complete_command_args, completion_matches_dyn};
    use std::path::Path;
    let cwd = Path::new(".");

    let all = complete_command_args("context", "", cwd);
    assert_eq!(all[0].0, "context");
    assert!(all.iter().any(|(n, _)| n == "context reserve"));
    assert!(all.iter().any(|(n, _)| n == "context auto"));

    let r = complete_command_args("context", "r", cwd);
    assert!(!r.iter().any(|(n, _)| n == "context"));
    assert!(r.iter().any(|(n, _)| n == "context reserve"));

    let r2 = complete_command_args("context", "reserve ", cwd);
    assert!(!r2.iter().any(|(n, _)| n == "context"));
    assert!(r2.iter().any(|(n, _)| n == "context reserve 16k"));

    assert!(complete_command_args("boguscmd", "", cwd).is_empty());

    let dyn_all = completion_matches_dyn("/context ", cwd);
    assert_eq!(dyn_all[0].0, "context");
    assert!(dyn_all.iter().any(|(n, _)| n == "context reserve"));

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
    crate::project_trust::assume_trusted_for_test();
    use super::completion_matches_dyn;
    let dir = temp_skill_cwd("commit");
    let cwd = dir.path();

    assert!(
        crate::skills::discover_skills(cwd)
            .skills
            .iter()
            .any(|s| s.name == "commit")
    );

    let top = completion_matches_dyn("/com", cwd);
    assert!(
        !top.iter().any(|(n, _)| n == "commit"),
        "skill must not appear in / completion: {top:?}"
    );

    let scoped = completion_matches_dyn("/skills com", cwd);
    assert!(
        scoped.iter().any(|(n, _)| n == "skills commit"),
        "skill must complete under /skills : {scoped:?}"
    );
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

    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", server.local_addr().unwrap());
    let response = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut stream, _) = server.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4096];
        let request_len = stream.read(&mut request).unwrap();
        assert!(request_len > 0);
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
    let rows = complete_command_args("models", "", cwd);
    assert!(
        rows.iter()
            .any(|(n, _)| n == "models test-completion-model-xyz"),
        "cached model id must complete: {rows:?}"
    );
    let filtered = complete_command_args("models", "xyz", cwd);
    assert!(
        filtered
            .iter()
            .any(|(n, _)| n == "models test-completion-model-xyz")
    );

    assert!(complete_command_args("model", "no-such-model-xyz-123", cwd).is_empty());

    crate::setup::set_active_model_provider("http://127.0.0.1:1/v1");
    assert!(complete_command_args("models", "xyz", cwd).is_empty());

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
    crate::project_trust::assume_trusted_for_test();
    use super::super::handlers::expand_skill_command;
    use super::completion_matches_dyn;
    let dir = temp_skill_cwd("commit");
    let cwd = dir.path();

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

    let a = expand_skill_command(parse_command("/skill commit"), cwd, None, false);
    let b = expand_skill_command(parse_command("/skills commit"), cwd, None, false);
    assert!(matches!(a, ReplCommand::Prompt(_)));
    assert_eq!(a, b);

    let a = expand_skill_command(parse_command("/skill commit bogus-arg"), cwd, None, false);
    let b = expand_skill_command(parse_command("/skills commit bogus-arg"), cwd, None, false);
    assert!(matches!(a, ReplCommand::Prompt(_)));
    assert_eq!(a, b);

    let a = expand_skill_command(parse_command("/skill nope"), cwd, None, false);
    let b = expand_skill_command(parse_command("/skills nope"), cwd, None, false);
    assert_eq!(a, ReplCommand::Empty);
    assert_eq!(a, b);

    let rows = completion_matches_dyn("/skill com", cwd);
    assert!(
        rows.iter().any(|(n, _)| n == "skill commit"),
        "skill must complete under /skill : {rows:?}"
    );
    let rows = completion_matches_dyn("/skill ", cwd);
    assert!(rows.iter().any(|(n, _)| n == "skill commit"));

    let rows = completion_matches_dyn("/skills com", cwd);
    assert!(
        rows.iter().any(|(n, _)| n == "skills commit"),
        "skill must complete under /skills : {rows:?}"
    );
    let rows = completion_matches_dyn("/skills ", cwd);
    assert!(rows.iter().any(|(n, _)| n == "skills commit"));

    assert!(matches!(
        parse_command("/skills:commit"),
        ReplCommand::Unknown(_)
    ));
}

#[test]
fn bare_model_and_provider_aliases_open_two_pane_picker() {
    assert!(matches!(parse_command("/models"), ReplCommand::ModelPanes));
    assert!(matches!(parse_command("/MODELS"), ReplCommand::ModelPanes));
    for cmd in ["/model", "/connect", "/provider"] {
        assert!(
            matches!(parse_command(cmd), ReplCommand::ModelPanes),
            "{cmd}"
        );
    }
    for cmd in ["/browse", "/providers", "/key", "/effort"] {
        assert!(
            matches!(parse_command(cmd), ReplCommand::Unknown(_)),
            "{cmd}"
        );
    }
    assert!(matches!(parse_command("/browsex"), ReplCommand::Unknown(_)));
}

#[test]
fn models_alias_opens_picker_and_preserves_direct_argument() {
    assert!(matches!(parse_command("/models"), ReplCommand::ModelPanes));
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

#[test]
fn update_and_restart_parse_and_register() {
    for (typed, name) in [
        ("/update", "update"),
        ("/UPDATE", "update"),
        ("/restart", "restart"),
        ("/reboot", "restart"),
    ] {
        let cmd = parse_command(typed);
        let right = match name {
            "update" => matches!(cmd, ReplCommand::Update(_)),
            _ => matches!(cmd, ReplCommand::Restart),
        };
        assert!(right, "{typed} must parse as /{name}");
        let def = super::resolve(typed).unwrap_or_else(|| panic!("{typed} in the registry"));
        let help = super::format_help_line(def);
        assert!(help.contains(name), "help line: {help}");
    }

    assert!(
        super::completion_matches("rest")
            .iter()
            .any(|(n, _)| *n == "restart")
    );
    assert!(
        super::completion_matches("upd")
            .iter()
            .any(|(n, _)| *n == "update")
    );
}

#[test]
fn undo_and_retry_parse_bare() {
    assert_eq!(parse_command("/undo"), ReplCommand::Undo);
    assert_eq!(parse_command("/UNDO"), ReplCommand::Undo);
    assert_eq!(parse_command("/retry"), ReplCommand::Retry);
}

#[test]
fn undo_and_retry_are_in_the_registry() {
    let names: Vec<&str> = super::REGISTRY.iter().map(|c| c.name).collect();
    assert!(names.contains(&"undo"), "registry: {names:?}");
    assert!(names.contains(&"retry"), "registry: {names:?}");
}

#[test]
fn jobs_command_opens_the_background_work_list() {
    assert!(matches!(parse_command("/jobs"), ReplCommand::Jobs));
    assert!(matches!(parse_command("/JOBS"), ReplCommand::Jobs));
}

#[test]
fn resume_dismiss_parses_alone_and_with_all() {
    let a = super::parse_resume_args("dismiss");
    assert!(a.dismiss && !a.all && a.target.is_none());
    let b = super::parse_resume_args("dismiss all");
    assert!(b.dismiss && b.all);
    assert!(!super::parse_resume_args("--last").dismiss);
}

#[test]
fn sessions_is_canonical_and_resume_stays_an_alias() {
    assert_eq!(super::resolve("/sessions").unwrap().name, "sessions");
    assert_eq!(super::resolve("/resume").unwrap().name, "sessions");
    for line in ["/sessions --last", "/resume --last"] {
        match super::parse_command(line) {
            super::ReplCommand::Resume(a) => assert!(a.last, "{line}"),
            _ => panic!("{line} did not parse as the sessions command"),
        }
    }

    let cwd = std::path::Path::new("/tmp");
    assert!(
        super::complete_command_args("sessions", "", cwd)
            .iter()
            .any(|(n, _)| n == "sessions dismiss")
    );
    assert!(
        super::complete_command_args("resume", "", cwd)
            .iter()
            .any(|(n, _)| n == "resume dismiss")
    );
}

#[test]
fn connect_completes_to_models_not_gateway() {
    // "/connect" used to fuzzy-match the old /gateway "connections" description.
    let got = super::completion_matches("connect");
    assert_eq!(got.first().map(|(n, _)| *n), Some("models"), "{got:?}");
}
