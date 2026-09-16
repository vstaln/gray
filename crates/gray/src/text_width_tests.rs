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

#[test]
fn fit_never_splits_and_always_progresses() {
    let chars: Vec<char> = "a⭐b".chars().collect();
    assert_eq!(fit_char_count(&chars, 3), 2);
    assert_eq!(fit_char_count(&chars, 0), 1); // progress over exactness
    assert_eq!(fit_char_count(&[], 0), 0);
}
