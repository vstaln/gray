//! Shared Signal formatting helpers.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/platforms/signal_format.py` (140 LOC).
//! Keep markdown → Signal native formatting conversion in one place so both the
//! live Signal adapter and standalone send paths emit the same bodyRanges.
//!
//! Python source docstring (preserved):
//! ```text
//! Shared Signal formatting helpers.
//!
//! Keep markdown → Signal native formatting conversion in one place so both the
//! live Signal adapter and standalone send paths emit the same bodyRanges.
//! ```

// ---------------------------------------------------------------------------
// Internal helpers — mirrors Python underscore-prefixed helpers
// ---------------------------------------------------------------------------

fn utf16_len_chars(chars: &[char]) -> usize {
    chars.iter().map(|c| c.len_utf16()).sum()
}

fn utf16_len_str(s: &str) -> usize {
    s.encode_utf16().count()
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_lang_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '+' || c == '-'
}

fn collapse_newlines(text: &str) -> String {
    // Mirrors re.sub(r"\n{3,}", "\n\n", text)
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\n' {
            let mut count = 0;
            let mut j = i;
            while j < chars.len() && chars[j] == '\n' {
                count += 1;
                j += 1;
            }
            if count >= 3 {
                out.push('\n');
                out.push('\n');
            } else {
                for _ in 0..count {
                    out.push('\n');
                }
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out.into_iter().collect()
}

fn normalize_bullet_line(part: &str) -> String {
    // Mirrors re.sub(r"(?m)^([ \t]{0,3})[-*+]\s+", r"\1• ", part) per line
    let mut out = String::new();
    let lines: Vec<&str> = part.split('\n').collect();
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let chars: Vec<char> = line.chars().collect();
        // Count leading spaces/tabs up to 3
        let mut indent_len = 0;
        while indent_len < 3 && indent_len < chars.len() && (chars[indent_len] == ' ' || chars[indent_len] == '\t') {
            indent_len += 1;
        }
        // Check bullet
        if indent_len < chars.len()
            && (chars[indent_len] == '-' || chars[indent_len] == '*' || chars[indent_len] == '+')
            && indent_len + 1 < chars.len()
            && chars[indent_len + 1].is_whitespace()
        {
            // Need at least one whitespace after bullet; we have it at indent_len+1
            // Find end of whitespace sequence after bullet
            let mut ws_end = indent_len + 1;
            while ws_end < chars.len() && chars[ws_end].is_whitespace() {
                ws_end += 1;
            }
            // Replacement: indent + "• " + rest (remaining after ws)
            let indent: String = chars[0..indent_len].iter().collect();
            let rest: String = chars[ws_end..].iter().collect();
            out.push_str(&indent);
            out.push('•');
            out.push(' ');
            out.push_str(&rest);
        } else {
            // Check also case where bullet preceded by fewer indent than maximum but we counted greedy max.
            // Example line "  - item" indent_len=2 matches, above handles.
            // For line "    - item" indent_len=3 but chars[3]==' ' not bullet -> no match, correct.
            // However line " - item" indent_len=1? Actually chars[0]==' ', indent_len=1, chars[1]=='-' matches.
            // So correct.
            out.push_str(line);
        }
    }
    out
}

fn normalize_bullet_markers(source: &str) -> String {
    // Mirrors _normalize_bullet_markers:
    //   parts = re.split(r"(```.*?```)", source, flags=re.DOTALL)
    //   for idx, part in enumerate(parts):
    //       if idx % 2 == 1: continue
    //       parts[idx] = re.sub(r"(?m)^([ \t]{0,3})[-*+]\s+", r"\1• ", part)
    let chars: Vec<char> = source.chars().collect();
    let mut result = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        // Check for opening ```
        if i + 2 < chars.len() && chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
            // Find closing ```
            let mut j = i + 3;
            let mut found: Option<usize> = None;
            while j + 2 < chars.len() {
                if chars[j] == '`' && chars[j + 1] == '`' && chars[j + 2] == '`' {
                    found = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(end) = found {
                let segment: String = chars[i..end + 3].iter().collect();
                result.push_str(&segment);
                i = end + 3;
                continue;
            } else {
                // No closing, remainder is non-code
                let remaining: String = chars[i..].iter().collect();
                result.push_str(&normalize_bullet_line(&remaining));
                break;
            }
        } else {
            // Find next ```
            let mut next: Option<usize> = None;
            let mut k = i;
            while k + 2 < chars.len() {
                if chars[k] == '`' && chars[k + 1] == '`' && chars[k + 2] == '`' {
                    next = Some(k);
                    break;
                }
                k += 1;
            }
            let end = next.unwrap_or(chars.len());
            let segment: String = chars[i..end].iter().collect();
            result.push_str(&normalize_bullet_line(&segment));
            i = end;
            if next.is_none() {
                break;
            }
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Code block handling — mirrors Python code_block loop
// ---------------------------------------------------------------------------

fn find_code_block(chars: &[char]) -> Option<(usize, usize, usize, usize)> {
    // Returns (open_pos, inner_start, inner_end, close_end)
    let n = chars.len();
    let mut i = 0;
    while i + 2 < n {
        if chars[i] == '`' && chars[i + 1] == '`' && chars[i + 2] == '`' {
            let mut cursor = i + 3;
            while cursor < n && is_lang_char(chars[cursor]) {
                cursor += 1;
            }
            if cursor < n && chars[cursor] == '\n' {
                cursor += 1;
            }
            let inner_start = cursor;
            // Find closing ```
            let mut j = inner_start;
            let mut found: Option<usize> = None;
            while j + 2 < n {
                if chars[j] == '`' && chars[j + 1] == '`' && chars[j + 2] == '`' {
                    found = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(close_pos) = found {
                return Some((i, inner_start, close_pos, close_pos + 3));
            }
            // No closing for this opening, try next opening
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Heading handling — mirrors Python heading loop
// ---------------------------------------------------------------------------

fn find_heading_matches(chars: &[char]) -> Vec<(usize, usize, usize)> {
    // Each entry: (match_start, match_end, eol)
    let mut matches = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let at_line_start = i == 0 || chars[i - 1] == '\n';
        if at_line_start {
            let mut hash_count = 0;
            let mut j = i;
            while j < n && chars[j] == '#' && hash_count < 6 {
                hash_count += 1;
                j += 1;
            }
            if hash_count >= 1 && hash_count <= 6 && j < n && chars[j].is_whitespace() {
                // Consume all consecutive whitespace after hashes ( \s+ )
                let mut ws_end = j;
                while ws_end < n && chars[ws_end].is_whitespace() {
                    ws_end += 1;
                }
                // Need at least one whitespace, which we have because j < ws_end
                let match_start = i;
                let match_end = ws_end;
                // Find eol: next '\n' from match_end, or len
                let mut eol = match_end;
                while eol < n && chars[eol] != '\n' {
                    eol += 1;
                }
                matches.push((match_start, match_end, eol));
                // Advance i to match_end to continue scanning; heading_text is not part of match
                // But to find next heading, we need to scan after match_end.
                // We'll set i = match_end and continue (loop will increment).
                // To avoid infinite, jump to eol (next line)
                i = eol;
                // The loop will increment i at bottom, but we want to continue from eol
                // So set i = eol and continue without extra increment
                // We handle by continuing with i = eol (which points to '\n' or len)
                // Next iteration will check i, but i points to newline, not line start, so will advance.
                // To move to next char after newline, we will increment.
                // Simplify: set i = match_end, but we already have eol.
                // Let's set i = eol and then i+=1 handled by loop?
                // We'll manually handle: if we are here, set i = eol; then i+=1 in next iteration equivalent.
                // Instead, just set i = eol and continue loop with i+=1 logic.
                // We'll do i = eol; // will be incremented at end, but need to avoid double.
                // So we break to handle correctly: set i = eol + 1? Let's just set i = eol and continue with i = eol (and loop's i+=1 will happen). Simpler to increment after.
            }
        }
        i += 1;
    }
    matches
}

// ---------------------------------------------------------------------------
// Inline pattern scanners — mirrors Python patterns list
// ---------------------------------------------------------------------------

fn find_double_star(chars: &[char]) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i + 1 < n {
        if chars[i] == '*' && chars[i + 1] == '*' {
            let inner_start = i + 2;
            // Need closing ** with at least one char inner
            let mut j = inner_start + 1; // minimal inner len 1 => j >= inner_start+1
            let mut found: Option<usize> = None;
            while j + 1 < n {
                if chars[j] == '*' && chars[j + 1] == '*' {
                    // Check inner length >=1
                    if j >= inner_start + 1 {
                        found = Some(j);
                        break;
                    }
                }
                j += 1;
            }
            if let Some(close) = found {
                out.push((i, close + 2, inner_start, close));
                i = close + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn find_double_underscore(chars: &[char]) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i + 1 < n {
        if chars[i] == '_' && chars[i + 1] == '_' {
            let inner_start = i + 2;
            let mut j = inner_start + 1;
            let mut found: Option<usize> = None;
            while j + 1 < n {
                if chars[j] == '_' && chars[j + 1] == '_' {
                    if j >= inner_start + 1 {
                        found = Some(j);
                        break;
                    }
                }
                j += 1;
            }
            if let Some(close) = found {
                out.push((i, close + 2, inner_start, close));
                i = close + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn find_double_tilde(chars: &[char]) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i + 1 < n {
        if chars[i] == '~' && chars[i + 1] == '~' {
            let inner_start = i + 2;
            let mut j = inner_start + 1;
            let mut found: Option<usize> = None;
            while j + 1 < n {
                if chars[j] == '~' && chars[j + 1] == '~' {
                    if j >= inner_start + 1 {
                        found = Some(j);
                        break;
                    }
                }
                j += 1;
            }
            if let Some(close) = found {
                out.push((i, close + 2, inner_start, close));
                i = close + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn find_backtick(chars: &[char]) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i] == '`' {
            // Find closing ` on same line (no newline in inner)
            let inner_start = i + 1;
            let mut j = inner_start;
            let mut found: Option<usize> = None;
            while j < n {
                if chars[j] == '`' {
                    if j >= inner_start + 1 {
                        // Check no newline in inner
                        let has_newline = chars[inner_start..j].contains(&'\n');
                        if !has_newline {
                            found = Some(j);
                            break;
                        }
                    }
                    // If has newline, this candidate invalid, continue searching for next `
                    // But any further j will also contain newline, so we could break.
                    // However we continue to allow cases where opening and closing are on same line but inner contains newline? That's invalid per DOTALL false, so no match for this i.
                    // So we break after finding newline? Simplify: if has_newline, then no valid closing for this opening on same line, break search.
                    if chars[inner_start..j].contains(&'\n') {
                        break;
                    }
                }
                // If we encounter newline before closing, then no match for this opening
                if chars[j] == '\n' {
                    break;
                }
                j += 1;
            }
            if let Some(close) = found {
                out.push((i, close + 1, inner_start, close));
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn find_single_star_italic(chars: &[char]) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i] == '*' {
            // Opening conditions: (?<!\*) and (?!\*| )
            let not_preceded_by_star = i == 0 || chars[i - 1] != '*';
            let not_followed_by_star_or_space = i + 1 < n && chars[i + 1] != '*' && chars[i + 1] != ' ';
            if not_preceded_by_star && not_followed_by_star_or_space {
                let inner_start = i + 1;
                // Search for closing *
                let mut j = inner_start + 1; // at least one char inner, and need closing after inner
                let mut found: Option<usize> = None;
                while j < n {
                    if chars[j] == '*' {
                        // Closing conditions: (?<!\*) and (?!\*)
                        let closing_not_preceded = j == 0 || chars[j - 1] != '*';
                        let closing_not_followed = j + 1 >= n || chars[j + 1] != '*';
                        if closing_not_preceded && closing_not_followed {
                            // Check inner constraints: inner = chars[inner_start..j]
                            if j > inner_start {
                                let inner = &chars[inner_start..j];
                                if !inner.contains(&'\n') {
                                    // valid
                                    found = Some(j);
                                    break;
                                } else {
                                    // Inner contains newline -> DOTALL false, invalid; further j will also contain newline, so break
                                    break;
                                }
                            }
                        }
                    }
                    // If we encounter newline, DOTALL false means inner cannot cross newline, so break
                    if chars[j] == '\n' {
                        break;
                    }
                    j += 1;
                }
                if let Some(close) = found {
                    out.push((i, close + 1, inner_start, close));
                    i = close + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn find_single_underscore_italic(chars: &[char]) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        if chars[i] == '_' {
            let not_preceded_by_word = i == 0 || !is_word_char(chars[i - 1]);
            let not_followed_by_underscore = i + 1 >= n || chars[i + 1] != '_';
            if not_preceded_by_word && not_followed_by_underscore {
                let inner_start = i + 1;
                let mut j = inner_start + 1;
                let mut found: Option<usize> = None;
                while j < n {
                    if chars[j] == '_' {
                        let not_preceded = j == 0 || chars[j - 1] != '_';
                        let not_followed_by_word = j + 1 >= n || !is_word_char(chars[j + 1]);
                        if not_preceded && not_followed_by_word {
                            if j > inner_start {
                                let inner = &chars[inner_start..j];
                                if !inner.contains(&'\n') {
                                    found = Some(j);
                                    break;
                                } else {
                                    break;
                                }
                            }
                        }
                    }
                    if chars[j] == '\n' {
                        break;
                    }
                    j += 1;
                }
                if let Some(close) = found {
                    out.push((i, close + 1, inner_start, close));
                    i = close + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python top-level function
// ---------------------------------------------------------------------------

/// Convert markdown to plain text + Signal textStyles list.
///
/// Signal doesn't render markdown. Instead it uses ``bodyRanges`` (exposed by
/// signal-cli as ``textStyle`` / ``textStyles`` params) with the format
/// ``start:length:STYLE``.
///
/// Positions are measured in UTF-16 code units because that's what the Signal
/// protocol uses.
///
/// Supported styles: BOLD, ITALIC, STRIKETHROUGH, MONOSPACE.
pub fn markdown_to_signal(text: &str) -> (String, Vec<String>) {
    // Collapse 3+ newlines and trim — mirrors:
    //   text = re.sub(r"\n{3,}", "\n\n", text)
    //   text = text.strip()
    let mut current = collapse_newlines(text);
    current = current.trim().to_string();
    current = normalize_bullet_markers(&current);

    let mut styles: Vec<(usize, usize, String)> = Vec::new();

    // Code blocks — mirrors Python while loop
    let mut chars: Vec<char> = current.chars().collect();
    loop {
        let found = find_code_block(&chars);
        let Some((open_pos, inner_start, inner_end, close_end)) = found else {
            break;
        };
        // inner is chars[inner_start..inner_end] rstrip "\n"
        let mut inner: Vec<char> = chars[inner_start..inner_end].to_vec();
        while inner.last() == Some(&'\n') {
            inner.pop();
        }
        let start = open_pos;
        let len = inner.len();
        // Replace text[open_pos..close_end] with inner
        let mut new_chars = Vec::with_capacity(chars.len() - (close_end - open_pos) + len);
        new_chars.extend_from_slice(&chars[0..open_pos]);
        new_chars.extend_from_slice(&inner);
        new_chars.extend_from_slice(&chars[close_end..]);
        chars = new_chars;
        styles.push((start, len, "MONOSPACE".to_string()));
    }
    current = chars.into_iter().collect();

    // Headings — mirrors Python heading loop
    let chars: Vec<char> = current.chars().collect();
    let heading_matches = find_heading_matches(&chars);
    let mut new_chars: Vec<char> = Vec::new();
    let mut last_end: usize = 0;
    let mut heading_styles: Vec<(usize, usize, String)> = Vec::new();
    for (ms, me, eol) in heading_matches {
        new_chars.extend_from_slice(&chars[last_end..ms]);
        last_end = me;
        let heading_text = &chars[me..eol];
        let start = new_chars.len();
        new_chars.extend_from_slice(heading_text);
        heading_styles.push((start, heading_text.len(), "BOLD".to_string()));
        last_end = eol;
    }
    new_chars.extend_from_slice(&chars[last_end..]);
    // styles already contains code block entries; add heading entries
    styles.extend(heading_styles);
    current = new_chars.into_iter().collect();

    // Inline patterns — mirrors Python patterns + occupied logic
    let mid_chars: Vec<char> = current.chars().collect();
    let patterns: Vec<(&str, fn(&[char]) -> Vec<(usize, usize, usize, usize)>)> = vec![
        ("BOLD", find_double_star as fn(&[char]) -> Vec<(usize, usize, usize, usize)>),
        ("BOLD", find_double_underscore),
        ("STRIKETHROUGH", find_double_tilde),
        ("MONOSPACE", find_backtick),
        ("ITALIC", find_single_star_italic),
        ("ITALIC", find_single_underscore_italic),
    ];

    let mut all_matches: Vec<(usize, usize, usize, usize, String)> = Vec::new();
    let mut occupied: Vec<(usize, usize)> = Vec::new();

    for (style, finder) in patterns {
        let matches = finder(&mid_chars);
        for (ms, me, g1s, g1e) in matches {
            let mut overlap = false;
            for (os, oe) in &occupied {
                if ms < *oe && me > *os {
                    overlap = true;
                    break;
                }
            }
            if !overlap {
                all_matches.push((ms, me, g1s, g1e, style.to_string()));
                occupied.push((ms, me));
            }
        }
    }
    all_matches.sort_by(|a, b| a.0.cmp(&b.0));

    // Removals for prior style adjustment
    let mut removals: Vec<(usize, usize)> = Vec::new();
    for (ms, me, g1s, g1e, _) in &all_matches {
        if *g1s > *ms {
            removals.push((*ms, g1s - ms));
        }
        if *me > *g1e {
            removals.push((*g1e, me - g1e));
        }
    }
    removals.sort_by(|a, b| a.0.cmp(&b.0));

    // _adjust helper
    let adjust = |pos: usize, removals: &[(usize, usize)]| -> usize {
        let mut shift = 0usize;
        for (rpos, rlen) in removals {
            if *rpos < pos {
                shift += std::cmp::min(*rlen, pos - rpos);
            } else {
                break;
            }
        }
        pos - shift
    };

    let mut adjusted_prior: Vec<(usize, usize, String)> = Vec::new();
    for (start, len, style) in styles {
        let new_start = adjust(start, &removals);
        let new_end = adjust(start + len, &removals);
        if new_end > new_start {
            adjusted_prior.push((new_start, new_end - new_start, style));
        }
    }

    // Build result stripping inline markers
    let mut result_chars: Vec<char> = Vec::new();
    let mut last_end: usize = 0;
    let mut inline_styles: Vec<(usize, usize, String)> = Vec::new();
    for (ms, me, g1s, g1e, style) in &all_matches {
        result_chars.extend_from_slice(&mid_chars[last_end..*ms]);
        let pos = result_chars.len();
        let inner = &mid_chars[*g1s..*g1e];
        result_chars.extend_from_slice(inner);
        inline_styles.push((pos, inner.len(), style.clone()));
        last_end = *me;
    }
    result_chars.extend_from_slice(&mid_chars[last_end..]);
    let final_text: String = result_chars.iter().collect();

    let mut combined = adjusted_prior;
    combined.extend(inline_styles);
    combined.sort_by(|a, b| a.0.cmp(&b.0));

    // Build style_strings with UTF-16 positions
    let final_chars: Vec<char> = final_text.chars().collect();
    let mut style_strings: Vec<String> = Vec::new();
    for (cp_start, cp_len, style_type) in combined {
        if cp_start > final_chars.len() || cp_start + cp_len > final_chars.len() {
            continue;
        }
        let u16_start = utf16_len_chars(&final_chars[0..cp_start]);
        let u16_len = utf16_len_chars(&final_chars[cp_start..cp_start + cp_len]);
        style_strings.push(format!("{}:{}:{}", u16_start, u16_len, style_type));
    }

    (final_text, style_strings)
}

// Provide private aliases mirroring Python's underscore-prefixed helpers for traceability
#[allow(dead_code)]
fn _utf16_len(s: &str) -> usize {
    utf16_len_str(s)
}

#[allow(dead_code)]
fn _normalize_bullet_markers(source: &str) -> String {
    normalize_bullet_markers(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_and_italic() {
        let (text, styles) = markdown_to_signal("**bold** and *italic*");
        assert_eq!(text, "bold and italic");
        // bold at 0:4, italic at 9:6 (utf16 same as char for ascii)
        assert!(styles.contains(&"0:4:BOLD".to_string()));
        assert!(styles.contains(&"9:6:ITALIC".to_string()));
    }

    #[test]
    fn code_block_monospace() {
        let (text, styles) = markdown_to_signal("```\nhello\n```");
        assert_eq!(text, "hello");
        assert!(styles.contains(&"0:5:MONOSPACE".to_string()));
    }

    #[test]
    fn heading_bold() {
        let (text, styles) = markdown_to_signal("# Title");
        assert_eq!(text, "Title");
        assert!(styles.contains(&"0:5:BOLD".to_string()));
    }

    #[test]
    fn bullet_normalization() {
        let (text, _) = markdown_to_signal("- item\n* item2");
        assert_eq!(text, "• item\n• item2");
    }

    #[test]
    fn utf16_emoji() {
        // Emoji outside BMP is 2 code units
        let (text, styles) = markdown_to_signal("**a😀b**");
        assert_eq!(text, "a😀b");
        // a(1) + emoji(2) + b(1) = 4 code units length
        // start 0 len 4
        assert!(styles.contains(&"0:4:BOLD".to_string()));
        // Inner "a😀b" length in utf16: a1 +2 +1 =4
        // Verify prefix calculation
        let (text2, styles2) = markdown_to_signal("x **a😀b**");
        // "x " is 2 chars, 2 units, bold starts at 2
        assert_eq!(text2, "x a😀b");
        assert!(styles2.contains(&"2:4:BOLD".to_string()));
    }

    #[test]
    fn strikethrough_and_monospace() {
        let (text, styles) = markdown_to_signal("~~strike~~ and `code`");
        assert_eq!(text, "strike and code");
        assert!(styles.contains(&"0:6:STRIKETHROUGH".to_string()));
        assert!(styles.contains(&"11:4:MONOSPACE".to_string()));
    }

    #[test]
    fn collapse_newlines() {
        let (text, _) = markdown_to_signal("a\n\n\n\nb");
        assert_eq!(text, "a\n\nb");
    }
}
