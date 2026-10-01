//! The report is the product: a health check whose output is unreadable, or
//! whose exit code lies, is worse than none.

use super::*;

/// A failure is the only thing that fails the command. Warnings (no model
/// picked yet, a stopped gateway) are states, and a setup script gating on
/// `gray doctor` must not trip over one.
#[test]
fn only_failures_set_the_exit_code() {
    assert_eq!(exit_code(&[]), 0);
    assert_eq!(
        exit_code(&[
            Check::new("model", Status::Pass, "m"),
            Check::new("gateway", Status::Warn, "stopped"),
            Check::new("provider call", Status::Skip, "not run"),
        ]),
        0
    );
    assert_eq!(
        exit_code(&[
            Check::new("model", Status::Pass, "m"),
            Check::new("provider key", Status::Fail, "no key"),
        ]),
        1
    );
}

/// The line format is what a user reads in a terminal and pastes into an
/// issue: aligned names, a fixed status column, one check per line.
#[test]
fn the_report_is_aligned_and_counts_the_outcome() {
    let report = render(&[
        Check::new("gray home", Status::Pass, "/root/.gray"),
        Check::new("provider key", Status::Fail, "no key for x"),
        Check::new("provider call", Status::Skip, "not run"),
    ]);
    let lines: Vec<&str> = report.lines().collect();
    assert_eq!(lines[0], "gray home      ok    /root/.gray");
    assert_eq!(lines[1], "provider key   FAIL  no key for x");
    assert_eq!(lines[2], "provider call  skip  not run");
    assert_eq!(lines[3], "1 failure(s), 0 warning(s)");
}

/// A clean report says so in words rather than printing nothing, so an empty
/// tail is never mistaken for a crash.
#[test]
fn a_clean_report_still_speaks() {
    let report = render(&[Check::new("shell", Status::Pass, "sh -c")]);
    assert!(report.trim_end().ends_with("0 warning(s), no failures"));
}
