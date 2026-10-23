use super::widgets::transcript_ends_blank;
use super::*;

#[test]
fn transcript_ends_blank_matches_ensure_gap() {
    use ratatui::style::Style;
    assert!(!transcript_ends_blank(&[]));
    assert!(transcript_ends_blank(&[Line::from("")]));
    assert!(transcript_ends_blank(&[Line::from(" ")]));
    assert!(!transcript_ends_blank(&[Line::from("text")]));

    let bg = Style::default().bg(crate::theme::GRAY_UI_THEME.surface_bg);
    assert!(!transcript_ends_blank(&[Line::from("").style(bg)]));
}

#[test]
fn desired_viewport_exact_fit() {
    let box_rows = build_input_box("", 0, 80, None, None, &[]).lines.len() as u16;
    assert_eq!(box_rows, 3, "top pad + prompt + bottom pad");

    assert_eq!(
        desired_viewport_h(0, 0, 0, box_rows, 0, 0, 1, viewport_cap(40)),
        MIN_VIEWPORT_H
    );

    assert_eq!(
        desired_viewport_h(0, 0, 0, box_rows, 6, 0, 1, viewport_cap(40)),
        10
    );

    assert_eq!(
        desired_viewport_h(2, 0, 0, box_rows, 0, 0, 1, viewport_cap(40)),
        6
    );

    assert_eq!(
        desired_viewport_h(3, 0, 0, box_rows, 6, 0, 1, viewport_cap(40)),
        13
    );

    assert_eq!(desired_viewport_h(0, 0, 0, 0, 15, 0, 1, 23), 16);
}

#[test]
fn viewport_cap_is_screen_bounded_not_viewport_h() {
    assert_eq!(viewport_cap(40), 38);

    assert!(viewport_cap(40) < 40);

    assert_eq!(viewport_cap(3), MIN_VIEWPORT_H);
    assert_eq!(viewport_cap(0), MIN_VIEWPORT_H);
}

#[test]
fn multiline_input_is_not_clipped_by_the_viewport_cap() {
    let box_rows: u16 = 13;
    let rows: u16 = 40;
    let cap = viewport_cap(rows);
    let desired = desired_viewport_h(0, 0, 0, box_rows, 0, 0, 1, cap);

    assert!(
        desired > box_rows,
        "viewport {desired} cannot hold a {box_rows}-row input box"
    );
    assert!(desired <= cap);
}

#[test]
fn the_seam_is_the_one_gap_under_the_transcript() {
    assert!(needs_seam(&[Line::from("text")]));
    assert!(!needs_seam(&[Line::from("text"), Line::from("")]));
    assert!(!needs_seam(&[]));
    for tail in [Line::from("text"), Line::from("")] {
        let transcript = [tail];
        let blank_rows =
            usize::from(transcript_ends_blank(&transcript)) + usize::from(needs_seam(&transcript));
        assert_eq!(blank_rows, 1, "exactly one row between transcript and band");
    }
}

#[test]
fn the_tail_margin_is_the_one_gap_above_the_status_line() {
    assert_eq!(tail_gap_h(true, &[Line::from("streamed text")]), 1);
    assert_eq!(tail_gap_h(false, &[Line::from("streamed text")]), 0);
    assert_eq!(tail_gap_h(true, &[]), 0);
    assert_eq!(tail_gap_h(true, &[Line::from("a"), Line::from("")]), 0);
    assert_eq!(tail_block_h(3, 1), 3);
    assert_eq!(tail_block_h(1, 1), 0);
    assert_eq!(tail_block_h(0, 1), 0);
    assert_eq!(tail_block_h(4, 0), 4);
}

#[test]
fn queued_preview_renders_header_and_entries() {
    let mut q: std::collections::VecDeque<(String, Vec<std::path::PathBuf>)> =
        std::collections::VecDeque::new();
    assert!(queued_preview_lines(&q, &[], 80).is_empty());
    q.push_back(("hello".to_string(), vec![]));
    q.push_back(("second\nline".to_string(), vec![]));
    let lines = queued_preview_lines(&q, &[], 80);
    assert_eq!(lines.len(), 4);
    assert!(lines.last().is_some_and(|line| line.spans.is_empty()));
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
    let without = desired_viewport_h(2, 0, 0, 3, 0, 0, 1, viewport_cap(40));
    let with = desired_viewport_h(2, 0, 1, 3, 0, 0, 1, viewport_cap(40));
    assert_eq!(with, without + 1, "live rows must reserve viewport space");

    let capped = desired_viewport_h(2, 0, 4, 3, 0, 0, 1, viewport_cap(40));
    assert_eq!(capped, without + 4, "3 live + overflow row: {capped}");

    let cap = viewport_cap(40);
    assert_eq!(desired_viewport_h(3, 4, 40, 3, 6, 1, 1, cap), cap);
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
    use crate::setup::context::model_supports_reasoning;

    let live_model = "footer-badge-tests/live-flag-model";

    crate::setup::cache_model_reasoning(live_model, true);
    assert_eq!(model_supports_reasoning(live_model), Some(true));

    assert!(footer_badge_visible(live_model, None));
    crate::setup::cache_model_reasoning(live_model, false);
    assert!(!footer_badge_visible(live_model, None));

    let streaming = "footer-badge-tests/streaming-flag-model";
    assert!(footer_badge_visible(streaming, Some(true)));
    assert!(!footer_badge_visible(streaming, Some(false)));
    crate::setup::cache_model_reasoning(streaming, true);
    assert!(!footer_badge_visible(streaming, Some(false)));
    assert!(footer_badge_visible(live_model, Some(true)));
}

#[test]
fn footer_paint_row_clamps_into_the_frame_instead_of_vanishing() {
    let area = Rect::new(0, 10, 40, 6);

    assert_eq!(footer_paint_row(15, area), Some(15));

    assert_eq!(footer_paint_row(16, area), Some(15));
    assert_eq!(footer_paint_row(20, area), Some(15));

    assert_eq!(footer_paint_row(5, Rect::new(0, 0, 40, 0)), None);
}

#[test]
fn latched_viewport_floor_covers_the_measured_frame_when_the_estimate_shortcuts_a_segment() {
    for status_h in 0..=3u16 {
        let floor = latched_viewport_floor(status_h, 0, 0, 3, 0, 1, viewport_cap(40));
        assert!(
            floor > status_h + 3,
            "status_h={status_h}: floor {floor} cannot hold dock {status_h} + box 3 + footer"
        );
    }

    let floor = latched_viewport_floor(0, 0, 2, 3, 0, 1, viewport_cap(40));

    assert!(floor > 2 + 3);

    assert_eq!(latched_viewport_floor(3, 0, 0, 30, 0, 1, 10), 10);
}

#[test]
fn chrome_row_paints_the_whole_composer_band() {
    use ratatui::widgets::Widget;

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

#[test]
fn the_text_area_and_footer_are_never_trimmed_away() {
    assert_eq!(band_budget(30, 3 + 2 + 1), 24);

    assert_eq!(band_budget(30, 3 + 4 + 1), 22);

    assert_eq!(band_budget(6, 3 + 3 + 1), 0);
}

#[test]
fn a_busy_band_is_trimmed_most_transient_first() {
    let mut counts = [2, 2, 4, 10];
    trim_to_allowance(&mut counts, 6);
    assert_eq!(counts, [2, 2, 2, 0]);

    let mut counts = [2, 2, 4, 10];
    trim_to_allowance(&mut counts, 0);
    assert_eq!(counts, [0, 0, 0, 0]);

    let mut counts = [10, 2];
    trim_to_allowance(&mut counts, 40);
    assert_eq!(counts, [10, 2]);
}

#[test]
fn continue_ghost_hides_while_a_turn_runs() {
    assert_eq!(
        continue_ghost(true, false, ""),
        Some(crate::repl::CONTINUE_GHOST)
    );

    assert_eq!(continue_ghost(true, true, ""), None, "no ghost mid-turn");

    assert_eq!(continue_ghost(true, false, "hi"), None);
    assert_eq!(continue_ghost(false, false, ""), None);
}

#[test]
fn widget_band_takes_two_fifths_of_the_screen() {
    assert_eq!(widget_row_cap(40), 16);
    assert_eq!(widget_row_cap(24), 9);
    assert_eq!(widget_row_cap(10), 4);
    assert_eq!(widget_row_cap(0), 4);
}

#[test]
fn a_video_chip_says_video_not_file() {
    let cells = attachment_cells(&[std::path::PathBuf::from("/tmp/clip.mp4")], "");
    let text: String = cells.iter().map(|(c, _)| *c).collect();
    assert!(text.starts_with(" Video "), "got {text:?}");
}

#[test]
fn input_box_growth_covers_transcript_instead_of_scrolling_it_away() {
    // One-line composer: nothing may cover.
    assert_eq!(band_overlay(3, 1), 0);
    // A 12-line recalled entry: its 11 extra rows cover, so shrinking repaints.
    assert_eq!(band_overlay(14, 12), 11);
    // The total never exceeds the band.
    assert_eq!(band_overlay(5, 40), 5);
}

#[test]
fn the_popup_shows_only_rows_that_fit_beside_the_box() {
    // Room to spare: capped at PANEL_ROWS.
    assert_eq!(popup_rows(40, 30), PANEL_ROWS);
    // A short band keeps one spacer row and shrinks the list.
    assert_eq!(popup_rows(40, 4), 3);
    // No room for a single row: hidden, never drawn over the box.
    assert_eq!(popup_rows(40, 1), 0);
    assert_eq!(popup_rows(2, 30), 2);
}
