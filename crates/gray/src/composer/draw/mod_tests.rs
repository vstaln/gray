use super::*;

#[test]
fn status_dock_seam_is_dynamic() {
    assert_eq!(status_dock_h(false, true), 0);
    assert_eq!(status_dock_h(true, false), 2); // scrollback already blank: status + breath
    assert_eq!(status_dock_h(true, true), 3); // seam + status + breath
}

#[test]
fn transcript_ends_blank_matches_ensure_gap() {
    use ratatui::style::Style;
    assert!(!transcript_ends_blank(&[]));
    assert!(transcript_ends_blank(&[Line::from("")]));
    assert!(transcript_ends_blank(&[Line::from(" ")])); // left_pad-only row
    assert!(!transcript_ends_blank(&[Line::from("text")]));
    // card / code padding rows carry a bg: they are edges, not gaps
    let bg = Style::default().bg(crate::theme::GRAY_UI_THEME.surface_bg);
    assert!(!transcript_ends_blank(&[Line::from("").style(bg)]));
}

#[test]
fn desired_viewport_exact_fit() {
    // Idle: input 3 + footer 1 = 4 rows (MIN_VIEWPORT_H).
    assert_eq!(desired_viewport_h(0, 0, 3, 0, 0, VIEWPORT_H), 4);
    // Slash popup: input 3 + panel 6 + footer 1 = 10.
    assert_eq!(desired_viewport_h(0, 0, 3, 6, 0, VIEWPORT_H), 10);
    // Running: status 2 + input 3 + footer 1 = 6.
    assert_eq!(desired_viewport_h(2, 0, 3, 0, 0, VIEWPORT_H), 6);
    // Running + full panel: 3 + 3 + 6 + 1 = 13.
    assert_eq!(desired_viewport_h(3, 0, 3, 6, 0, VIEWPORT_H), 13);
    // Question panel: expands up to available screen height to show all options.
    assert_eq!(desired_viewport_h(0, 0, 0, 15, 0, 23), 16);
}

/// One streamed paragraph arriving chunk by chunk flips the transcript
/// tail blank / non-blank between frames. Driven through the exact path
/// `draw` uses: the viewport must hold still instead of bouncing 6<->7
/// rows per chunk (the reported input-box bounce).
#[test]
fn streaming_tail_flicker_holds_viewport_still() {
    let mut cached = false;
    let mut heights = Vec::new();
    for blank in [true, false, true, false, true] {
        cached = ratchet_seam(cached, true, !blank);
        heights.push(desired_viewport_h(
            status_dock_h(true, cached),
            0,
            3,
            0,
            0,
            VIEWPORT_H,
        ));
    }
    // One growth step when content first flows, then steady — never an
    // oscillation. Latch releases when the status clears.
    assert_eq!(heights, vec![6, 7, 7, 7, 7], "heights: {heights:?}");
    assert!(!ratchet_seam(true, false, true));
}

/// Checkpoint trailing gaps (`Thought for` spacer, tool-box trailing)
/// release the seam latch, so the gap never stacks with a latched seam
/// into a double blank above the live status. Streaming re-latches on
/// the next non-blank frame, so per-chunk flicker still holds steady.
#[test]
fn checkpoint_gap_releases_seam_to_single_spaced_status() {
    // thinking streams: tail non-blank latches the seam on.
    let mut seam = ratchet_seam(false, true, true);
    assert!(seam);
    // Thought summary commits + trailing spacer gap; the checkpoint
    // releases the latch (see `release_dock_seam` callers).
    seam = false;
    // status-only frames with a blank tail: no seam, status + breath.
    seam = ratchet_seam(seam, true, false);
    assert!(!seam, "seam must not stack on the checkpoint gap");
    assert_eq!(status_dock_h(true, seam), 2, "status + breath only");
    // answer streams: first non-blank row re-latches, height steady.
    seam = ratchet_seam(seam, true, true);
    assert!(seam);
    assert_eq!(status_dock_h(true, seam), 3, "seam + status + breath");
}

#[test]
fn queued_preview_renders_header_and_entries() {
    let mut q: std::collections::VecDeque<(String, Vec<std::path::PathBuf>)> =
        std::collections::VecDeque::new();
    assert!(queued_preview_lines(&q, 80).is_empty());
    q.push_back(("hello".to_string(), vec![]));
    q.push_back(("second\nline".to_string(), vec![]));
    let lines = queued_preview_lines(&q, 80);
    assert_eq!(lines.len(), 3); // header + 2 entries
    let text: String = lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Queued follow-up inputs (2)"), "got: {text}");
    assert!(text.contains("↳ hello"), "got: {text}");
    assert!(text.contains("↳ second"), "got: {text}");
}
