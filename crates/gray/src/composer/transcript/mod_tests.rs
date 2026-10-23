use super::rows::RIGHT_MARGIN;
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
fn word_flush_cut_boundary_space_rides_with_the_row() {
    let chars: Vec<char> = "hi you  there".chars().collect();
    let cut = word_flush_cut(&chars, 6);
    let rest: String = chars[cut..].iter().collect();
    assert_eq!(rest, "there");
}

#[test]
fn thinking_paint_text_trims_only_word_cut_continuations() {
    assert_eq!(
        thinking_paint_text(" changed and", true),
        Some("changed and")
    );
    assert_eq!(thinking_paint_text("  - nested", false), Some("  - nested"));

    assert_eq!(thinking_paint_text(" ", true), None);
    assert_eq!(thinking_paint_text("", true), None);
    assert_eq!(thinking_paint_text("", false), Some(""));
}

#[test]
fn live_thinking_rows_never_open_with_a_space() {
    let text = "Given the ambiguity, the most useful interpretation: they want to know what \
                PR #166 actually changed and whether it's in the rebuilt binary. Since I \
                verified margins symbols = 2 in the binary, #166 IS in.";
    for max_w in 10..60 {
        let mut pending = String::new();
        let mut run = String::new();
        let mut painted: Vec<String> = Vec::new();
        let flush = |frag: &str, run: &mut String, painted: &mut Vec<String>| {
            let mid = !run.is_empty() && !run.ends_with('\n');
            run.push_str(frag);
            if let Some(t) = thinking_paint_text(frag, mid) {
                painted.push(t.to_string());
            }
        };

        for tok in text.split_inclusive(' ') {
            let tok = match tok.strip_suffix(' ') {
                Some(t) => format!(" {t}"),
                None => format!(" {tok}"),
            };
            let tok = if pending.is_empty() && run.is_empty() {
                tok.trim_start().to_string()
            } else {
                tok
            };
            gray_core::event::append_thinking_chunk(&mut pending, &tok);
            if display_width(&pending) > max_w {
                let chars: Vec<char> = pending.chars().collect();
                let cut = word_flush_cut(&chars, max_w);
                let line: String = chars[..cut].iter().collect();
                pending = chars[cut..].iter().collect();
                flush(&line, &mut run, &mut painted);
            }
        }
        if !pending.is_empty() {
            let rest = std::mem::take(&mut pending);
            flush(&rest, &mut run, &mut painted);
        }
        for row in &painted {
            assert!(
                !row.starts_with(' '),
                "row opens with a space at {max_w}: {row:?}"
            );
        }
        assert_eq!(run, text, "stored run must round-trip at {max_w}");
    }
}

#[test]
fn word_flush_cut_budgets_cells_not_chars() {
    let chars: Vec<char> = "界界界界".chars().collect();

    assert_eq!(word_flush_cut(&chars, 5), 2);
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

#[test]
fn unboundaryed_punctuation_only_chunks_are_held_not_rendered() {
    assert!(should_hold_stream_chunk(0, "."));
    assert!(should_hold_stream_chunk(0, "…"));

    assert!(should_hold_stream_chunk(1, "."));
    assert!(should_hold_stream_chunk(3, " "));
    assert!(should_hold_stream_chunk(3, "\n"));

    assert!(!should_hold_stream_chunk(0, "I'll build it"));
    assert!(!should_hold_stream_chunk(0, ""));

    assert!(!should_hold_stream_chunk(0, "x"));
}

#[test]
fn user_prompt_wraps_at_word_boundaries() {
    let text = "write a very long poem about the restless sea";
    let lines = format_user_prompt_lines(text, &[], 24);

    let bodies: Vec<String> = lines
        .iter()
        .filter_map(|l| l.spans.get(1))
        .map(|s| s.content.to_string())
        .collect();
    assert!(bodies.len() > 1);
    assert_eq!(bodies.concat(), text);

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
    let lines = format_user_prompt_lines(text, &[], 6);
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

    assert_eq!(lines.len(), 7, "{lines:?}");

    assert_eq!(row_w(&lines[3]), 80);
    assert_eq!(row_w(&lines[4]), 80);

    assert!(row_w(&lines[5]) < 80);
}

#[test]
fn full_width_numbered_row_is_not_rewrapped() {
    let width = 80;
    let content = "word ".repeat(14);
    let row = format!("    1 | {content}");
    assert_eq!(row.len(), width - 2);
    let lines = format_tool_box_lines(Line::from("Ran x"), &[Line::from(row)], width);

    assert_eq!(lines.len(), 4, "{lines:?}");
}

#[test]
fn wrap_ranges_round_trip_and_identity() {
    let short = Line::from(vec![Span::raw("hello world")]);
    let out = wrap_styled_line_with_ranges(short, 20);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1.end, usize::MAX);

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

    let accumulate = |w_live: usize| {
        let max_live = prose_width(w_live);
        let mut run = String::new();
        let mut rest: Vec<char> = text.chars().collect();
        while !rest.is_empty() {
            let s: String = rest.iter().collect();
            if display_width(&s) <= max_live {
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
        let rows = thinking_run_rows(&run, prose_width(w_new), false);
        assert!(!rows.is_empty(), "run must paint at {w_new}");

        for r in &rows {
            let rw: usize = r.spans.iter().map(|s| s.width()).sum();
            assert!(rw <= w_new - RIGHT_MARGIN, "row overflows {w_new}: {r:?}");
        }
        let got = row_text(&rows)
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(got, norm, "text lost resizing {w_live} -> {w_new}");
    }

    let narrow_rows = thinking_run_rows(&accumulate(40), prose_width(40), false);
    let wide_rows = thinking_run_rows(&accumulate(40), prose_width(100), false);
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
        max_cells(&wide_rows) > 40 - RIGHT_MARGIN,
        "widened rows must exceed the narrow budget: {wide_rows:?}"
    );
}

#[test]
fn thinking_run_rows_collapse_stacked_blanks() {
    let rows = thinking_run_rows("\nfoo\n\n\nbar\n", 40, true);
    let texts: Vec<String> = rows
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();

    let pad = " ".repeat(GUTTER);
    assert_eq!(
        texts,
        vec![format!("{pad}foo"), pad.clone(), format!("{pad}bar")]
    );
}

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
    let tail_blank = Line::from("");
    let tail_text = Line::from(vec![Span::styled("hello".to_string(), thinking_style())]);
    let drained = "\n\nhello\n\n";

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

#[test]
fn tool_box_has_exactly_one_painted_margin_row_top_and_bottom() {
    let bg = crate::theme::theme().surface_bg;
    let row_w = |l: &Line<'static>| l.spans.iter().map(|s| s.width()).sum::<usize>();
    let painted = |l: &Line<'static>| {
        l.spans.iter().all(|s| s.style.bg == Some(bg)) && l.style.bg == Some(bg)
    };
    for body in [
        Vec::new(),
        vec![Line::from("1 | exit 0"), Line::from("2 | done")],
    ] {
        let lines = format_tool_box_lines(Line::from("Ran cargo fmt"), &body, 80);
        let first = &lines[0];
        let last = &lines[lines.len() - 1];
        assert_eq!(row_w(first), 80, "top margin spans the card");
        assert_eq!(row_w(last), 80, "bottom margin spans the card");
        assert!(painted(first) && painted(last), "{first:?} {last:?}");

        assert!(lines[1].spans.iter().any(|s| !s.content.trim().is_empty()));
        assert!(
            lines[lines.len() - 2]
                .spans
                .iter()
                .any(|s| !s.content.trim().is_empty())
        );
    }
}

#[test]
fn prose_and_card_glyphs_share_the_gutter_column() {
    let first_glyph = |l: &Line<'static>| {
        let flat: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
        flat.chars().take_while(|c| *c == ' ').count()
    };
    let prose = thinking_run_rows("hello world", prose_width(80), false);
    assert_eq!(first_glyph(&prose[0]), GUTTER);
    let card = format_tool_box_lines(Line::from("● Ran cargo fmt"), &[], 80);
    assert_eq!(first_glyph(&card[1]), GUTTER);
    let prompt = format_user_prompt_lines("rebuilt?", &[], 80);
    assert_eq!(first_glyph(&prompt[1]), GUTTER);

    let long = format_user_prompt_lines(&"word ".repeat(40), &[], 40);
    assert_eq!(first_glyph(&long[2]), GUTTER + 2);
    for row in &long {
        let w: usize = row.spans.iter().map(|s| s.width()).sum();
        assert!(w <= 40, "prompt row overflows: {row:?}");
    }
}

#[test]
fn the_backdrop_keeps_the_live_margins() {
    let styled = |rows: &[&str]| TranscriptEntry::StyledLines {
        lines: rows.iter().map(|r| Line::from(r.to_string())).collect(),
        hyperlinks: Vec::new(),
    };
    let entries = vec![
        TranscriptEntry::Welcome,
        TranscriptEntry::Gap(1),
        TranscriptEntry::UserPrompt("hi".into(), Vec::new()),
        TranscriptEntry::Gap(1),
        TranscriptEntry::Gap(1),
        styled(&["", "a", ""]),
        styled(&["", "b"]),
        TranscriptEntry::Gap(1),
        TranscriptEntry::ToolBox {
            header: Line::from("Ran ls"),
            body: vec![Line::from("1 | ok")],
        },
        TranscriptEntry::Gap(1),
    ];
    let welcome = crate::composer::build_welcome_lines(80).len();
    let rows = layout_history(&entries, 80, 24);
    let body = &rows[welcome..];
    assert!(
        !transcript_row_is_blank(&body[0]),
        "the welcome's gap is the gap"
    );
    for pair in body.windows(2) {
        assert!(
            !(transcript_row_is_blank(&pair[0]) && transcript_row_is_blank(&pair[1])),
            "two blank rows in {body:?}"
        );
    }
    assert!(!transcript_row_is_blank(rows.last().unwrap()));
    let texts: Vec<String> = body.iter().map(|l| l.to_string()).collect();
    let a = texts.iter().position(|t| t == "a").unwrap();
    assert_eq!(texts[a + 1], "", "one gap between a and b");
    assert_eq!(texts[a + 2], "b");
}

#[test]
fn user_prompt_renders_attachment_markers_as_chips() {
    let lines = format_user_prompt_lines("look [image: shot.png] here", &[], 60);
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(!text.contains("[image:"), "{text}");
    assert!(text.contains(" IMAGE "), "{text}");
    assert!(text.contains("shot.png"), "{text}");
}

#[test]
fn user_prompt_tiles_attached_images_side_by_side() {
    let dir = tempfile::tempdir().expect("tempdir");
    let red = ratatui::style::Color::Rgb(255, 0, 0);
    let green = ratatui::style::Color::Rgb(0, 255, 0);
    let paths: Vec<std::path::PathBuf> = [("a.png", [255, 0, 0, 255]), ("b.png", [0, 255, 0, 255])]
        .into_iter()
        .map(|(name, px)| {
            let p = dir.path().join(name);
            ::image::RgbaImage::from_pixel(8, 8, ::image::Rgba(px))
                .save(&p)
                .expect("write png");
            p
        })
        .collect();
    let has = |l: &Line<'static>, c| l.spans.iter().any(|s| s.style.fg == Some(c));

    let wide = format_user_prompt_lines("look", &paths, 200);
    assert!(
        wide.iter().any(|l| has(l, red) && has(l, green)),
        "both previews share a row when they fit"
    );
    let row = wide.iter().find(|l| has(l, red) && has(l, green)).unwrap();
    let first_green = row
        .spans
        .iter()
        .position(|s| s.style.fg == Some(green))
        .unwrap();
    assert_eq!(
        row.spans[first_green - 1].content.as_ref(),
        "  ",
        "gap between previews"
    );

    let narrow = format_user_prompt_lines("look", &paths, 40);
    assert!(
        !narrow.iter().any(|l| has(l, red) && has(l, green)),
        "too wide to share a row"
    );
    let last_red = narrow.iter().rposition(|l| has(l, red)).unwrap();
    let first_green = narrow.iter().position(|l| has(l, green)).unwrap();
    assert!(
        first_green > last_red + 1,
        "a blank row separates stacked bands"
    );
}

#[test]
fn viewed_image_lines_preview_read_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let img = dir.path().join("slide.png");
    ::image::RgbaImage::from_pixel(8, 8, ::image::Rgba([255, 0, 0, 255]))
        .save(&img)
        .expect("write png");
    let args = serde_json::json!({"path": "slide.png"});
    let rows = super::rows::viewed_image_lines("read", Some(&args), dir.path(), 40);
    assert!(rows.is_some(), "read on an image should preview");
}

#[test]
fn viewed_image_lines_preview_bash_cat() {
    let dir = tempfile::tempdir().expect("tempdir");
    let img = dir.path().join("slide.png");
    ::image::RgbaImage::from_pixel(8, 8, ::image::Rgba([0, 255, 0, 255]))
        .save(&img)
        .expect("write png");
    let cmd = format!("cat '{}'", img.display());
    let args = serde_json::json!({"command": cmd});
    let rows = super::rows::viewed_image_lines("bash", Some(&args), dir.path(), 40);
    assert!(rows.is_some(), "cat image.png should preview");
}

#[test]
fn viewed_image_lines_skip_non_image_bash() {
    let dir = tempfile::tempdir().expect("tempdir");
    let args = serde_json::json!({"command": "cat notes.txt"});
    assert!(super::rows::viewed_image_lines("bash", Some(&args), dir.path(), 40).is_none());
    let args = serde_json::json!({"command": "cat x.png | head"});
    assert!(super::rows::viewed_image_lines("bash", Some(&args), dir.path(), 40).is_none());
}

#[test]
fn split_synthetic_prompt_parts_notifications() {
    let (visible, notices) = split_synthetic_prompt("plain question");
    assert_eq!(visible, "plain question");
    assert!(notices.is_empty());

    let (visible, notices) = split_synthetic_prompt(
        "↳ from cli\nhi\n\n[Background task notification]\nBackground job a finished (exit 0) after 1s · log /tmp/x\ntail it\n\n[Background task notification]\nodd notice\nrest",
    );
    assert_eq!(visible, "↳ from cli\nhi");
    assert_eq!(
        notices,
        vec![
            "Background job a finished (exit 0) after 1s · log /tmp/x".to_string(),
            "odd notice".to_string(),
        ]
    );

    let (visible, notices) = split_synthetic_prompt(
        "[Background task notification]\nBackground job b finished (exit 1) after 2s",
    );
    assert!(visible.is_empty());
    assert_eq!(notices, vec!["Background job b finished (exit 1) after 2s"]);

    // cron deliveries collapse the same way, and an interleaved
    // `↳ from` send stays visible between notice segments
    let (visible, notices) = split_synthetic_prompt(
        "[Cron delivery: nightly]\nbuilt 3 crates ok\n\n↳ from cli\nhey\n\n[Background task notification]\nBackground job c finished (exit 0) after 3s",
    );
    assert_eq!(visible, "↳ from cli\nhey");
    assert_eq!(
        notices,
        vec![
            "built 3 crates ok".to_string(),
            "Background job c finished (exit 0) after 3s".to_string(),
        ]
    );
}

#[test]
fn wrapped_rows_keep_the_leading_indent() {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let text =
        "  plugin gray-gateway exited (exit 1) — restarted; see ~/.gray/logs/sidecar-stderr.log"
            .to_string();
    for max_w in [24usize, 40, 60] {
        let rows = wrap_styled_line(Line::from(Span::styled(text.clone(), dim)), max_w);
        assert!(rows.len() > 1, "expected a wrap at width {max_w}");
        for row in &rows {
            let row_text: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                row_text.starts_with("  "),
                "wrapped row lost its leading indent at width {max_w}: {row_text:?}"
            );
        }
    }
}

#[test]
fn wrapped_list_items_hang_under_their_text() {
    for (text, pad) in [
        (
            "• load_tools: one line per plugin tool, MCP servers grouped by name",
            "  ",
        ),
        (
            "  12. second level numbered item that keeps going past the edge",
            "      ",
        ),
    ] {
        let rows = wrap_styled_line(Line::from(text.to_string()), 30);
        assert!(rows.len() > 1);
        for row in &rows[1..] {
            let row_text: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(
                row_text.starts_with(pad) && !row_text[pad.len()..].starts_with(' '),
                "continuation not hung under the item text: {row_text:?}"
            );
            assert!(row.width() <= 30);
        }
    }
}

#[test]
fn split_synthetic_prompt_parts_subagent_and_nudge_segments() {
    // The subagents plugin's completion delivery: preamble is scaffolding,
    // the first report line becomes the dim notice.
    let (visible, notices) = split_synthetic_prompt(
        "Subagent finished — model output from a delegated run, NOT a message from the user. Verify before acting.\n  qa-r1 [completed] scout <swe-2-max> · run abc · 22s\n  report body line",
    );
    assert!(visible.is_empty());
    assert_eq!(
        notices,
        vec!["qa-r1 [completed] scout <swe-2-max> · run abc · 22s"]
    );

    // Agent-loop nudges replay as their own dim line, swallowing the rest
    // of the segment, and a following `↳ from` send still shows.
    let (visible, notices) = split_synthetic_prompt(
        "previous response truncated — continue exactly where you left off\n\n↳ from cli\nreal message",
    );
    assert_eq!(visible, "↳ from cli\nreal message");
    assert_eq!(
        notices,
        vec!["previous response truncated — continue exactly where you left off"]
    );

    for nudge in [
        "[Background jobs still running]\njob x is still running\nEnd your turn to be woken",
        "[Context update]\nsome hook context",
        "executed tool calls but returned empty — process results and continue",
        "no tool calls have run yet — if this task requires action, issue a real tool call now",
        "your last message contained raw tool-call markup instead of a real tool call, so nothing ran",
        "Before finishing, verify the deliverable: re-read the task spec",
        "Continue the pending user request using the retained context and summary.",
    ] {
        let (visible, notices) = split_synthetic_prompt(nudge);
        assert!(visible.is_empty(), "{nudge}");
        assert_eq!(notices.len(), 1, "{nudge}");
    }
}

#[test]
fn backdrop_tail_layout_matches_the_full_layout_tail() {
    let styled = |rows: &[&str]| TranscriptEntry::StyledLines {
        lines: rows.iter().map(|r| Line::from(r.to_string())).collect(),
        hyperlinks: Vec::new(),
    };
    let mut entries = vec![TranscriptEntry::Welcome];
    for i in 0..60 {
        entries.push(TranscriptEntry::UserPrompt(
            format!("prompt {i}"),
            Vec::new(),
        ));
        entries.push(TranscriptEntry::Gap(1));
        entries.push(styled(&["", &format!("reply {i}"), "more", ""]));
        entries.push(TranscriptEntry::Gap(1));
        entries.push(TranscriptEntry::ToolBox {
            header: Line::from(format!("Ran {i}")),
            body: vec![Line::from("ok")],
        });
    }
    let text = |rows: &[Line<'static>]| rows.iter().map(|l| l.to_string()).collect::<Vec<_>>();
    let full = text(&layout_history(&entries, 80, 24));
    for need in [1, 5, 24, 45, 120] {
        let tail = text(&layout_history_tail(&entries, 80, 24, need));
        assert!(tail.len() > need && tail.len() < full.len(), "need={need}");
        assert_eq!(
            tail[tail.len() - need..],
            full[full.len() - need..],
            "need={need}"
        );
    }
    let short = &entries[..4];
    assert_eq!(
        text(&layout_history_tail(short, 80, 24, 500)),
        text(&layout_history(short, 80, 24)),
        "a transcript shorter than the screen lays out whole"
    );
}

#[test]
fn push_user_prompt_skips_empty_submissions() {
    use crate::composer::Tui;
    let mut tui = Tui::test_tui(80, 24);
    tui.push_user_prompt("", &[], true);
    tui.push_user_prompt("  \n\t  ", &[], true);
    tui.push_user_prompt("\u{1b}[3~", &[], true);
    assert!(tui.history_entries.is_empty());
    assert!(tui.transcript.is_empty());
    tui.push_user_prompt("still here", &[], true);
    assert!(
        tui.transcript
            .iter()
            .any(|l| l.spans.iter().any(|s| s.content.contains("still here")))
    );
}

#[test]
fn repro_card_to_card_junction() {
    use crate::composer::Tui;
    let mut tui = Tui::test_tui(80, 24);
    tui.push_tool_box(
        Line::from(vec![Span::raw("● Ran rg -n foo".to_string())]),
        vec![Line::from("hit 1"), Line::from("hit 2")],
    );
    tui.push_tool_box(
        Line::from(vec![Span::raw("● Read print.rs".to_string())]),
        vec![Line::from("body a"), Line::from("body b")],
    );
    // Card-to-card junction: exactly one blank row separates the two boxes.
    let blanks: Vec<usize> = tui
        .transcript
        .iter()
        .enumerate()
        .filter_map(|(i, l)| transcript_row_is_blank(l).then_some(i))
        .collect();
    assert_eq!(blanks, [6], "junction rows: {blanks:?}");
}

#[test]
fn pending_stream_text_stays_before_prompt_after_reflow() {
    use crate::composer::Tui;
    let mut tui = Tui::test_tui(80, 24);
    tui.enable_reveal_pacing();
    tui.stream_text("streamed words go here");
    tui.push_user_prompt("what now?", &[], false);
    let text_of = |t: &Tui| -> Vec<String> {
        t.transcript
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    };
    let pos = |rows: &[String]| {
        (
            rows.iter().position(|r| r.contains("streamed words")),
            rows.iter().position(|r| r.contains("what now?")),
        )
    };
    let (md, pr) = pos(&text_of(&tui));
    assert!(
        md.is_some() && pr.is_some() && md < pr,
        "stream text must commit before the prompt"
    );
    tui.reflow_on_resize(100);
    let (md, pr) = pos(&text_of(&tui));
    assert!(
        md.is_some() && pr.is_some() && md < pr,
        "reflow swapped the blocks"
    );
}

#[test]
fn skill_box_margin() {
    use crate::composer::Tui;
    use ratatui::text::Line;
    let mut tui = Tui::test_tui(80, 24);
    tui.push_user_prompt("/skills be-ambitious", &[], false);
    let args = serde_json::json!({"name": "be-ambitious"});
    let header = crate::tool_fmt::format_tool_call_header("skill", &args, None, None);
    let body: Vec<Line> = vec![Line::from("body line one"), Line::from("body line two")];
    tui.push_tool_box(header, body);
    let _ = tui.draw();
    let text_of = |l: &Line| {
        l.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    };
    let body_rows: Vec<&Line> = tui
        .transcript
        .iter()
        .filter(|l| text_of(l).contains("body line"))
        .collect();
    assert_eq!(body_rows.len(), 2);
    for row in body_rows {
        assert!(
            text_of(row).starts_with(' '),
            "body row should carry a left gutter: {:?}",
            text_of(row)
        );
    }
}

#[test]
fn dock_shrink_pulls_transcript_down() {
    use crate::composer::Tui;
    let mut tui = Tui::test_tui(80, 24);
    for i in 0..20 {
        tui.push_dim(format!("line {i}"));
    }
    let _ = tui.draw();
    tui.set_status(Some("Working"));
    for i in 0..3 {
        tui.upsert_live_tool(
            &format!("t{i}"),
            vec![Line::from(format!("Ran c{i}"))],
            true,
        );
    }
    tui.queued_inputs.push_back(("q1".into(), Vec::new()));
    tui.queued_inputs.push_back(("q2".into(), Vec::new()));
    let _ = tui.draw();
    tui.set_status(None);
    for i in 0..3 {
        tui.remove_live_tool(&format!("t{i}"));
    }
    tui.queued_inputs.clear();
    let _ = tui.draw();
    let (vacated, _covered, _tail) = tui.terminal.debug_state();
    assert_eq!(vacated, 0, "dock shrink left vacated rows unfilled");
}

#[test]
fn a_growing_dock_scrolls_the_prompt_instead_of_covering_it() {
    use crate::composer::Tui;
    let mut tui = Tui::test_tui(80, 24);
    for i in 0..30 {
        tui.push_dim(format!("line {i}"));
    }
    tui.push_user_prompt("sent while busy", &[], false);
    let _ = tui.draw();
    tui.set_status(Some("Working"));
    tui.queued_inputs.push_back(("q1".into(), Vec::new()));
    let _ = tui.draw();
    let (_vacated, covered, _tail) = tui.terminal.debug_state();
    assert_eq!(covered, 0, "a one-line composer covered transcript rows");
}
