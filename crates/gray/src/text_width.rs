//! Display-width helpers: terminal cells, not chars.
//!
//! One place for the wide-char rule (emoji/CJK occupy 2 cells, combining
//! marks 0) so every renderer measures the same way. For pure-ASCII input
//! these equal the char count — swapping them in is a no-op except where
//! it fixes (the diff/panel/footer drift after emoji).
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Terminal cells occupied by `s`.
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Terminal cells occupied by one char (0 for control/combining marks).
pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Display width of a char slice without allocating.
pub fn chars_width(chars: &[char]) -> usize {
    chars.iter().map(|c| char_width(*c)).sum()
}

/// Length (in chars) of the longest prefix of `chars` fitting `max_w`
/// cells. Never splits a char; yields at least 1 for non-empty input so
/// wrapping loops always make progress.
pub fn fit_char_count(chars: &[char], max_w: usize) -> usize {
    let mut w = 0usize;
    for (i, c) in chars.iter().enumerate() {
        w += char_width(*c);
        if w > max_w {
            return i.max(1);
        }
    }
    chars.len()
}

#[cfg(test)]
mod tests {
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
}
