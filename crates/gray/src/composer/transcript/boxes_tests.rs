use super::*;

#[test]
fn replayed_thinking_keeps_every_line_dim_italic() {
    // Resume must show persisted reasoning: one row per source line,
    // in the live thinking style. Blank blocks paint nothing.
    let rows = thinking_replay_lines("first\nsecond");
    assert_eq!(rows.len(), 2, "{rows:?}");
    for r in &rows {
        assert!(r.spans.iter().all(|s| s.style == thinking_style()), "{r:?}");
    }
    assert!(thinking_replay_lines("   \n  ").is_empty());
    assert!(thinking_replay_lines("").is_empty());
}

#[test]
fn adjacent_file_links_do_not_steal_previous_url() {
    // TODO-list shape: two adjacent bullets carrying different file URLs.
    // Second commit arrives as slice [line 1] with absolute hyperlinks
    // for lines 0..2 and offset 1; it must resolve to NOTES.txt, not the
    // previous line's src/main.rs URL.
    let hyperlinks = vec![
        HyperlinkTarget {
            line_index: 0,
            column_range: 2..14,
            url: "file:///repo/src/main.rs".to_string(),
            id: 1,
        },
        HyperlinkTarget {
            line_index: 1,
            column_range: 2..13,
            url: "file:///repo/NOTES.txt".to_string(),
            id: 2,
        },
    ];
    let rebased = rebase_hyperlinks_for_slice(&hyperlinks, 1, 1);
    assert_eq!(
        rebased.len(),
        1,
        "only the sliced line's link survives: {rebased:?}"
    );
    assert_eq!(rebased[0].line_index, 0);
    assert_eq!(rebased[0].url, "file:///repo/NOTES.txt");
    assert_eq!(rebased[0].column_range, 2..13);
}
