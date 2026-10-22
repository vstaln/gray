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
fn the_mark_is_drawn_at_twice_its_source_size() {
    let src: Vec<String> = crate::tui::LOGO.lines().map(str::to_string).collect();
    let big = crate::tui::logo_mark();
    assert_eq!(big.len(), src.len() * 2);
    let (small, big) = (braille_dots(&src), braille_dots(big));
    assert_eq!(big.len(), small.len() * 4, "each dot becomes a 2x2 block");
    assert!(
        small
            .iter()
            .all(|&(x, y)| big.contains(&(2 * x + 1, 2 * y + 1)))
    );
}
