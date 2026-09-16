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
    assert!(expanded.contains("<skill"), "envelope missing: {expanded}");
    assert!(expanded.contains("paste-me"), "name missing: {expanded}");
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

#[test]
fn format_skill_paste_envelope_and_args() {
    let text = format_skill_paste(
        "demo",
        std::path::Path::new("/s/demo/SKILL.md"),
        "Do things.",
        Some("fast"),
    );
    assert!(text.contains("<skill name=\"demo\""), "{text}");
    assert!(text.contains("Do things."), "{text}");
    assert!(text.contains("**ARGUMENTS:** fast"), "{text}");
    let bare = format_skill_paste(
        "demo",
        std::path::Path::new("/s/demo/SKILL.md"),
        "Do things.",
        None,
    );
    assert!(!bare.contains("ARGUMENTS"), "{bare}");
}

// UNRUN (cargo test banned under X): run in TTY/CI.
// reload_agent with no model configured fails soft through say()
// (headless println path) and preserves the previous agent.
#[tokio::test]
async fn reload_agent_failure_preserves_agent() {
    let config = Config {
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
