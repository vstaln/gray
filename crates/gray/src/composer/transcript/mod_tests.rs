use super::*;
use crate::composer::TranscriptEntry;

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
fn gap_need_fills_one_above_content() {
    assert_eq!(gap_need(&[Line::from("text")], 1), 1);
}

#[test]
fn gap_need_is_idempotent_above_a_gap() {
    // Checkpoint spacers (thinking close, tool-box edges, turn footer) must
    // never stack a second blank onto an existing gap: closing a run right
    // after another gap leaves the tail alone, so every seam shows exactly
    // one break instead of two.
    let t = vec![Line::from("text"), Line::from("")];
    assert_eq!(gap_need(&t, 1), 0);
}

#[test]
fn gap_need_counts_partial_tail() {
    let t = vec![Line::from("text"), Line::from("")];
    assert_eq!(gap_need(&t, 2), 1);
}

#[test]
fn gap_need_treats_card_margins_as_edges() {
    // Tinted padding rows are card edges, not gaps: content ending on a
    // tool-box margin still earns its separating gap.
    let bg = Style::default().bg(crate::theme::GRAY_UI_THEME.surface_bg);
    let t = vec![Line::from("text"), Line::from("").style(bg)];
    assert_eq!(gap_need(&t, 1), 1);
}

#[test]
fn gap_need_sees_left_padded_rows_as_blank() {
    // Live thinking rows carry the 1-space left pad; a pad-only tail is a
    // gap, never a reason for another blank.
    let t = vec![Line::from(vec![Span::raw(" ")])];
    assert_eq!(gap_need(&t, 1), 0);
}

#[test]
fn gap_need_empty_transcript_still_gaps() {
    assert_eq!(gap_need(&[], 1), 1);
}

#[test]
fn streaming_boundaries_collapse_only_outer_blank_rows() {
    let lines = vec![
        Line::from(""),
        Line::from(""),
        Line::from("first"),
        Line::from(""),
        Line::from("second"),
        Line::from(""),
        Line::from(""),
    ];
    let (normalized, _) = normalize_stream_boundaries(lines.clone(), vec![], true);
    let text: Vec<String> = normalized
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();
    assert_eq!(text, vec!["first", "", "second", ""]);

    let (normalized, _) = normalize_stream_boundaries(lines, vec![], false);
    let text: Vec<String> = normalized
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();
    assert_eq!(text, vec!["", "first", "", "second", ""]);
}

#[test]
fn streaming_boundaries_rebase_links_after_dropping_outer_rows() {
    let lines = vec![
        Line::from(""),
        Line::from("body"),
        Line::from(""),
        Line::from(""),
    ];
    let links = vec![HyperlinkTarget {
        line_index: 1,
        column_range: 0..4,
        url: "file:///repo/body".to_string(),
        id: 1,
    }];
    let (normalized, links) = normalize_stream_boundaries(lines, links, true);
    assert_eq!(normalized.len(), 2);
    assert_eq!(links[0].line_index, 0);
    assert_eq!(links[0].url, "file:///repo/body");
}

#[test]
fn streaming_boundaries_keep_one_gap_for_a_blank_only_block() {
    let (normalized, _) =
        normalize_stream_boundaries(vec![Line::from(""), Line::from("")], vec![], false);
    assert_eq!(normalized.len(), 1);
    assert!(transcript_row_is_blank(&normalized[0]));

    let (normalized, links) =
        normalize_stream_boundaries(vec![Line::from(""), Line::from("")], vec![], true);
    assert!(normalized.is_empty());
    assert!(links.is_empty());
}

#[test]
fn deferred_stream_punctuation_stays_with_the_previous_prose_block() {
    let mut entries = vec![TranscriptEntry::StyledLines {
        lines: vec![Line::from("Let's patch")],
        hyperlinks: vec![],
    }];
    attach_stream_punctuation(&mut entries, Some(0), ".");
    let TranscriptEntry::StyledLines { lines, .. } = &entries[0] else {
        panic!("expected styled lines");
    };
    assert_eq!(lines[0].spans[0].content, "Let's patch.");
}

#[test]
fn deferred_stream_punctuation_is_preserved_without_prior_prose() {
    let mut entries = Vec::new();
    attach_stream_punctuation(&mut entries, None, ".");
    assert!(matches!(
        entries.as_slice(),
        [TranscriptEntry::StyledLines { .. }]
    ));
}

#[test]
fn deferred_punctuation_does_not_attach_to_a_later_status_row() {
    let mut entries = vec![
        TranscriptEntry::StyledLines {
            lines: vec![Line::from("Let's patch")],
            hyperlinks: vec![],
        },
        TranscriptEntry::ToolBox {
            header: Line::from("tool"),
            body: vec![],
        },
        TranscriptEntry::StyledLines {
            lines: vec![Line::from("reconnecting")],
            hyperlinks: vec![],
        },
    ];
    attach_stream_punctuation(&mut entries, Some(0), ".");
    let TranscriptEntry::StyledLines { lines, .. } = &entries[0] else {
        panic!("expected anchored styled lines");
    };
    assert_eq!(lines[0].spans[0].content, "Let's patch.");
    let TranscriptEntry::StyledLines { lines, .. } = &entries[2] else {
        panic!("expected later styled row");
    };
    assert_eq!(lines[0].spans[0].content, "reconnecting");
}

#[test]
fn streaming_punctuation_guard_only_matches_a_standalone_continuation() {
    assert!(is_orphan_stream_punctuation("."));
    assert!(is_orphan_stream_punctuation(" .\n"));
    assert!(is_orphan_stream_punctuation("…"));
    assert!(is_orphan_stream_punctuation("—"));
    assert!(should_drop_stream_punctuation(true, "."));
    assert!(!should_drop_stream_punctuation(false, "."));
    assert!(!is_orphan_stream_punctuation(". next"));
    assert!(!is_orphan_stream_punctuation(":)"));
    assert!(!is_orphan_stream_punctuation("Let's patch"));
    assert!(!is_orphan_stream_punctuation(""));
}

/// A punctuation-only delta with no round boundary armed (mid-round split,
/// post-interrupt tail) must be held back, not fed to the streaming
/// renderer — checkpoints only advance on complete blocks, so the chunk
/// would freeze as its own lone `.` transcript row.
#[test]
fn unboundaryed_punctuation_only_chunks_are_held_not_rendered() {
    // The repro: a trailing `.` arrives as its own chunk.
    assert!(should_hold_stream_chunk(0, "."));
    assert!(should_hold_stream_chunk(0, "…"));
    // Once a burst is held, keep holding punctuation and the whitespace
    // separators between bursts until a meaningful delta releases them.
    assert!(should_hold_stream_chunk(1, "."));
    assert!(should_hold_stream_chunk(3, " "));
    assert!(should_hold_stream_chunk(3, "\n"));
    // Meaningful deltas and the empty chunk render normally.
    assert!(!should_hold_stream_chunk(0, "I'll build it"));
    assert!(!should_hold_stream_chunk(0, ""));
    // The boundary-armed path stays `should_drop`'s job.
    assert!(!should_hold_stream_chunk(0, "x"));
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
    let header = Line::from("Ran edit");
    let body = vec![
        Line::from(vec![Span::styled(
            "  1 | - old",
            Style::default().bg(crate::theme::theme().diff_del_bg),
        )])
        .style(Style::default().bg(crate::theme::theme().diff_del_bg)),
        Line::from(vec![Span::styled(
            "  1 | + new",
            Style::default().bg(crate::theme::theme().diff_add_bg),
        )])
        .style(Style::default().bg(crate::theme::theme().diff_add_bg)),
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

/// tool_fmt pre-wraps numbered rows to `width - 2` cells (2-col lead +
/// gutter + content). The card must not re-wrap them narrower: a 2-col
/// mismatch orphaned each full row's last word onto its own row.
#[test]
fn full_width_numbered_row_is_not_rewrapped() {
    let width = 80;
    let content = "word ".repeat(14); // 70 cells
    let row = format!("    1 | {content}"); // 2 lead + "  1 | " + 70 = 78
    assert_eq!(row.len(), width - 2);
    let lines = format_tool_box_lines(Line::from("Ran x"), &[Line::from(row)], width);
    // margin, header, breathing row, ONE body row, margin
    assert_eq!(lines.len(), 5, "{lines:?}");
}

/// Trailing blank lines of streamed thinking collapse to one separator
/// row; with the structural dock seam off on a blank tail, that one row is
/// the only gap above the live status.
#[test]
fn thinking_trailing_blanks_collapse_to_one_row() {
    let rows = thinking_run_rows("foo\n\n\n\n", 40, false);
    let texts: Vec<String> = rows
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert_eq!(texts, vec![" foo".to_string(), " ".to_string()]);
    assert!(transcript_row_is_blank(rows.last().unwrap()));
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

/// Resize regression: the live thinking cut (`word_flush_cut` at the old
/// width) stores raw run text, and reflow re-wraps whole logical lines at
/// the new width (`thinking_run_rows`). Narrowing must never lose words
/// (no hard-clipped "shortened" rows); widening must re-join the live
/// fragments into full-width rows instead of leaving narrow shards.
#[test]
fn thinking_survives_resize_round_trip() {
    let text = "The second command was blocked by a guard because of `curl ... | python3`? \
            Weird, the first one worked. Let me avoid pipes into interpreters and write to a file instead";
    let norm = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let row_text = |rows: &[Line<'static>]| {
        rows.iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
    };
    // Live accumulation at the old width, mirroring `stream_thinking`:
    // `\n`-drains store terminated, word-cuts concatenate bare (which for
    // `\n`-free text reproduces the source exactly).
    let accumulate = |w_live: usize| {
        let max_live = w_live.saturating_sub(4).max(1);
        let mut run = String::new();
        let mut rest: Vec<char> = text.chars().collect();
        while !rest.is_empty() {
            let s: String = rest.iter().collect();
            if display_width(&s) < max_live {
                run.push_str(&s);
                break;
            }
            let cut = word_flush_cut(&rest, max_live);
            assert!(cut > 0, "cut must progress");
            run.push_str(&rest[..cut].iter().collect::<String>());
            rest = rest[cut..].to_vec();
        }
        run
    };
    for (w_live, w_new) in [(100usize, 40usize), (40usize, 100usize)] {
        let run = accumulate(w_live);
        let rows = thinking_run_rows(&run, w_new.saturating_sub(2).max(1), false);
        assert!(!rows.is_empty(), "run must paint at {w_new}");
        // Render budget is w-2 with the 1-cell left pad on top: padded
        // rows may reach w-1 but never the full width (Paragraph would
        // hard-clip anything wider — the "shortened" symptom).
        for r in &rows {
            let rw: usize = r.spans.iter().map(|s| s.width()).sum();
            assert!(
                rw <= w_new.saturating_sub(1).max(1),
                "row overflows {w_new}: {r:?}"
            );
        }
        let got = row_text(&rows)
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(got, norm, "text lost resizing {w_live} -> {w_new}");
    }
    // The reported bug: streamed narrow (40), widened to 100 — rows must
    // expand past the narrow cut budget instead of lingering as shards.
    let narrow_rows = thinking_run_rows(&accumulate(40), 40usize - 2, false);
    let wide_rows = thinking_run_rows(&accumulate(40), 100usize - 2, false);
    let max_cells = |rows: &[Line<'static>]| {
        rows.iter()
            .map(|r| r.spans.iter().map(|s| s.width()).sum::<usize>())
            .max()
            .unwrap_or(0)
    };
    assert!(
        wide_rows.len() < narrow_rows.len(),
        "widening must re-join rows: {} vs {}",
        wide_rows.len(),
        narrow_rows.len()
    );
    assert!(
        max_cells(&wide_rows) > 40 - 2,
        "widened rows must exceed the narrow budget: {wide_rows:?}"
    );
}

#[test]
fn thinking_run_rows_collapse_stacked_blanks() {
    // Live parity: blank-on-blank never paints, at run start (blank tail
    // from the opening gap) or mid-run (provider `\n\n` breaks).
    let rows = thinking_run_rows("\nfoo\n\n\nbar\n", 40, true);
    let texts: Vec<String> = rows
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    // Live blank rows carry the 1-space left pad (`" "`); the blank
    // predicate treats them as blank, so no stacked gaps ever paint.
    assert_eq!(
        texts,
        vec![" foo".to_string(), " ".to_string(), " bar".to_string()]
    );
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

// Over-cap push evicts oldest-first, mirroring the transcript
// >1000/drain-100 guard.
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

// At-cap is a no-op.
#[test]
fn history_entries_cap_keeps_at_most_1000_unrun() {
    let mut entries: Vec<crate::composer::TranscriptEntry> = (0..1000)
        .map(crate::composer::TranscriptEntry::Gap)
        .collect();
    cap_history_entries(&mut entries);
    assert_eq!(entries.len(), 1000);
}

#[test]
fn thinking_blank_never_stacks_on_blank() {
    // Sibling `stream()` parity: a drained blank must not stack on a
    // trailing blank, or provider `\n\n` paragraph breaks paint the
    // double gap from the report (`[gap][gap AGAIN]`).
    // Pure drain simulation: `stream_thinking` pushes every drained line
    // via `push_line_styled` (needs a TTY `Tui`), so mirror its exact
    // guard here — `trimmed` blank + blank tail means skip.
    let tail_blank = Line::from("");
    let tail_text = Line::from(vec![Span::styled("hello".to_string(), thinking_style())]);
    let drained = "\n\nhello\n\n";
    // blank tail: leading `\n\n` skipped, `hello` + one trailing blank
    // land (2 rows). text tail: the two leading blanks and `hello` land,
    // the single trailing blank lands too — but the second trailing blank
    // would stack on it, so it skips (3 rows, never a stacked pair).
    for (tail, want_rows) in [(&tail_blank, 2usize), (&tail_text, 3usize)] {
        let mut transcript = vec![tail.clone()];
        let mut pending = drained.to_string();
        let mut pushed = 0usize;
        while let Some(idx) = pending.find('\n') {
            let line: String = pending.drain(..=idx).collect();
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            if trimmed.trim().is_empty() && transcript.last().is_some_and(transcript_row_is_blank) {
                continue;
            }
            transcript.push(Line::from(vec![Span::styled(
                trimmed.to_string(),
                thinking_style(),
            )]));
            pushed += 1;
        }
        assert_eq!(pushed, want_rows, "tail={tail:?}");
        let blanks = transcript
            .windows(2)
            .filter(|w| transcript_row_is_blank(&w[0]) && transcript_row_is_blank(&w[1]))
            .count();
        assert_eq!(blanks, 0, "stacked blanks: {transcript:?}");
    }
}

#[test]
fn transcript_row_is_blank_matches_gap_predicates() {
    // The shared predicate must agree with `ensure_gap` /
    // `transcript_ends_blank`, or the live drain and the gap logic drift.
    assert!(transcript_row_is_blank(&Line::from("")));
    assert!(transcript_row_is_blank(&Line::from(" ")));
    assert!(transcript_row_is_blank(&Line::from(vec![Span::raw(" ")])));
    assert!(!transcript_row_is_blank(&Line::from("text")));
    let bg = Style::default().bg(crate::theme::theme().surface_bg);
    assert!(!transcript_row_is_blank(&Line::from("").style(bg)));
    assert!(!transcript_row_is_blank(&Line::from(vec![Span::styled(
        "".to_string(),
        Style::default().bg(crate::theme::theme().surface_bg)
    )])));
}
