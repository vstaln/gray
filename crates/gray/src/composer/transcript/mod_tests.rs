use super::*;

#[test]
fn word_flush_cut_breaks_at_spaces() {
    let chars: Vec<char> = "hello world foo".chars().collect();
    let cut = word_flush_cut(&chars, 8);
    let row: String = chars[..cut].iter().collect();
    let rest: String = chars[cut..].iter().collect();
    assert_eq!(row, "hello ");
    assert_eq!(rest, "world foo");
}

#[test]
fn word_flush_cut_hard_cuts_overlong_word() {
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyz".chars().collect();
    assert_eq!(word_flush_cut(&chars, 8), 8);
}

#[test]
fn word_flush_cut_exact_fit_pushes_whole() {
    let chars: Vec<char> = "hi you".chars().collect();
    assert_eq!(word_flush_cut(&chars, 6), 6);
}

#[test]
fn word_flush_cut_budgets_cells_not_chars() {
    let chars: Vec<char> = "界界界界".chars().collect();
    // 2×2 cells fit; a third wide char would overflow the 5-cell budget.
    assert_eq!(word_flush_cut(&chars, 5), 2);
}

#[test]
fn thought_duration_matches_opencode_locale() {
    assert_eq!(fmt_thought_duration(Duration::from_millis(198)), "198ms");
    assert_eq!(fmt_thought_duration(Duration::from_millis(5800)), "5.8s");
    assert_eq!(fmt_thought_duration(Duration::from_millis(9800)), "9.8s");
    assert_eq!(fmt_thought_duration(Duration::from_millis(61_500)), "1m 1s");
}

#[test]
fn thought_summary_line_names_duration() {
    let line = thought_summary_line(Duration::from_millis(5800));
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "✻ Thought for 5.8s");
    assert_eq!(
        line.spans[0].style.fg,
        // Single gray palette (no global read — keeps this test
        // hermetic under parallel execution).
        Some(crate::theme::GRAY_UI_THEME.text_muted)
    );
}

#[test]
fn user_prompt_wraps_at_word_boundaries() {
    let text = "write a very long poem about the restless sea";
    let lines = format_user_prompt_lines(text, &[], 24);
    // content rows (skip blank margins) preserve the text exactly
    let bodies: Vec<String> = lines
        .iter()
        .filter_map(|l| l.spans.get(1))
        .map(|s| s.content.to_string())
        .collect();
    assert!(bodies.len() > 1);
    assert_eq!(bodies.concat(), text);
    // every row except the last ends at a space or is a full hard cut
    let max_w = 24usize - 4;
    for b in &bodies[..bodies.len() - 1] {
        assert!(
            b.ends_with(' ') || b.chars().count() == max_w,
            "mid-word break: {b:?}"
        );
    }
}

#[test]
fn user_prompt_wraps_wide_chars_by_cells() {
    let text = "界界界界界";
    let lines = format_user_prompt_lines(text, &[], 6); // 2-cell budget
    let bodies: Vec<Span<'static>> = lines
        .iter()
        .filter_map(|l| l.spans.get(1).cloned())
        .collect();
    assert_eq!(
        bodies
            .iter()
            .map(|s| s.content.to_string())
            .collect::<String>(),
        text
    );
    assert!(bodies.len() > 1);
    for b in &bodies {
        assert!(b.width() <= 2, "row overflows: {:?}", b.content);
    }
}

#[test]
fn diff_rows_pad_edge_to_edge() {
    use crate::tool_fmt::{diff_delete_bg, diff_insert_bg};
    let header = Line::from("Ran edit");
    let body = vec![
        Line::from(vec![Span::styled(
            "  1 | - old",
            Style::default().bg(diff_delete_bg()),
        )])
        .style(Style::default().bg(diff_delete_bg())),
        Line::from(vec![Span::styled(
            "  1 | + new",
            Style::default().bg(diff_insert_bg()),
        )])
        .style(Style::default().bg(diff_insert_bg())),
        Line::from(vec![Span::raw("  2 |   same")]),
    ];
    let lines = format_tool_box_lines(header, &body, 80);
    let row_w = |l: &Line<'static>| l.spans.iter().map(|s| s.width()).sum::<usize>();
    // margin, header, breathing row, then the three body rows
    assert_eq!(lines.len(), 7);
    // tinted rows span the full width (no dark strip on the right)
    assert_eq!(row_w(&lines[3]), 80);
    assert_eq!(row_w(&lines[4]), 80);
    // untinted rows are untouched (card block bg shows through, same color)
    assert!(row_w(&lines[5]) < 80);
}

#[test]
fn wrap_ranges_round_trip_and_identity() {
    // identity: short line maps to the whole source
    let short = Line::from(vec![Span::raw("hello world")]);
    let out = wrap_styled_line_with_ranges(short, 20);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1.end, usize::MAX);

    // long line: rows fit max_w and each row's text equals the source slice
    let text =
        "the quick brown fox jumps over the lazy dog again and again until it wraps somewhere";
    let long = Line::from(vec![Span::raw(text.to_string())]);
    let max_w = 24;
    let out = wrap_styled_line_with_ranges(long, max_w);
    assert!(out.len() > 1);
    let mut prev_end = 0usize;
    for (l, r) in &out {
        let row_text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            row_text,
            &text[r.clone()],
            "row text must equal its source slice"
        );
        assert!(r.start >= prev_end, "ranges ascend without overlap");
        prev_end = r.end;
    }
}

/// Resize regression: the live thinking cut (`word_flush_cut` at the
/// old width) followed by a reflow re-wrap (each stored chunk wrapped
/// at the new width) must preserve every word — narrowing or widening
/// the window mid-turn must never "shorten" the reasoning.
#[test]
fn thinking_survives_resize_round_trip() {
    let text = "The second command was blocked by a guard because of `curl ... | python3`? \
            Weird, the first one worked. Let me avoid pipes into interpreters and write to a file instead";
    let norm = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (w_live, w_new) in [(100usize, 40usize), (40usize, 100usize)] {
        // live cut, mirroring `stream_thinking`
        let max_live = w_live.saturating_sub(4).max(1);
        let mut chunks: Vec<String> = Vec::new();
        let mut rest: Vec<char> = text.chars().collect();
        while !rest.is_empty() {
            let s: String = rest.iter().collect();
            if display_width(&s) < max_live {
                chunks.push(s);
                break;
            }
            let cut = word_flush_cut(&rest, max_live);
            assert!(cut > 0, "cut must progress");
            chunks.push(rest[..cut].iter().collect());
            rest = rest[cut..].to_vec();
        }
        // reflow re-wrap, mirroring `reflow_on_resize` (render budget w-2)
        let mut rows: Vec<String> = Vec::new();
        for c in &chunks {
            let line = Line::from(vec![Span::styled(c.clone(), thinking_style())]);
            for w in wrap_styled_line(line, w_new.saturating_sub(2).max(1)) {
                rows.push(w.spans.iter().map(|s| s.content.as_ref()).collect());
            }
        }
        // inter-row boundary spaces are re-flowable (live chunks keep
        // a trailing space the wrapper then drops); words must survive
        let got = rows
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            got, norm,
            "text lost resizing {w_live} -> {w_new}: {rows:?}"
        );
    }
}

/// Paragraph has no `.wrap()`: any row wider than the viewport is
/// hard-clipped at the right edge (the "shortened" symptom). The wrapper
/// must therefore never emit an over-budget row, including overlong
/// words (URLs) that force hard cuts.
#[test]
fn wrapped_rows_never_exceed_budget() {
    let text = format!(
        "{} https://api.github.com/repos/some/really/long/path/that/never/breaks/at/all",
        "word ".repeat(50)
    );
    for w in [20usize, 40, 80, 120] {
        for row in wrap_styled_line(Line::from(text.clone()), w) {
            let rw: usize = row.spans.iter().map(|s| s.width()).sum();
            assert!(rw <= w, "row overflows budget {w}: {row:?}");
        }
    }
}

// UNRUN (cargo test banned in X session; run in TTY/CI): over-cap push
// evicts oldest-first, mirroring the transcript >1000/drain-100 guard.
#[test]
fn history_entries_cap_evicts_oldest_first_unrun() {
    let mut entries: Vec<crate::composer::TranscriptEntry> = (0..1001)
        .map(crate::composer::TranscriptEntry::Gap)
        .collect();
    cap_history_entries(&mut entries);
    assert_eq!(entries.len(), 901);
    match &entries[0] {
        crate::composer::TranscriptEntry::Gap(n) => assert_eq!(*n, 100),
        other => panic!("must drop gaps 0..100 oldest-first, got {other:?}"),
    }
}

// UNRUN (cargo test banned in X session; run in TTY/CI): at-cap is a no-op.
#[test]
fn history_entries_cap_keeps_at_most_1000_unrun() {
    let mut entries: Vec<crate::composer::TranscriptEntry> = (0..1000)
        .map(crate::composer::TranscriptEntry::Gap)
        .collect();
    cap_history_entries(&mut entries);
    assert_eq!(entries.len(), 1000);
}
