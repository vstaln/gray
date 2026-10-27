use super::format_tool_result_lines_with_context as body;
use serde_json::json;

fn text(lines: &[ratatui::text::Line<'_>]) -> String {
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn plugin_tool_results_render_a_numbered_body() {
    let out = "▶ abc:5  fixed the picker\n  files: src/x.rs";
    for name in ["recall", "memory", "mcp__srv__lookup"] {
        let rows = body(name, Some(&json!({"query": "picker"})), out, false, None);
        let shown = text(&rows);
        assert!(
            shown.contains("1 | ▶ abc:5  fixed the picker"),
            "{name}: {shown}"
        );
        assert!(shown.contains("2 |   files: src/x.rs"), "{name}: {shown}");
    }
}

#[test]
fn long_plugin_results_are_cut_like_bash_output() {
    let out: String = (1..=100).map(|i| format!("hit {i}\n")).collect();
    let rows = body("recall", None, &out, false, None);
    let shown = text(&rows);
    assert!(shown.contains("… +76 lines"), "{shown}");
    assert!(shown.contains("100 | hit 100"), "{shown}");
    assert!(!shown.contains("hit 50\n"), "{shown}");
}

#[test]
fn bulky_or_echoed_results_stay_hidden() {
    for name in ["read", "skill", "request_user_input", "discord_ui_schema"] {
        assert!(
            body(name, None, "some\noutput", false, None).is_empty(),
            "{name}"
        );
    }
}
