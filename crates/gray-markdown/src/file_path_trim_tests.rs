use super::*;
use ratatui::text::Line;

fn single_path_targets(text: &str) -> Vec<HyperlinkTarget> {
    let line = Line::from(text);
    let (targets, _) = detect_file_paths(std::slice::from_ref(&line), &[], 0);
    targets
}

#[test]
#[ignore = "UNRUN: cargo test banned in X (amdgpu page-flip); run in TTY/CI"]
fn trailing_dot_trimmed_when_text_follows_in_span() {
    // The trim must inspect the char AT end_b (the '.'), not the span's
    // last char ('l'): previously the '.' survived whenever tail text
    // followed the path in the same span.
    let targets = single_path_targets("see /home/u/file. tail");
    assert_eq!(targets.len(), 1, "got: {targets:?}");
    assert_eq!(targets[0].url, "file:///home/u/file");
    assert_eq!(targets[0].column_range, 4..16);
}

#[test]
#[ignore = "UNRUN: cargo test banned in X (amdgpu page-flip); run in TTY/CI"]
fn trailing_dot_trimmed_at_span_end() {
    let targets = single_path_targets("see /home/u/file.");
    assert_eq!(targets.len(), 1, "got: {targets:?}");
    assert_eq!(targets[0].url, "file:///home/u/file");
    assert_eq!(targets[0].column_range, 4..16);
}
