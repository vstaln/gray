//! `gray spill` — the way back into a result the context had to leave out.
//!
//! The contract these check is the one the footer promises: numbered lines
//! against the original, a stated ceiling instead of a silent cut, and an
//! unknown handle as an error rather than an empty success.

use super::*;

const H: &str = "0123456789abcdef";

/// `$GRAY_HOME` is process-global: these two tests write through it, so they
/// take a lock rather than repointing each other's home mid-run.
static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn numbered(out: &str) -> Vec<(usize, &str)> {
    out.lines()
        .filter_map(|l| {
            let (num, rest) = l.split_once('\t')?;
            let n = num.trim().parse::<usize>().ok()?;
            Some((n, rest.strip_prefix("- ").unwrap_or(rest)))
        })
        .collect()
}

#[test]
fn head_numbers_from_the_start_of_the_original() {
    let text = "alpha\nbravo\ncharlie\n";
    let out = slice(H, text, 0, 50);
    assert_eq!(
        numbered(&out),
        vec![(1, "alpha"), (2, "bravo"), (3, "charlie")]
    );
}

#[test]
fn head_states_the_ceiling_instead_of_silently_cutting() {
    let text = (0..500)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let out = slice(H, &text, 0, 500);
    let shown = numbered(&out);
    assert_eq!(shown.len(), MAX_OUT_LINES, "prints the whole line budget");
    assert!(
        out.contains(&format!("showing lines 1-{} of 500", MAX_OUT_LINES)),
        "must say what it hid: {}",
        out.lines().last().unwrap_or_default()
    );
    assert!(out.contains("gray spill grep"), "must name the way back");
}

#[test]
fn tail_ends_at_the_last_line() {
    let text = "a\nb\nc\nd\ne\n";
    let out = slice(H, text, text.lines().count() - 2, 50);
    assert_eq!(numbered(&out), vec![(4, "d"), (5, "e")]);
    assert!(out.contains("lines 4-5 of 5"), "got: {out}");
}

#[test]
fn tail_beyond_the_end_is_a_position_not_a_panic() {
    let out = slice(H, "only\n", 900, 50);
    assert!(out.contains("that result has 1 line(s)"), "got: {out}");
}

#[test]
fn grep_finds_the_line_and_says_how_many_matched() {
    let text = "ok\nERROR: boom\nfine\n";
    let re = build("ERROR", false, false).expect("regex");
    let out = grep_lines(H, text, &re, 0, None);
    assert_eq!(numbered(&out), vec![(2, "ERROR: boom")]);
    assert!(out.contains("1 match(es)"), "got: {out}");
}

#[test]
fn grep_context_marks_the_neighbours() {
    let text = "a\nb\nERROR\nd\ne\nf\n";
    let re = build("ERROR", false, false).expect("regex");
    let out = grep_lines(H, text, &re, 1, None);
    assert_eq!(
        numbered(&out),
        vec![(2, "b"), (3, "ERROR"), (4, "d")],
        "got: {out}"
    );
    assert!(out.contains("\t- b"), "context lines are marked: {out}");
}

#[test]
fn grep_honours_the_limit_and_says_it_did() {
    let text = (0..50)
        .map(|i| format!("hit {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let re = build("hit", false, false).expect("regex");
    let out = grep_lines(H, &text, &re, 0, Some(5));
    assert_eq!(numbered(&out).len(), 5);
    assert!(out.contains("50 match(es), showing 5"), "got: {out}");
}

#[test]
fn a_literal_pattern_is_not_a_regex() {
    let re = build("a.c", false, true).expect("literal");
    let out = grep_lines(H, "abc\na.c\n", &re, 0, None);
    assert_eq!(numbered(&out), vec![(2, "a.c")]);
}

#[test]
fn ignore_case_folds() {
    let re = build("boom", true, false).expect("regex");
    let out = grep_lines(H, "BOOM\n", &re, 0, None);
    assert_eq!(numbered(&out).len(), 1);
}

#[test]
fn no_match_is_a_clear_answer_not_an_empty_one() {
    let re = build("needle", false, false).expect("regex");
    let out = grep_lines(H, "hay\nhay\n", &re, 0, None);
    assert!(out.contains("no match in the 2 lines stored"), "got: {out}");
    assert!(
        out.contains("gray spill head"),
        "must offer the ends: {out}"
    );
}

#[test]
fn a_bad_pattern_is_an_error_not_a_panic() {
    assert!(build("(unclosed", false, false).is_err());
}

#[test]
fn stats_reports_nothing_honestly_on_a_fresh_machine() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::TempDir::new().expect("tempdir");
    // SAFETY: the lock makes this the only thread touching the var while it is
    // set, and the body reads back only its own directory.
    unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
    let out = stats();
    unsafe { std::env::remove_var("GRAY_HOME") };
    assert_eq!(out, "nothing squeezed or spilled yet on this machine");
}

#[test]
fn stats_names_the_rules_that_earned_their_keep() {
    let _guard = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::TempDir::new().expect("tempdir");
    // SAFETY: as above — the lock covers the whole body.
    unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
    for (rule, raw, sent) in [("cargo", 30_000u64, 900u64), ("npm", 10_000, 2_000)] {
        gray_core::spill::record(gray_core::spill::MeterEvent {
            ts: 1,
            rule: rule.into(),
            raw,
            sent,
        });
    }
    let out = stats();
    unsafe { std::env::remove_var("GRAY_HOME") };
    assert!(out.contains("2 result(s) metered"), "got: {out}");
    assert!(out.contains("by rule:"), "got: {out}");
    assert!(out.contains("cargo"), "got: {out}");
    // Biggest saving first.
    let cargo = out.find("cargo").expect("cargo row");
    let npm = out.find("npm").expect("npm row");
    assert!(cargo < npm, "cargo saved more, so it comes first: {out}");
    assert!(out.contains("39.1 KiB produced"), "byte totals: {out}");
    let facts: Vec<&str> = out.lines().collect();
    assert!(
        facts.iter().any(|l| l.starts_with('\u{2248}')),
        "each fact gets its own line, not a run-on: {out}"
    );
    assert!(out.contains("36.2 KiB saved"), "byte totals: {out}");
}
