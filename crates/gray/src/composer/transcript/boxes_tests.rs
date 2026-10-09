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

#[test]
fn compaction_summary_renders_markdown_not_literal() {
    // `/compact` summary is LLM markdown (`**bold**`, `##` headings):
    // it must render through the markdown pipeline like assistant
    // answers, not `push_dim` literally (screenshot: raw `**` markers).
    let (lines, _) = render_markdown_lines("**Active request:** foo", Some(80));
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !text.contains("**"),
        "markers must render, not print: {text:?}"
    );
    assert!(text.contains("Active request:"), "{text:?}");
    assert!(
        lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD)),
        "strong must carry BOLD: {lines:?}"
    );
    assert!(render_markdown_lines("   \n  ", Some(80)).0.is_empty());
    assert!(render_markdown_lines("", Some(80)).0.is_empty());
}

#[test]
fn a_turn_error_reads_as_an_error_not_as_prose() {
    // A rate limit used to stream as plain assistant prose. The headline
    // takes the error colour, the hint below it is muted.
    let theme = crate::theme::theme();
    let msg = "✗ Rate limited (retryable): status 429: slow down\n  Try again later or switch model via /model.\n";
    let rows = error_lines(msg);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows[0]
            .spans
            .iter()
            .all(|s| s.style.fg == Some(theme.error))
    );
    assert!(rows[0].spans[0].content.starts_with("✗ Rate limited"));
    assert!(
        rows[1]
            .spans
            .iter()
            .all(|s| s.style.fg == Some(theme.text_muted))
    );
    assert!(error_lines("  \n").is_empty());
}

#[test]
fn long_header_breaks_before_its_in_detail() {
    use ratatui::text::{Line, Span};
    let header = Line::from(vec![
        Span::raw("\u{2b22} "),
        Span::raw("Searched "),
        Span::raw("build the intuition\\|totally normal\\|sees that"),
        Span::raw(" \u{00b7} in /home/u/content/videos/attention-explained/SCRIPT-FINAL.txt"),
    ]);
    let text = |l: &Line<'_>| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>();
    let rows = super::cards::header_rows(header.clone(), 70);
    let got: Vec<String> = rows.iter().map(text).collect();
    assert_eq!(
        got,
        vec![
            "\u{2b22} Searched build the intuition\\|totally normal\\|sees that".to_string(),
            "  in /home/u/content/videos/attention-explained/SCRIPT-FINAL.txt".to_string(),
        ]
    );
    // Even when it fits, `in <dir>` takes its own row.
    let rows = super::cards::header_rows(header, 200);
    assert_eq!(rows.len(), 2);
    assert_eq!(text(&rows[1]), "  in /home/u/content/videos/attention-explained/SCRIPT-FINAL.txt");
}

#[test]
fn other_header_details_stay_inline_when_they_fit() {
    use ratatui::text::{Line, Span};
    let header = Line::from(vec![
        Span::raw("\u{2b22} "),
        Span::raw("Background job bwrap-5"),
        Span::raw(" \u{00b7} exit 0"),
    ]);
    let rows = super::cards::header_rows(header, 80);
    assert_eq!(rows.len(), 1);
}
