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
    assert_eq!(desired_viewport_h(0, 0, 0, 3, 0, 0, viewport_cap(40)), 4);
    // Slash popup: input 3 + panel 6 + footer 1 = 10.
    assert_eq!(desired_viewport_h(0, 0, 0, 3, 6, 0, viewport_cap(40)), 10);
    // Running: status 2 + input 3 + footer 1 = 6.
    assert_eq!(desired_viewport_h(2, 0, 0, 3, 0, 0, viewport_cap(40)), 6);
    // Running + full panel: 3 + 3 + 6 + 1 = 13.
    assert_eq!(desired_viewport_h(3, 0, 0, 3, 6, 0, viewport_cap(40)), 13);
    // Question panel: expands up to available screen height to show all options.
    assert_eq!(desired_viewport_h(0, 0, 0, 0, 15, 0, 23), 16);
}

/// A multi-line input must not be clipped by the viewport cap.
///
/// The cap used to be `viewport_cap(40) + widget_h`, so the whole inline viewport --
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
            viewport_cap(40),
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
    let without = desired_viewport_h(2, 0, 0, 3, 0, 0, viewport_cap(40));
    let with = desired_viewport_h(2, 0, 1, 3, 0, 0, viewport_cap(40));
    assert_eq!(with, without + 1, "live rows must reserve viewport space");
    // Cap: 3 cards + `… +N more` overflow row, never unbounded.
    let capped = desired_viewport_h(2, 0, 4, 3, 0, 0, viewport_cap(40));
    assert_eq!(capped, without + 4, "3 live + overflow row: {capped}");
    // Clamp holds at the top: the screen cap. `live_h = 40` is hypothetical --
    // MAX_LIVE_TOOLS bounds it to 3 cards + 1 overflow row -- and exists only
    // to prove the clamp still bites.
    let cap = viewport_cap(40);
    assert_eq!(desired_viewport_h(3, 4, 40, 3, 6, 1, cap), cap);
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

#[test]
fn footer_badge_snapshot_freezes_the_right_segment_mid_turn() {
    use crate::setup::context::{cache_model_reasoning, model_supports_reasoning};

    let live_model = "footer-badge-tests/live-flag-model";
    // The flag under the badge is a process-global cache that background
    // discovery (provider /models, then models.dev) keeps writing while a
    // turn streams: unresolved → some(true), provider arrives → some(false),
    // models.dev lands → some(true). Each flip used to resize the
    // right-anchored footer text and walk the model name across the bar.
    crate::setup::cache_model_reasoning(live_model, true);
    assert_eq!(model_supports_reasoning(live_model), Some(true));

    // No snapshot (idle frames): resolution stays live, so the provider's
    // converged answer still lands between turns.
    assert!(footer_badge_visible(live_model, None));
    crate::setup::cache_model_reasoning(live_model, false);
    assert!(!footer_badge_visible(live_model, None));

    // Turn in flight: the snapshot wins, whatever the async caches wrote.
    let streaming = "footer-badge-tests/streaming-flag-model";
    assert!(footer_badge_visible(streaming, Some(true)));
    assert!(!footer_badge_visible(streaming, Some(false)));
    crate::setup::cache_model_reasoning(streaming, true);
    assert!(!footer_badge_visible(streaming, Some(false)));
    assert!(footer_badge_visible(live_model, Some(true)));
}

/// The footer gauge is the viewport's last row: a one-row shortfall in the
/// pre-computed viewport estimate (mid-stream text wraps taller than the
/// estimate) used to push `footer_y` past the frame bottom and skip the
/// paint — the gauge vanished for that frame while tokens streamed. The
/// clamp paints it on the frame's last row instead.
#[test]
fn footer_paint_row_clamps_into_the_frame_instead_of_vanishing() {
    let area = Rect::new(0, 10, 40, 6);
    // In budget: unchanged.
    assert_eq!(footer_paint_row(15, area), Some(15));
    // One row over: painted on the last frame row, never skipped.
    assert_eq!(footer_paint_row(16, area), Some(15));
    assert_eq!(footer_paint_row(20, area), Some(15));
    // Zero-height frame paints nothing.
    assert_eq!(footer_paint_row(5, Rect::new(0, 0, 40, 0)), None);
}

/// The latched viewport: whenever the estimate short-cuts a dock segment
/// (`status_h` re-read inside the frame after the ratchet latch re-arms),
/// the floor must already cover the tallest possible frame so the measured
/// rows fit without a second grow (the footer/padding flicker).
#[test]
fn latched_viewport_floor_covers_the_measured_frame_when_the_estimate_shortcuts_a_segment() {
    // Estimate short-cut the dock entirely (status_h computed as 0 at
    // sizing time): the frame can still measure up to 3 dock rows
    // (seam + status + breath) plus the footer.
    for status_h in 0..=3u16 {
        let floor = latched_viewport_floor(status_h, 0, 0, 3, 0, viewport_cap(40));
        assert!(
            floor >= status_h + 3 + 1,
            "status_h={status_h}: floor {floor} cannot hold dock {status_h} + box 3 + footer"
        );
    }
    // A short-cut live-row reserve (estimate 0, frame measures 2) keeps
    // the input box and footer inside the viewport too.
    let floor = latched_viewport_floor(0, 0, 2, 3, 0, viewport_cap(40));
    assert!(floor >= 0 + 2 + 3 + 1);
    // The floor never exceeds the screen cap.
    assert_eq!(latched_viewport_floor(3, 0, 0, 30, 0, 10), 10);
}

#[test]
fn chrome_row_paints_the_whole_composer_band() {
    use ratatui::widgets::Widget;
    // Plugin-widget / queued / ask-modal rows live between the live cards
    // and the input box; painted transparent they showed as a stripe of the
    // terminal's default background through the middle of the composer.
    let area = Rect::new(0, 0, 40, 1);
    let mut buffer = ratatui::buffer::Buffer::empty(area);
    chrome_row(Line::from("Build phase one")).render(area, &mut buffer);
    assert_eq!(buffer[(0, 0)].symbol(), "B");
    assert_eq!(buffer[(0, 0)].bg, crate::theme::theme().surface_bg);
    assert_eq!(
        buffer[(39, 0)].bg,
        crate::theme::theme().surface_bg,
        "the band covers the full row, not just the text cells"
    );
}
