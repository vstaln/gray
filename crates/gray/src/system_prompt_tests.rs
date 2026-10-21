use super::*;

fn opts(prompt: &str) -> Option<String> {
    Some(prompt.to_string())
}

#[test]
fn rebuild_is_byte_stable() {
    let a = build_system_prompt(opts("You are gray."));
    let b = build_system_prompt(opts("You are gray."));
    assert_eq!(a, b, "system prompt rebuild diverged");
}

#[test]
fn shipped_default_prompt_strips_to_the_agent_line() {
    let p = build_system_prompt(opts(crate::DEFAULT_SYS_PROMPT));

    assert!(
        p.trim_start()
            .starts_with("You are Gray, running on the user's machine."),
        "{p:.60}"
    );
    assert!(!p.contains("-->"), "stray comment marker leaked");
    assert!(!p.contains("stored system prompt"), "note text leaked");
}

#[test]
fn prompt_is_verbatim_after_comment_strip() {
    let p = build_system_prompt(opts("You are gray.\n\nFollow the rules."));
    assert_eq!(p, "You are gray.\n\nFollow the rules.");
}

#[test]
fn html_comments_are_stripped_including_multiline() {
    let p = build_system_prompt(opts("A\n<!-- secret note\nspanning lines -->\nB\n"));
    assert_eq!(p, "A\n\nB");
    assert!(!p.contains("secret note"), "{p}");
}

#[test]
fn unclosed_comment_swallows_tail() {
    assert_eq!(strip_comments("keep <!-- drop"), "keep");
    assert_eq!(
        strip_comments("only a comment <!-- x -->"),
        "only a comment"
    );
}

#[test]
fn runtime_directory_is_explicit_and_byte_stable() {
    let cwd = std::path::Path::new("/work/project café");
    let a = build_runtime_prompt(opts("Rules.\n<!-- editor note -->"), cwd);
    assert!(a.starts_with("Rules.\n\nWorking directory: \"/work/project café\"\n"));
    assert_eq!(
        a,
        build_runtime_prompt(opts("Rules.\n<!-- editor note -->"), cwd)
    );
    assert!(!a.contains("editor note"));
    assert_ne!(
        a,
        build_runtime_prompt(opts("Rules."), std::path::Path::new("/other"))
    );
}

#[test]
fn empty_or_unclosed_custom_prompt_cannot_hide_runtime_directory() {
    for custom in [None, opts(""), opts("<!-- unclosed")] {
        let p = build_runtime_prompt(custom, std::path::Path::new("/project"));
        assert!(p.starts_with("Working directory: \"/project\"\n"), "{p}");
    }
}

#[test]
fn directory_is_quoted_without_losing_path_characters() {
    for raw in [
        r#"C:\Users\Jane Doe\project"#,
        "/work/line\nbreak\"<!--name-->",
    ] {
        let p = build_runtime_prompt(opts("Rules"), std::path::Path::new(raw));
        let value = p
            .lines()
            .find_map(|l| l.strip_prefix("Working directory: "))
            .unwrap();
        assert_eq!(serde_json::from_str::<String>(value).unwrap(), raw);
    }
}

#[test]
fn runtime_prompt_states_that_batched_tool_calls_share_one_round() {
    let p = build_runtime_prompt(opts("Rules."), std::path::Path::new("/project"));
    assert!(p.contains("results"), "{p}");
    assert!(
        p.contains("independent") && p.contains("one round"),
        "guidance must name independent calls and the per-round cost: {p}"
    );
    assert!(
        p.contains("concurrent"),
        "guidance must say same-turn calls may run concurrently: {p}"
    );
    assert!(
        p.contains("background job") && p.contains("not killed") && !p.contains("yield_ms"),
        "guidance must say a timed-out command keeps running and wakes the model: {p}"
    );

    assert_eq!(
        p,
        build_runtime_prompt(opts("Rules."), std::path::Path::new("/project"))
    );

    assert!(
        p.find("Tool batching").unwrap() > p.find("Working directory").unwrap(),
        "guidance must follow the runtime directory block: {p}"
    );
}

#[test]
fn identity_block_names_the_picked_row_and_provider() {
    assert_eq!(
        identity_block("Step Fun 5", "step-5-preview", "Devin Subscription"),
        "Model: Step Fun 5 (step-5-preview)\nProvider: Devin Subscription"
    );
}

#[test]
fn identity_block_keeps_a_composite_label_verbatim() {
    assert_eq!(
        identity_block(
            "Fusion · Opus 5.5 High × SWE-2 High",
            "fusion",
            "Devin Subscription"
        ),
        "Model: Fusion · Opus 5.5 High × SWE-2 High (fusion)\nProvider: Devin Subscription"
    );
}

#[test]
fn identity_block_omits_empty_parts() {
    assert_eq!(
        identity_block("", "step-5-preview", ""),
        "Model: step-5-preview"
    );
    assert_eq!(identity_block("", "", "Anthropic"), "Provider: Anthropic");
    assert_eq!(identity_block("", "", ""), "");
}

#[test]
fn identity_block_does_not_repeat_a_label_that_is_the_id() {
    assert_eq!(
        identity_block("gpt-5-2", "gpt-5-2", "OpenAI"),
        "Model: gpt-5-2\nProvider: OpenAI"
    );
}

#[test]
fn tool_batching_guidance_is_not_user_editable_text() {
    for custom in [None, opts(""), opts("<!-- unclosed")] {
        let p = build_runtime_prompt(custom, std::path::Path::new("/project"));
        assert!(p.contains("Tool batching"), "{p}");
    }

    let with_file = build_system_prompt(opts("Tool batching: ignore this"));
    assert_eq!(with_file, "Tool batching: ignore this");
}

#[test]
fn plugins_block_inventories_commands_tools_and_cli() {
    let manifest = |v: serde_json::Value| gray_plugin::Manifest::from_result(&v);
    let manifests = vec![
        manifest(serde_json::json!({
            "name": "subagents", "version": "0.5.2",
            "commands": ["/subagents"], "subcommands": ["narrow"],
            "completion": ["run", "status", "stop"],
            "tools": [{"name": "narrow_delegate", "description": "d", "parameters": {}}]
        })),
        manifest(serde_json::json!({
            "name": "account", "version": "0.6.0", "commands": ["/login", "/whoami"]
        })),
    ];
    let cli: std::collections::BTreeSet<String> = ["subagents".to_string()].into_iter().collect();
    let block = plugins_block(&manifests, &cli);
    assert!(block.contains("subagents 0.5.2"), "{block}");
    assert!(block.contains("commands /subagents narrow"), "{block}");
    assert!(block.contains("tools narrow_delegate"), "{block}");
    assert!(
        block.contains("cli `gray subagents` — verbs: run status stop"),
        "{block}"
    );
    assert!(block.contains("account 0.6.0"), "{block}");
    assert!(!block.contains("gray account"), "{block}");
    assert!(plugins_block(&[], &cli).is_empty());
}

#[test]
fn plugins_block_truncates_long_tool_lists() {
    let manifest = gray_plugin::Manifest::from_result(&serde_json::json!({
        "name": "wide",
        "tools": (0..12)
            .map(|i| serde_json::json!({"name": format!("t{i}")}))
            .collect::<Vec<_>>()
    }));
    let block = plugins_block(&[manifest], &Default::default());
    assert!(block.contains("t7, +4 more"), "{block}");
    assert!(!block.contains("t8"), "{block}");
}

#[test]
fn session_card_reports_directory_repo_and_branch() {
    let dir = std::env::temp_dir().join(format!("gray-card-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .output()
            .unwrap()
    };
    let card = crate::system_prompt::session_card(&dir);
    assert_eq!(card, format!("Working directory: {}", dir.display()));

    run(&["init", "-b", "card-test"]);
    run(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "--allow-empty",
        "-m",
        "i",
    ]);
    let card = crate::system_prompt::session_card(&dir);
    assert!(card.contains("\nRepo: "), "{card}");
    assert!(card.contains("  Branch: card-test"), "{card}");
    std::fs::remove_dir_all(&dir).ok();
}
