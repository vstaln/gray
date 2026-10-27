use super::ask_lines;
use crate::composer::AskModal;

fn row(k: &str, l: &str, t: &str) -> (String, String, String) {
    (k.to_string(), l.to_string(), t.to_string())
}

fn approval(subject: &[&str]) -> AskModal {
    let mut rows = vec![row("?", "Allow bash?", "Allow?")];
    rows.extend(subject.iter().map(|s| row(">", "", s)));
    rows.push(row("\u{2502}", "", "in /tmp/permtest"));
    rows.push(row(" ", "", ""));
    let cursor = rows.len();
    rows.push(row("1.", "Yes", "Run this once."));
    rows.push(row(
        "2.",
        "Yes, for session",
        "Remember it for this session.",
    ));
    rows.push(row("3.", "No", "Skip it; the turn continues."));
    rows.push(row(" ", "", ""));
    rows.push(row(
        "\u{b7}",
        "",
        "\u{2191}\u{2193} select \u{b7} 1-3 choose \u{b7} enter confirm \u{b7} esc cancel",
    ));
    AskModal { rows, cursor }
}

fn text(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines.iter().map(|l| l.to_string()).collect()
}

#[test]
fn approval_shows_title_subject_cwd_and_aligned_options() {
    let shown = text(&ask_lines(
        &approval(&["echo interactive-gate-ok"]),
        100,
        usize::MAX,
    ));
    assert_eq!(shown[0].trim(), "Allow bash?");
    assert!(shown[1].ends_with("echo interactive-gate-ok"), "{shown:?}");
    assert_eq!(shown[2].trim(), "in /tmp/permtest");
    let yes = shown.iter().find(|l| l.contains("Yes ")).unwrap();
    let no = shown.iter().find(|l| l.contains("3. No")).unwrap();
    assert!(yes.contains("\u{203a} 1. Yes"), "{yes}");
    // Descriptions start in one column.
    let col = |l: &str, pat: &str| l[..l.find(pat).unwrap()].chars().count();
    assert_eq!(col(yes, "Run this"), col(no, "Skip it"), "{shown:?}");
    assert!(
        !shown
            .iter()
            .any(|l| l.contains("Other") || l.contains("Recommended"))
    );
}

#[test]
fn a_long_command_wraps_whole_instead_of_being_cut() {
    let cmd = format!("printf '%s' {}", "x".repeat(150));
    let shown = text(&ask_lines(&approval(&[&cmd]), 60, usize::MAX));
    let joined: String = shown
        .iter()
        .filter(|l| l.contains('\u{258e}'))
        .map(|l| l.split_once('\u{258e}').unwrap().1.replace(' ', ""))
        .collect();
    assert_eq!(joined, cmd.replace(' ', ""), "{shown:?}");
}

#[test]
fn a_tall_subject_folds_in_the_middle_and_keeps_the_options() {
    let lines: Vec<String> = (1..=40).map(|i| format!("line {i}")).collect();
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let shown = text(&ask_lines(&approval(&refs), 80, 20));
    assert_eq!(shown.len(), 20, "{shown:?}");
    assert!(shown.iter().any(|l| l.contains("line 1")));
    assert!(shown.iter().any(|l| l.contains("line 40")));
    assert!(shown.iter().any(|l| l.contains("\u{2026} +")), "{shown:?}");
    assert!(shown.iter().any(|l| l.contains("3. No")), "{shown:?}");
    assert!(shown.last().unwrap().contains("esc cancel"), "{shown:?}");
}
