use super::*;

#[test]
fn title_uses_first_line_and_caps() {
    assert_eq!(build_title("  hello world\nsecond"), "hello world");
    assert_eq!(build_title("\n\n  \nreal"), "real");
    assert_eq!(build_title(""), "Feedback");
    let long = "w".repeat(200);
    let t = build_title(&long);
    assert!(t.chars().count() <= MAX_TITLE_CHARS + 1, "{t}");
    assert!(t.ends_with('…'));
}

#[test]
fn body_carries_environment_footer() {
    let b = build_body(
        "broken x",
        "0.1.0",
        "linux / x86_64",
        "m",
        "s1",
        "ghostty",
        "bash",
    );
    assert!(b.contains("## Summary\nbroken x"), "{b}");
    assert!(b.contains("## Expected Behavior\n—"), "{b}");
    assert!(b.contains("## Actual Behavior\nbroken x"), "{b}");
    assert!(b.contains("## Steps to Reproduce\n—"), "{b}");
    assert!(b.contains("Gray Version: 0.1.0"), "{b}");
    assert!(b.contains("Operating System: linux / x86_64"), "{b}");
    assert!(b.contains("Terminal: ghostty"), "{b}");
    assert!(b.contains("Shell: bash"), "{b}");
    assert!(b.contains("Model: m"), "{b}");
    assert!(b.contains("Session: s1"), "{b}");
    assert!(b.contains("## Fix prompt\n—"), "{b}");
    assert!(b.contains("## Additional context\n—"), "{b}");
}

#[test]
fn terminal_label_prefers_term_program() {
    assert_eq!(
        terminal_label(Some("ghostty"), Some("xterm-256color")),
        "ghostty"
    );
    assert_eq!(
        terminal_label(None, Some("xterm-256color")),
        "xterm-256color"
    );
    assert_eq!(terminal_label(Some(""), Some("xterm")), "xterm");
    assert_eq!(terminal_label(None, None), "unknown");
    assert_eq!(terminal_label(Some(""), Some("")), "unknown");
}

#[test]
fn shell_label_basename() {
    assert_eq!(shell_label(Some("/bin/bash")), "bash");
    assert_eq!(shell_label(Some("/usr/bin/zsh")), "zsh");
    assert_eq!(shell_label(Some("fish")), "fish");
    assert_eq!(shell_label(None), "unknown");
    assert_eq!(shell_label(Some("")), "unknown");
}

#[test]
fn url_is_prefilled_and_encoded() {
    let u = issue_url("a b", "c&d");
    assert!(u.starts_with(ISSUES_NEW_URL));
    assert!(u.contains("title=a%20b"), "{u}");
    assert!(u.contains("body=c%26d"), "{u}");
}

#[test]
fn url_body_truncates_but_file_does_not() {
    let big = "x".repeat(MAX_URL_BODY_CHARS + 10);
    let u = issue_url("t", &big);
    assert!(u.contains("truncated"), "{u}");
    let dir = tempfile::tempdir().unwrap();
    let p = save_feedback(dir.path(), "t", &big, "stamp").unwrap();
    let saved = std::fs::read_to_string(p).unwrap();
    assert!(saved.contains(&big));
}

#[test]
fn save_dedups_on_collision() {
    let dir = tempfile::tempdir().unwrap();
    let a = save_feedback(dir.path(), "t", "b", "s").unwrap();
    let b = save_feedback(dir.path(), "t", "b", "s").unwrap();
    assert_ne!(a, b);
    assert!(b.to_string_lossy().contains("feedback-s-2.md"));
}

#[test]
fn save_never_overwrites_a_reserved_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("feedback-s.md");
    std::fs::write(&base, "original").unwrap();
    let p = save_feedback(dir.path(), "t", "b", "s").unwrap();
    assert!(p.to_string_lossy().ends_with("feedback-s-2.md"));
    assert_eq!(std::fs::read_to_string(&base).unwrap(), "original");
}

#[test]
fn save_refuses_when_suffix_space_is_exhausted() {
    let dir = tempfile::tempdir().unwrap();
    for n in 1..100 {
        let name = if n == 1 {
            "feedback-s.md".to_string()
        } else {
            format!("feedback-s-{n}.md")
        };
        std::fs::write(dir.path().join(name), "original").unwrap();
    }
    assert!(save_feedback(dir.path(), "t", "b", "s").is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("feedback-s-99.md")).unwrap(),
        "original"
    );
}
