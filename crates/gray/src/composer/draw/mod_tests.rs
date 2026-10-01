use super::*;

/// The reported bug: a streamed paragraph break grew and shrank the band by
/// one row. The band is the inline viewport's top edge, so every resize
/// swallowed the transcript row that had just been painted (the second,
/// phantom blank row between paragraphs) and hopped the input box and the
/// footer up and down. The dock's height is a constant while a status shows,
/// so the transcript tail cannot move the band any more.
#[test]
fn the_band_does_not_resize_at_a_paragraph_break() {
    // No status: the band is the input box and the footer.
    assert_eq!(status_dock_h(false), 0);
    // With a status: separator + status + the live-card slot, on every
    // frame — the slot is also the gap between the status line and the box.
    assert_eq!(status_dock_h(true), 3);
    // Band while streaming: dock 3 + box 3 + footer 1. Never 6 or 8.
    assert_eq!(
        desired_viewport_h(status_dock_h(true), 0, 0, 3, 0, 0, viewport_cap(40)),
        7
    );
}

#[test]
fn desired_viewport_exact_fit() {
    // The idle box is measured, not hard-coded: 3 rows (top pad, `❯`, pad).
    let box_rows = build_input_box("", 0, 80, None).lines.len() as u16;
    assert_eq!(box_rows, 3, "top pad + prompt + bottom pad");
    // Idle: box 3 + footer 1 = 4 rows (MIN_VIEWPORT_H).
    assert_eq!(
        desired_viewport_h(0, 0, 0, box_rows, 0, 0, viewport_cap(40)),
        MIN_VIEWPORT_H
    );
    // Slash popup: box 3 + panel 6 + footer 1 = 10.
    assert_eq!(
        desired_viewport_h(0, 0, 0, box_rows, 6, 0, viewport_cap(40)),
        10
    );
    // Running: status 2 + box 3 + footer 1 = 6.
    assert_eq!(
        desired_viewport_h(2, 0, 0, box_rows, 0, 0, viewport_cap(40)),
        6
    );
    // Running + full panel: 3 + 3 + 6 + 1 = 13.
    assert_eq!(
        desired_viewport_h(3, 0, 0, box_rows, 6, 0, viewport_cap(40)),
        13
    );
    // Question panel: expands up to available screen height to show all options.
    assert_eq!(desired_viewport_h(0, 0, 0, 0, 15, 0, 23), 16);
}

/// A multi-line input must not be clipped by the viewport cap.
///
/// The cap used to be `viewport_cap(40) + widget_h`, so the whole inline viewport --
/// and therefore the input box -- was pinned near 14 rows. A pasted paragraph
/// that wrapped to 12 content rows plus the margin row hit exactly 13 and
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
    // 12 wrapped content rows + the bottom margin build_input_box adds.
    let box_rows: u16 = 13;
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

/// The live card lives in the dock's own slot row, so starting or ending a
/// tool call cannot resize the band — that resize hopped the input box and
/// left the row it vacated blank in the transcript (the reported flicker
/// and the stray gap around every tool card).
#[test]
fn the_live_card_cannot_resize_the_band() {
    let with = desired_viewport_h(status_dock_h(true), 0, 0, 3, 0, 0, viewport_cap(40));
    // No live parameter exists any more: any number of live cards paints
    // into the dock's single slot row, so the height is one value.
    assert_eq!(with, 7, "dock 3 + box 3 + footer 1");
    // Clamp holds at the top: the screen cap.
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

/// The text area and the context footer always keep their rows: whatever
/// the live cards, widget, ask modal and queued preview want, they are
/// capped by what the screen has left after the rows that must survive.
#[test]
fn the_text_area_and_footer_are_never_trimmed_away() {
    // 30-row screen, 3-row dock, 2-row box, 1-row footer: 24 rows of band.
    assert_eq!(band_budget(30, 3 + 2 + 0 + 1), 24);
    // A 4-row box (a two-line draft) eats the band above it, never the reverse.
    assert_eq!(band_budget(30, 3 + 4 + 1), 22);
    // Short screen: the budget floors at zero instead of going negative.
    assert_eq!(band_budget(6, 3 + 3 + 1), 0);
}

/// A busy turn is trimmed most-transient-first and the total never exceeds
/// the allowance, so the box and the footer keep their rows.
#[test]
fn a_busy_band_is_trimmed_most_transient_first() {
    // Counts in trim order: ask modal, queued follow-up, plugin widget,
    // live tool cards. 6 rows of allowance on a busy turn: the blocking
    // ask modal and the user's own text keep their rows, the widget
    // decoration is cut, the elastic live cards take what is left.
    let mut counts = [2, 2, 4, 10];
    trim_to_allowance(&mut counts, 6);
    assert_eq!(counts, [2, 2, 2, 0]);
    // An allowance with no room trims everything and never goes negative.
    let mut counts = [2, 2, 4, 10];
    trim_to_allowance(&mut counts, 0);
    assert_eq!(counts, [0, 0, 0, 0]);
    // Room to spare leaves the counts alone.
    let mut counts = [10, 2];
    trim_to_allowance(&mut counts, 40);
    assert_eq!(counts, [10, 2]);
}

/// The reported bug: the box kept painting "Please continue…" while the turn
/// was still working. The flag is armed when the REPL loop blocks on input —
/// before that turn is submitted — so mid-turn it is stale, and a live status
/// pill next to the hint reads as "working AND waiting for Enter". The hint
/// belongs to the idle composer after an interrupt or an error, nothing else.
#[test]
fn continue_ghost_only_on_an_idle_composer_with_a_pending_resume() {
    // Idle after an interrupt, nothing typed: the hint is the point.
    assert_eq!(
        continue_ghost(true, false, false, ""),
        Some(crate::repl::CONTINUE_GHOST)
    );
    // Working: never, whatever the stale flag says.
    assert_eq!(
        continue_ghost(true, true, false, ""),
        None,
        "no ghost mid-turn"
    );
    // A live status pill (the turn's own line, `Reconnecting`, the ask
    // modal's `Question`): the hint would contradict it.
    assert_eq!(
        continue_ghost(true, false, true, ""),
        None,
        "no ghost under a pill"
    );
    // Typing, or no pending resume: no hint either.
    assert_eq!(continue_ghost(true, false, false, "hi"), None);
    assert_eq!(continue_ghost(false, false, false, ""), None);
    // The flag is dropped at turn start, so the input gate is honest too.
    // (`begin_turn` needs a TTY; the predicate above is the testable seam.)
}
