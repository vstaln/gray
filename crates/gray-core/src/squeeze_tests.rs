//! The compression rules, checked against the output shapes they claim to know
//! and against the shapes they must not touch.

use crate::squeeze::*;

/// A `cargo build` over a 300-crate tree: the bulk is progress, the payload is
/// one error line and the summary.
fn cargo_build() -> String {
    let mut out = String::new();
    for i in 0..300 {
        out.push_str(&format!("   Compiling crate-{i} v0.1.{i}\n"));
    }
    out.push_str("warning: unused variable: `x`\n");
    out.push_str("error[E0308]: mismatched types\n");
    out.push_str("   Compiling last-one v0.9.0\n");
    out.push_str("    Finished dev [unoptimized + debuginfo] target(s) in 42.19s\n");
    out
}

#[test]
fn cargo_collapses_progress_and_keeps_the_error() {
    let s = squeeze(&cargo_build(), "cargo build --release");
    assert_eq!(s.rule, "cargo");
    assert!(s.squeezed());
    // The progress run is one counted line, not 302 of them.
    assert!(s.text.contains("progress ×30"), "got: {}", s.text);
    assert!(s.text.contains("error[E0308]"), "got: {}", s.text);
    assert!(s.text.contains("Finished dev"), "got: {}", s.text);
    // Fewer than a tenth of the original: this is the whole point.
    assert!(
        s.sent_bytes * 10 < s.raw_bytes,
        "{} -> {}",
        s.raw_bytes,
        s.sent_bytes
    );
}

#[test]
fn two_progress_lines_are_a_list_not_a_count() {
    let text = format!(
        "   Compiling a v0.1.0\n   Compiling b v0.1.0\n{}",
        "x".repeat(3000)
    );
    let s = squeeze(&text, "cargo check");
    assert!(
        !s.text.contains("×"),
        "two lines must not collapse: {}",
        s.text
    );
}

#[test]
fn a_mixed_pipeline_gets_only_the_generic_rule() {
    // grep's output is not cargo's output; a family rule here would be wrong.
    let mut text = String::new();
    for _ in 0..200 {
        text.push_str("src/a.rs:12:   Compiling thing v0.1.0\n");
    }
    let s = squeeze(&text, "cargo build 2>&1 | grep Compiling");
    assert_eq!(s.rule, "runs");
    // The generic rule fires: identical consecutive lines, counted.
    assert!(s.text.contains("×200"), "got: {}", s.text);
}

#[test]
fn npm_tree_becomes_a_count_and_the_summary_stays() {
    let mut text = String::new();
    for i in 0..400 {
        text.push_str(&format!("+ package-{i}@1.0.0\n"));
    }
    text.push_str("added 412 packages in 6s\n");
    let s = squeeze(&text, "npm install");
    assert_eq!(s.rule, "npm");
    assert!(s.text.contains("dependency tree ×400"), "got: {}", s.text);
    assert!(s.text.contains("added 412 packages"), "got: {}", s.text);
}

#[test]
fn pip_double_names_every_package() {
    let mut text = String::new();
    for i in 0..200 {
        text.push_str(&format!("Collecting pkg{i}\n"));
    }
    text.push_str("Installing collected packages: a, b, c, d, e\n");
    text.push_str("Successfully installed a b c d e\n");
    let s = squeeze(&text, "pip install -r requirements.txt");
    assert_eq!(s.rule, "pip");
    assert!(s.text.contains("package chatter ×200"), "got: {}", s.text);
    assert!(
        s.text.contains("[pip] installing 5 collected packages"),
        "got: {}",
        s.text
    );
    assert!(s.text.contains("Successfully installed"), "got: {}", s.text);
}

#[test]
fn pytest_progress_dots_are_not_content() {
    let mut text = String::new();
    for pct in 1..=50 {
        text.push_str(&format!("{:.<60} [ {:>3}%]\n", ".".repeat(40), pct * 2));
    }
    text.push_str("= 1 failed, 99 passed in 3.2s =\n");
    let s = squeeze(&text, "pytest -x tests/");
    assert_eq!(s.rule, "pytest");
    assert!(s.text.contains("progress ×50"), "got: {}", s.text);
    assert!(s.text.contains("1 failed, 99 passed"), "got: {}", s.text);
}

#[test]
fn unknown_command_still_gets_the_generic_rule() {
    let mut text = String::new();
    for _ in 0..500 {
        text.push_str("waiting for the thing to happen\n");
    }
    let s = squeeze(&text, "./some-script --go");
    assert_eq!(s.rule, "runs");
    assert!(s.text.contains("×500"), "got: {}", s.text);
}

#[test]
fn small_output_is_never_touched() {
    let text = "one\n".repeat(100);
    let s = squeeze(&text, "cargo build");
    assert!(!s.squeezed());
    assert_eq!(s.text, text);
}

#[test]
fn a_compressor_never_returns_more_than_it_was_given() {
    // Nothing to collapse: the output must pass through untouched, not grow a
    // note for a rule that did nothing.
    let text = format!(
        "{}\n{}",
        "unique line of its own here", "another distinct one"
    );
    let text = format!("{text}{}", " ".repeat(4000));
    let s = squeeze(&text, "cargo build");
    assert!(s.sent_bytes <= s.raw_bytes);
    assert!(!s.squeezed());
}

#[test]
fn the_escape_hatch_is_an_environment_variable() {
    let text = cargo_build();
    // SAFETY: single-threaded test body; the var is read once, right here.
    unsafe { std::env::set_var("GRAY_NO_SQUEEZE", "1") };
    let s = squeeze(&text, "cargo build");
    unsafe { std::env::remove_var("GRAY_NO_SQUEEZE") };
    assert!(!s.squeezed());
    assert_eq!(s.text, text);
}

#[test]
fn a_missing_trailing_newline_stays_missing() {
    let mut text = String::new();
    for _ in 0..100 {
        text.push_str("same line over and over\n");
    }
    text.pop();
    let s = squeeze(&text, "ls -R /usr");
    assert!(s.squeezed());
    assert!(!s.text.ends_with('\n'), "got: {:?}", s.text);
}

#[test]
fn a_trailing_newline_is_preserved() {
    let mut text = String::new();
    for _ in 0..100 {
        text.push_str("same line over and over\n");
    }
    let s = squeeze(&text, "ls -R /usr");
    assert!(s.squeezed());
    assert!(s.text.ends_with('\n'));
}

#[test]
fn blank_floods_collapse_to_a_count() {
    // Over the 2 KiB gate, so the rule is allowed to cost a line.
    let mut text = String::new();
    for _ in 0..3000 {
        text.push('\n');
    }
    text.push_str("tail line\n");
    let s = squeeze(&text, "cat big.txt");
    assert!(s.squeezed(), "nothing squeezed");
    assert_eq!(s.text, "[3000 blank lines]\ntail line\n");
}

#[test]
fn the_pip_rewrite_never_leaks_into_another_command() {
    let line = "Installing collected packages: a, b, c, d\n";
    let text = format!("{line}{}", "x".repeat(3000));
    let s = squeeze(&text, "grep Installing /var/log/dpkg.log");
    assert!(
        !s.text.contains("[pip] installing"),
        "a non-pip command must keep its lines: {}",
        s.text
    );
}
