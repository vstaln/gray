use super::*;

#[test]
fn format_matches_cat_n_layout() {
    assert_eq!(prefix_line(1, "hi"), "     1\thi");
    assert_eq!(prefix_line(412, "foo"), "   412\tfoo");
}

#[test]
fn width_never_truncates_large_numbers() {
    assert_eq!(prefix_line(1_000_000, "x"), "1000000\tx");
    assert_eq!(prefix_line(12_345_678, "x"), "12345678\tx");
}

fn wlines(n: usize) -> Vec<WindowLine> {
    (1..=n)
        .map(|i| WindowLine {
            text: format!("line {i:04}"),
            overflow_chars: 0,
        })
        .collect()
}

#[test]
fn ceilings_default_to_spec_values() {
    assert_eq!(MAX_LINES, 2000);
    assert_eq!(MAX_BYTES, 50 * 1024);
    assert_eq!(MAX_LINE_CHARS, 2000);
}

#[test]
fn exactly_filled_window_is_complete_without_peek_hit() {
    // T2.2 core: 2000 lines, file ends there (peek saw EOF) → no cut.
    let lines = wlines(2000);
    let w = window(1, &lines, MAX_LINES, MAX_BYTES, MAX_LINE_CHARS, false);
    assert_eq!(w.cut, None);
    assert_eq!(w.next_offset, None);
    assert_eq!(w.shown.len(), 2000);
    assert_eq!(w.shown[0], "     1\tline 0001");
    assert_eq!(w.shown[1999], "  2000\tline 2000");
    // Same window with one observed line past it → line cut at 2001.
    let w = window(1, &lines, MAX_LINES, MAX_BYTES, MAX_LINE_CHARS, true);
    assert_eq!(w.cut, Some(Cut::Lines));
    assert_eq!(w.next_offset, Some(2001));
    assert_eq!(w.shown.len(), 2000);
}

#[test]
fn over_supplied_window_cuts_without_peek() {
    // More lines observed than the window holds: cut regardless of peek.
    let lines = wlines(2005);
    let w = window(1, &lines, MAX_LINES, MAX_BYTES, MAX_LINE_CHARS, false);
    assert_eq!(w.cut, Some(Cut::Lines));
    assert_eq!(w.next_offset, Some(2001));
    assert_eq!(w.shown.len(), 2000);
}

#[test]
fn byte_cut_stops_before_the_unshown_line() {
    // 300-byte lines with prefixes (~307B): the first line that does not
    // fit is named, never emitted.
    let lines: Vec<WindowLine> = (1..=500)
        .map(|i| WindowLine {
            text: format!("{:<300}", format!("log line {i:04} ")),
            overflow_chars: 0,
        })
        .collect();
    let w = window(1, &lines, MAX_LINES, MAX_BYTES, MAX_LINE_CHARS, false);
    assert_eq!(w.cut, Some(Cut::Bytes));
    let next = w.next_offset.unwrap();
    assert!(w.shown.len() < 500);
    assert_eq!(next, w.shown.len() + 1, "resume ON the unshown line");
    assert!(w.shown[0].starts_with("     1\tlog line 0001"));
    assert_eq!(w.clamped, 0);
}

#[test]
fn byte_budget_exact_fit_is_complete() {
    // Two lines sized so prefixed bytes land exactly on the budget.
    let mk = |s: &str| WindowLine {
        text: s.to_string(),
        overflow_chars: 0,
    };
    let first = prefix_line(1, "a");
    let second = prefix_line(2, "b");
    let budget = first.len() + 1 + second.len();
    let w = window(
        1,
        &[mk("a"), mk("b")],
        MAX_LINES,
        budget,
        MAX_LINE_CHARS,
        false,
    );
    assert_eq!(w.cut, None);
    assert_eq!(w.shown.len(), 2);
    let w = window(
        1,
        &[mk("a"), mk("b")],
        MAX_LINES,
        budget - 1,
        MAX_LINE_CHARS,
        false,
    );
    assert_eq!(w.cut, Some(Cut::Bytes));
    assert_eq!(w.next_offset, Some(2));
    assert_eq!(w.shown.len(), 1);
}

#[test]
fn clamp_counts_shown_lines_only() {
    // A long line past the byte cut is read but unshown: it must not
    // inflate the clamp note about the visible output.
    let long = "y".repeat(3900);
    let lines = vec![
        WindowLine {
            text: "ok".to_string(),
            overflow_chars: 0,
        },
        WindowLine {
            text: long,
            overflow_chars: 0,
        },
    ];
    // Budget fits the first prefixed line but not the clamped second.
    let budget = prefix_line(1, "ok").len();
    let w = window(1, &lines, MAX_LINES, budget, MAX_LINE_CHARS, false);
    assert_eq!(w.cut, Some(Cut::Bytes));
    assert_eq!(w.next_offset, Some(2));
    assert_eq!(w.clamped, 0, "unshown clamped line is not counted");
    // Room for both: the shown clamp counts once.
    let w = window(1, &lines, MAX_LINES, MAX_BYTES, MAX_LINE_CHARS, false);
    assert_eq!(w.cut, None);
    assert_eq!(w.clamped, 1);
    assert!(w.shown[1].ends_with("…[+1900 chars]"));
}

#[test]
fn overflow_chars_keep_the_clamp_marker_exact() {
    // 5000-emoji line: the stream buffers a prefix and counts the rest.
    // Total 5000 chars → marker says +3000.
    let kept_text = "😀".repeat(2000);
    let w = window(
        1,
        &[WindowLine {
            text: format!("{kept_text}�"),
            overflow_chars: 2999,
        }],
        MAX_LINES,
        MAX_BYTES,
        MAX_LINE_CHARS,
        false,
    );
    assert_eq!(w.cut, None);
    assert_eq!(w.clamped, 1);
    assert_eq!(w.shown[0], format!("     1\t{kept_text} …[+3000 chars]"));
}
