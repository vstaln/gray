use super::*;
use ratatui::text::{Line, Span};

fn live(id: &str, text: &str, running: bool) -> LiveTool {
    LiveTool {
        id: id.to_string(),
        header: Line::from(vec![Span::raw(text.to_string())]),
        running,
    }
}

fn row_text(l: &Line<'_>) -> String {
    l.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn live_rows_preserve_order_and_cap_at_three() {
    let tools: Vec<LiveTool> = (0..5)
        .map(|i| live(&format!("t{i}"), &format!("tool {i}"), false))
        .collect();
    let rows = live_tool_rows(&tools, Duration::ZERO);
    assert_eq!(rows.len(), 3, "cap mirrors the queued preview");
    assert_eq!(row_text(&rows[0]), "tool 0");
    assert_eq!(row_text(&rows[2]), "tool 2");
    assert_eq!(live_tool_overflow(tools.len()), 2);
    assert_eq!(live_tool_overflow(3), 0);
    assert_eq!(live_tool_overflow(0), 0);
}

fn bash_live(command: &str, running: bool) -> LiveTool {
    LiveTool {
        id: "bash".into(),
        header: crate::tool_fmt::format_tool_call_header(
            "bash",
            &serde_json::json!({"command": command}),
            None,
        ),
        running,
    }
}

#[test]
fn live_rows_mark_running_cards() {
    let tool = bash_live("cargo test", true);
    let original = tool.header.clone();
    let rows = live_tool_rows(&[tool.clone()], Duration::ZERO);
    assert_eq!(row_text(&rows[0]), "⬡ Running cargo test");
    assert_eq!(row_text(&tool.header), row_text(&original));
    assert_eq!(rows[0].spans.last(), original.spans.last());
    let later = live_tool_rows(&[tool], Duration::from_secs(1));
    assert_eq!(row_text(&rows[0]), row_text(&later[0]));
    assert_ne!(
        rows[0], later[0],
        "label must shimmer without changing text"
    );
    let preparing = bash_live("cargo test", false);
    assert_eq!(
        live_tool_rows(&[preparing.clone()], Duration::ZERO)[0],
        preparing.header
    );
}

#[test]
fn live_rows_update_in_place_without_reorder() {
    // pi `updateArgs`: same id replaces the header, first-seen order kept.
    let mut tools = vec![live("a", "old", false), live("b", "bee", false)];
    // mirror of `upsert_live_tool` ordering (pure, no Tui::new/TTY):
    if let Some(slot) = tools.iter_mut().find(|t| t.id == "a") {
        slot.header = Line::from(vec![Span::raw("new".to_string())]);
        slot.running = true;
    }
    let rows = live_tool_rows(&tools, Duration::ZERO);
    assert_eq!(row_text(&rows[0]), "Running new");
    assert_eq!(row_text(&rows[1]), "bee");
}

#[test]
fn running_label_does_not_orphan_a_trailing_status() {
    for command in ["sleep 5", "echo café 日本語", &"x".repeat(100)] {
        let tool = bash_live(command, true);
        // The replacement verb needs four more cells than "Ran", not a tail row.
        let width = tool.header.width() + 4;
        let rows = live_tool_rows(&[tool], Duration::ZERO);
        let wrapped = crate::composer::transcript::wrap_styled_line(rows[0].clone(), width);
        assert_eq!(wrapped.len(), 1, "{wrapped:?}");
        for width in [12, 40, 80] {
            let wrapped = crate::composer::transcript::wrap_styled_line(rows[0].clone(), width);
            assert!(
                wrapped
                    .iter()
                    .all(|row| !row_text(row).contains("running…"))
            );
        }
    }
}

#[test]
fn other_tools_keep_their_identity_and_empty_headers_are_safe() {
    let tool = LiveTool {
        id: "plugin".into(),
        header: crate::tool_fmt::format_tool_call_header("custom", &serde_json::json!({}), None),
        running: true,
    };
    let rows = live_tool_rows(&[tool], Duration::ZERO);
    assert!(row_text(&rows[0]).starts_with("⬡ Running custom"));
    assert_eq!(
        row_text(&live_tool_rows(&[live("empty", "", true)], Duration::ZERO)[0]),
        "Running "
    );
}
