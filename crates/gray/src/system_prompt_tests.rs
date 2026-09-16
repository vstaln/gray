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
