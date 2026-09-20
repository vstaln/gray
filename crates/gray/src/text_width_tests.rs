use super::*;

#[test]
fn ascii_width_equals_char_count() {
    assert_eq!(display_width("abc *"), 5);
    assert_eq!(display_width("hello"), 5);
    assert_eq!(display_width(""), 0);
}

#[test]
fn emoji_counts_two_cells() {
    assert_eq!(display_width("⭐"), 2);
    assert_eq!(display_width("a⭐b"), 4);
    assert_eq!(char_width('⭐'), 2);
    assert_eq!(char_width('a'), 1);
}

/// UI glyphs must stay single-cell: the footer's pad math
/// (`left_len`/`right_len`) and every wrap assume one cell per glyph.
/// ◷ is the prompt-cache warmth countdown, ⬡ the status dock,
/// ⚠ the warning rows.
#[test]
fn ui_glyphs_are_single_cell() {
    for c in ['\u{25f7}', '\u{2b21}', '\u{26a0}'] {
        assert_eq!(char_width(c), 1, "U+{:04X} must be one cell", c as u32);
    }
}

#[test]
fn fit_never_splits_and_always_progresses() {
    let chars: Vec<char> = "a⭐b".chars().collect();
    assert_eq!(fit_char_count(&chars, 3), 2);
    assert_eq!(fit_char_count(&chars, 0), 1); // progress over exactness
    assert_eq!(fit_char_count(&[], 0), 0);
}
