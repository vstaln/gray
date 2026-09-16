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
    let ibox = build_input_box(text, text.len(), 20);
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
    // Cursor at end must land on the last content row (top/bottom
    // margins excluded from cur_row).
    assert_eq!(ibox.cur_row, rows.len() - 3, "rows: {rows:?}");
}

#[test]
fn input_box_hard_cuts_only_overlong_words() {
    let text = "ok abcdefghijklmnopqrstuvwxyz0129 end";
    let ibox = build_input_box(text, 0, 20);
    let rows = row_texts(&ibox);
    assert!(rows.iter().any(|r| r.contains("ok ")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("end")), "{rows:?}");
}

#[test]
fn input_box_has_top_and_bottom_margin_rows() {
    let ibox = build_input_box("", 0, 80);
    let rows = row_texts(&ibox);
    assert_eq!(
        rows.len(),
        3,
        "top margin + prompt + bottom margin: {rows:?}"
    );
    assert!(rows.first().unwrap().trim().is_empty());
    assert!(rows.last().unwrap().trim().is_empty());
    assert!(rows[1].contains('❯'));
}
