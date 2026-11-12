use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

pub fn chars_width(chars: &[char]) -> usize {
    chars.iter().map(|c| char_width(*c)).sum()
}

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

pub fn fit_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    if max > 0 {
        out.push('…');
    }
    out
}

pub fn word_window_end(chars: &[char], start: usize, max_w: usize) -> usize {
    let mut end = (start + fit_char_count(&chars[start..], max_w)).min(chars.len());
    if end < chars.len()
        && let Some(sp) = chars[start..end].iter().rposition(|c| *c == ' ')
        && sp > 0
    {
        end = start + sp + 1;
    }
    end
}
