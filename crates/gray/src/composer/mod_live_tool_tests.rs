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
    let rows = live_tool_rows(&tools);
    assert_eq!(rows.len(), 3, "cap mirrors the queued preview");
    assert_eq!(row_text(&rows[0]), "tool 0");
    assert_eq!(row_text(&rows[2]), "tool 2");
    assert_eq!(live_tool_overflow(tools.len()), 2);
    assert_eq!(live_tool_overflow(3), 0);
    assert_eq!(live_tool_overflow(0), 0);
}

#[test]
fn live_rows_mark_running_cards() {
    let tools = vec![live("a", "Ran cargo test", true)];
    let rows = live_tool_rows(&tools);
    assert_eq!(rows.len(), 1);
    assert!(
        row_text(&rows[0]).contains("running…"),
        "got {:?}",
        row_text(&rows[0])
    );
    let tools = vec![live("a", "Ran cargo test", false)];
    assert!(!row_text(&live_tool_rows(&tools)[0]).contains("running"));
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
    let rows = live_tool_rows(&tools);
    assert_eq!(row_text(&rows[0]), "new · running…");
    assert_eq!(row_text(&rows[1]), "bee");
}
