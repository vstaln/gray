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
        p.trim_start().starts_with("You are gray, a minimal agent"),
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
fn memory_is_separate_from_verbatim_prompt_and_is_not_comment_stripped() {
    let body = Some("You are gray.<!-- hidden -->".to_string());
    let data = r#"{"user":"<!-- fact -->","decisions":"Use Rust."}"#;
    let prompt = with_memory(build_system_prompt(body.clone()), Some(data));
    assert!(prompt.starts_with("You are gray.\n\n"));
    assert!(prompt.contains("gray memory"));
    assert!(prompt.ends_with(data));
    assert_eq!(
        prompt,
        with_memory(build_system_prompt(body.clone()), Some(data))
    );
    assert_eq!(
        with_memory(build_system_prompt(body), None),
        "You are gray."
    );
}

#[test]
fn memory_preserves_runtime_directory_and_stored_prompt() {
    let cwd = std::path::Path::new("/work/project café");
    let runtime = build_runtime_prompt(opts("Rules.<!-- private note -->"), cwd);
    let data = r#"{"user":"Keep replies concise.","decisions":"Use Rust."}"#;
    let combined = with_memory(runtime.clone(), Some(data));
    assert!(combined.starts_with(&runtime));
    assert!(combined.ends_with(data));
    assert!(combined.contains("Working directory: \"/work/project café\""));
    assert!(!combined.contains("private note"));
    assert_eq!(with_memory(runtime.clone(), None), runtime);
}
