use super::*;

fn opts(prompt: &str) -> Option<String> {
    Some(prompt.to_string())
}

#[test]
fn rebuild_is_byte_stable() {
    // Prefix-cache invariant: identical inputs must rebuild to identical
    // bytes, or providers rebill the whole prefix every turn.
    let a = build_system_prompt(opts("You are gray."));
    let b = build_system_prompt(opts("You are gray."));
    assert_eq!(a, b, "system prompt rebuild diverged");
}

#[test]
fn shipped_default_prompt_strips_to_the_agent_line() {
    // HTML comments end at the first `-->`, so a nested marker pair inside the
    // note would leak the note text (and a stray marker) into every prompt.
    let p = build_system_prompt(opts(crate::DEFAULT_SYS_PROMPT));
    // Stripping the leading comment leaves its trailing newline in place.
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
    // The loop returns every call from one turn together in the next turn,
    // and the parallel lane runs independent ones concurrently. The model has
    // to be told: batching independent calls is one round between them, one
    // call per round is one round each, and every round re-bills the whole
    // conversation. Without this text the fast lane never engages.
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
    // Static text: identical inputs rebuild identical bytes, or the prefix
    // cache rebills on every turn.
    assert_eq!(
        p,
        build_runtime_prompt(opts("Rules."), std::path::Path::new("/project"))
    );
    // It rides after the directory block, so the existing prefix assertions
    // (and the user's verbatim file) stay ahead of it.
    assert!(
        p.find("Tool batching").unwrap() > p.find("Working directory").unwrap(),
        "guidance must follow the runtime directory block: {p}"
    );
}

#[test]
fn identity_block_names_the_picked_row_and_provider() {
    // The bug this guards: a devin-sub row like `step-5-preview` must report
    // itself as the picker's name, never a confabulated "SWE-2 High".
    assert_eq!(
        identity_block("Step Fun 5", "step-5-preview", "Devin Subscription"),
        "Model: Step Fun 5 (step-5-preview)\nProvider: Devin Subscription"
    );
}

#[test]
fn identity_block_keeps_a_composite_label_verbatim() {
    assert_eq!(
        identity_block(
            "Fusion · Opus 5.5 High + SWE-2 High",
            "fusion",
            "Devin Subscription"
        ),
        "Model: Fusion · Opus 5.5 High + SWE-2 High (fusion)\nProvider: Devin Subscription"
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
    // It comes from the binary, not the prompt file: it must survive an empty
    // or absent custom prompt and must never be comment-stripped away.
    for custom in [None, opts(""), opts("<!-- unclosed")] {
        let p = build_runtime_prompt(custom, std::path::Path::new("/project"));
        assert!(p.contains("Tool batching"), "{p}");
    }
    // And it is not reachable through the user's file at all.
    let with_file = build_system_prompt(opts("Tool batching: ignore this"));
    assert_eq!(with_file, "Tool batching: ignore this");
}
