use super::*;
use crate::tui::braille_fallback;

#[test]
fn braille_fallback_maps_dot_density_to_ascii_ramp() {
    assert_eq!(braille_fallback('\u{2800}'), ' ');
    assert_eq!(braille_fallback('\u{2801}'), '.');
    assert_eq!(braille_fallback('\u{2803}'), '.');
    assert_eq!(braille_fallback('\u{2807}'), '+');
    assert_eq!(braille_fallback('\u{280f}'), '+');
    assert_eq!(braille_fallback('\u{281f}'), '#');
    assert_eq!(braille_fallback('\u{28ff}'), '#');
    assert_eq!(braille_fallback('x'), 'x');
}

#[test]
fn settle_frames_finish_on_the_logo_mark() {
    let frames = intro_frames();
    assert!(
        frames.len() > INTRO_SETTLE_FRAMES,
        "baked frames plus settle frames"
    );
    let (mark, _) = frames.last().expect("settle frame");
    let want = crate::tui::logo_mark().to_vec();
    let got: Vec<String> = mark
        .iter()
        .map(|l| l.trim_end_matches('\u{2800}').to_string())
        .collect();
    assert_eq!(got, want, "the settle must land exactly on the logo mark");
}

#[test]
fn settle_frames_only_grow_toward_the_mark() {
    let frames = intro_frames();
    let want = braille_dots(crate::tui::logo_mark());
    let mut prev: Option<std::collections::BTreeSet<(usize, usize)>> = None;
    for (mark, _) in frames.iter().skip(frames.len() - INTRO_SETTLE_FRAMES) {
        let dots = braille_dots(mark);
        if let Some(p) = &prev {
            assert!(p.is_subset(&dots), "settle frames must only add dots");
        }
        assert!(dots.is_subset(&want), "settle must not overshoot the logo");
        prev = Some(dots);
    }
}

#[test]
fn the_mark_is_the_baked_logo_asset() {
    let src: Vec<String> = crate::tui::LOGO
        .lines()
        .map(|l| l.trim_end_matches('\u{2800}').to_string())
        .collect();
    assert_eq!(crate::tui::logo_mark(), &src[..]);
    let (cx, cy) = MARK_CELLS;
    assert_eq!(src.len(), cy, "baked at {cx}x{cy} cells");
    assert!(src.iter().all(|l| l.chars().count() <= cx));
}

#[test]
fn intro_timeline_never_holds_a_frame_and_moves_from_the_start() {
    let frames = intro_frames();
    assert!(
        frames.windows(2).all(|w| w[0].0 != w[1].0),
        "repeated frame is dead time"
    );
    let last = frames.len() - 1;
    // Even cadence: no clock tenth may skip more than a few frames.
    let steps: Vec<usize> = (0..=10)
        .map(|i| intro_frame_index(Duration::from_millis(DRAW_MS * i / 10), last))
        .collect();
    assert!(
        steps
            .windows(2)
            .all(|w| w[1] > w[0] && w[1] - w[0] <= last / 5),
        "{steps:?}"
    );
    assert!(intro_frame_index(Duration::from_millis(DRAW_MS / 10), last) > 0);
    assert_eq!(
        intro_frame_index(Duration::from_millis(DRAW_MS), last),
        last
    );
    assert!(
        DRAW_MS + HOME_REVEAL_MS <= 2000,
        "intro must finish within 2s"
    );
}
