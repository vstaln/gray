use super::*;
use ratatui::style::Color;

const TXT: Color = Color::White;
const SEP: Color = Color::DarkGray;

fn known(name: &str) -> Option<Seg> {
    match name {
        "context" => Some(Seg::new("12k/200k", Color::Cyan)),
        "cache" => Some(Seg::new("80.0% cache", Color::Green)),
        "timer" => Some(Seg::new("", TXT)),
        "model" => Some(Seg::new("Opus", TXT)),
        "branch" => Some(Seg::new("main", TXT)),
        _ => None,
    }
}

fn spec(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn texts(segs: &[Seg]) -> String {
    segs.iter().map(|s| s.text.as_str()).collect()
}

#[test]
fn defaults_match_the_stock_footer() {
    let c = StatusLineConfig::default();
    assert_eq!(c.left(), spec(DEFAULT_LEFT));
    assert_eq!(c.right(), spec(DEFAULT_RIGHT));
    assert_eq!(c.separator(), " \u{b7} ");
    assert_eq!(c.interval(), Duration::from_millis(5_000));
}

#[test]
fn interval_has_a_floor() {
    let c = StatusLineConfig {
        interval_ms: Some(10),
        ..Default::default()
    };
    assert_eq!(c.interval(), Duration::from_millis(MIN_INTERVAL_MS));
}

#[test]
fn compose_drops_empty_segments_with_their_separator() {
    let segs = compose(
        &spec(&["context", "timer", "cache"]),
        &known,
        " | ",
        SEP,
        TXT,
    );
    assert_eq!(texts(&segs), "12k/200k | 80.0% cache");
    assert_eq!(segs[0].color, Color::Cyan);
    assert_eq!(segs[1].color, SEP);
    assert_eq!(segs[2].color, Color::Green);
}

#[test]
fn compose_all_empty_is_empty() {
    assert!(compose(&spec(&["timer"]), &known, " · ", SEP, TXT).is_empty());
    assert!(compose(&[], &known, " · ", SEP, TXT).is_empty());
}

#[test]
fn unknown_names_are_literal_and_templates_expand() {
    let segs = compose(
        &spec(&["hello", "\u{2387} {branch}", "{nope}"]),
        &known,
        " ",
        SEP,
        TXT,
    );
    // "{nope}" renders empty and drops out.
    assert_eq!(texts(&segs), "hello \u{2387} main");
    assert_eq!(segs[2].color, TXT);
}

#[test]
fn template_escapes_and_unclosed_braces() {
    assert_eq!(render_template("{{x}} {model}", &known), "{x}} Opus");
    assert_eq!(render_template("a {model", &known), "a {model");
    assert_eq!(render_template("{ model }", &known), "Opus");
}

#[test]
fn plugin_statuses_are_one_line_and_bump_only_on_change() {
    set_status("zz-test", Some("  \x1b[31mred\x1b[0m\nsecond  "));
    assert_eq!(status("zz-test").as_deref(), Some("red"));
    let v = version();
    set_status("zz-test", Some("red"));
    assert_eq!(version(), v, "same text must not repaint");
    set_status("zz-test", Some(""));
    assert_eq!(status("zz-test"), None);
    assert!(version() > v);
}

#[test]
fn config_parses_from_json_and_ignores_missing_fields() {
    let c: StatusLineConfig =
        serde_json::from_str(r#"{"left":["branch"],"command":"echo hi"}"#).unwrap();
    assert_eq!(c.left(), spec(&["branch"]));
    assert_eq!(c.right(), spec(DEFAULT_RIGHT));
    assert_eq!(c.command.as_deref(), Some("echo hi"));
}

#[test]
fn run_command_feeds_stdin_and_returns_stdout() {
    let out = run_command("cat", &serde_json::json!({"a": 1})).unwrap();
    assert_eq!(out, r#"{"a":1}"#);
    assert_eq!(
        run_command("exit 3; echo no", &serde_json::Value::Null).as_deref(),
        Some("")
    );
}

#[test]
fn read_branch_handles_refs_detached_and_worktrees() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(read_branch(root), None);
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/feat/x\n").unwrap();
    let sub = root.join("a/b");
    std::fs::create_dir_all(&sub).unwrap();
    assert_eq!(read_branch(&sub).as_deref(), Some("feat/x"));

    std::fs::write(root.join(".git/HEAD"), "0123456789abcdef\n").unwrap();
    assert_eq!(read_branch(root).as_deref(), Some("0123456"));

    let wt = tempfile::tempdir().unwrap();
    let gd = root.join(".git/worktrees/wt");
    std::fs::create_dir_all(&gd).unwrap();
    std::fs::write(gd.join("HEAD"), "ref: refs/heads/side\n").unwrap();
    std::fs::write(
        wt.path().join(".git"),
        format!("gitdir: {}\n", gd.display()),
    )
    .unwrap();
    assert_eq!(read_branch(wt.path()).as_deref(), Some("side"));
}
