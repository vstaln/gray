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
    // Cursor at end must land on the last content row (the bottom margin is
    // excluded from cur_row).
    assert_eq!(ibox.cur_row, rows.len() - 2, "rows: {rows:?}");
}

#[test]
fn input_box_hard_cuts_only_overlong_words() {
    let text = "ok abcdefghijklmnopqrstuvwxyz0129 end";
    let ibox = build_input_box(text, 0, 20, None);
    let rows = row_texts(&ibox);
    assert!(rows.iter().any(|r| r.contains("ok ")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("end")), "{rows:?}");
}

/// One blank row above the input box, never two: the transcript's own
/// trailing gap (or the latched dock seam) already owns the separation, so
/// a pad row inside the box stacked a second empty row on top of it.
#[test]
fn input_box_has_a_bottom_margin_and_no_top_pad() {
    let ibox = build_input_box("", 0, 80, None);
    let rows = row_texts(&ibox);
    assert_eq!(rows.len(), 2, "prompt + bottom margin: {rows:?}");
    assert!(rows.first().unwrap().contains('❯'));
    assert!(rows.last().unwrap().trim().is_empty());
}

#[test]
fn input_box_ghost_hint_shows_only_when_empty() {
    // Resume pending: empty box paints the dim ghost hint, same 2 rows.
    let ibox = build_input_box("", 0, 80, Some("Please continue…"));
    let rows = row_texts(&ibox);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].contains("Please continue"), "{rows:?}");
    // No resume: unchanged bare prompt.
    let ibox = build_input_box("", 0, 80, None);
    assert_eq!(row_texts(&ibox)[0].trim(), "❯");
    // Typed text never shows the ghost.
    let ibox = build_input_box("hi", 2, 80, Some("Please continue…"));
    assert!(
        !row_texts(&ibox).iter().any(|r| r.contains("Please")),
        "{:?}",
        row_texts(&ibox)
    );
}
