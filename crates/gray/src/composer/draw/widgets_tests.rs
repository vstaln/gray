use super::*;

fn row_texts(ibox: &InputBox) -> Vec<String> {
    ibox.lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

#[test]
fn input_box_wraps_at_word_boundaries() {
    // Narrow box forces a wrap inside "...with colors." — the word must
    // move whole to the next row, never split as "c" / "olors.".
    let text = "aa bb cc dd ee ff with colors.";
    let ibox = build_input_box(text, text.len(), 20, None);
    let rows = row_texts(&ibox);
    let joined = rows.join("\n");
    assert!(
        joined.contains("colors."),
        "word must survive whole: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.ends_with('c') && r.contains("with ")),
        "must not split mid-word: {rows:?}"
    );
    // Cursor at end must land on the last content row (both margin rows
    // excluded from cur_row's target, the top pad's offset included).
    assert_eq!(ibox.cur_row, rows.len() - 2, "rows: {rows:?}");
}

#[test]
fn input_box_ghost_hint_shows_only_when_empty() {
    // Resume pending: empty box paints the dim ghost hint, same 3 rows.
    let ibox = build_input_box("", 0, 80, Some("Please continue…"));
    let rows = row_texts(&ibox);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows[1].contains("Please continue"), "{rows:?}");
    // No resume: unchanged bare prompt.
    let ibox = build_input_box("", 0, 80, None);
    assert_eq!(row_texts(&ibox)[1].trim(), "❯");
    // Typed text never shows the ghost.
    let ibox = build_input_box("hi", 2, 80, Some("Please continue…"));
    assert!(
        !row_texts(&ibox).iter().any(|r| r.contains("Please")),
        "{:?}",
        row_texts(&ibox)
    );
}
