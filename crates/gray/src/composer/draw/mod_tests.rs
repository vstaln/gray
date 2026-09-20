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
    assert_eq!(desired_viewport_h(0, 0, 0, 3, 0, 0, VIEWPORT_H), 4);
    // Slash popup: input 3 + panel 6 + footer 1 = 10.
    assert_eq!(desired_viewport_h(0, 0, 0, 3, 6, 0, VIEWPORT_H), 10);
    // Running: status 2 + input 3 + footer 1 = 6.
    assert_eq!(desired_viewport_h(2, 0, 0, 3, 0, 0, VIEWPORT_H), 6);
    // Running + full panel: 3 + 3 + 6 + 1 = 13.
    assert_eq!(desired_viewport_h(3, 0, 0, 3, 6, 0, VIEWPORT_H), 13);
    // Question panel: expands up to available screen height to show all options.
    assert_eq!(desired_viewport_h(0, 0, 0, 0, 15, 0, 23), 16);
}

/// A multi-line input must not be clipped by the viewport cap.
///
/// The cap used to be `VIEWPORT_H + widget_h`, so the whole inline viewport --
/// and therefore the input box -- was pinned near 14 rows. A pasted paragraph
/// that wrapped to 12 content rows plus the two margin rows hit exactly 14 and
/// lost its last row; anything longer was cut hard. The cap is the screen now.
#[test]
fn viewport_cap_is_screen_bounded_not_viewport_h() {
    // A 40-row terminal must allow a far taller viewport than the 14-row idle
    // transcript, otherwise a long input gets clipped.
    assert_eq!(viewport_cap(40), 38);
    // Leaves the shell prompt row below, never the whole screen.
    assert!(viewport_cap(40) < 40);
    // Small terminals still clamp to the idle floor.
    assert_eq!(viewport_cap(3), MIN_VIEWPORT_H);
    assert_eq!(viewport_cap(0), MIN_VIEWPORT_H);
}

/// The reported symptom: pasting a paragraph that wraps to 12 rows left only a
/// couple of them visible, because the viewport could not grow to hold the box.
/// Given a screen-bounded cap the box fits.
#[test]
fn multiline_input_is_not_clipped_by_the_viewport_cap() {
    // 12 wrapped content rows + the two margin rows build_input_box adds.
    let box_rows: u16 = 14;
    let rows: u16 = 40;
    let cap = viewport_cap(rows);
    let desired = desired_viewport_h(0, 0, 0, box_rows, 0, 0, cap);
    // The viewport must be tall enough for the whole box, not a couple of rows.
    assert!(
        desired >= box_rows + 1,
        "viewport {desired} cannot hold a {box_rows}-row input box"
    );
    assert!(desired <= cap);
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

#[test]
fn live_tool_viewport_height_counts_live_rows() {
    // Live cards sit between queued preview and input: 1 live row grows
    // the exact-fit viewport by exactly 1 (status 2 + live 1 + input 3).
    let without = desired_viewport_h(2, 0, 0, 3, 0, 0, VIEWPORT_H);
    let with = desired_viewport_h(2, 0, 1, 3, 0, 0, VIEWPORT_H);
    assert_eq!(with, without + 1, "live rows must reserve viewport space");
    // Cap: 3 cards + `… +N more` overflow row, never unbounded.
    let capped = desired_viewport_h(2, 0, 4, 3, 0, 0, VIEWPORT_H);
    assert_eq!(capped, without + 4, "3 live + overflow row: {capped}");
    // Clamp holds at the top: live cards can never push past VIEWPORT_H.
    assert_eq!(
        desired_viewport_h(3, 4, 40, 3, 6, 1, VIEWPORT_H),
        VIEWPORT_H
    );
}

#[test]
fn live_card_has_one_left_padding_cell() {
    use ratatui::widgets::Widget;
    let area = Rect::new(0, 0, 30, 1);
    let mut buffer = ratatui::buffer::Buffer::empty(area);
    live_tool_row(Line::from("⬡ Running cargo test")).render(area, &mut buffer);
    assert_eq!(buffer[(0, 0)].symbol(), " ");
    assert_eq!(buffer[(1, 0)].symbol(), "⬡");
    assert_eq!(buffer[(0, 0)].bg, crate::theme::theme().surface_bg);
}
