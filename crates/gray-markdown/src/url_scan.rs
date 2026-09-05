//! Plain-URL detection over rendered display ratatui Lines.

use linkify::{LinkFinder, LinkKind};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::buffers::unicode_display_width;
use crate::output::HyperlinkTarget;

/// Style for file/web links — cyan + underlined, always. Mirrors `gray_markdown_style().link_text`.
fn link_style() -> Style {
    Style::default()
        .fg(Color::Rgb(125, 207, 255))
        .add_modifier(Modifier::UNDERLINED)
}

/// Scan `lines` for plain URLs and return new `HyperlinkTarget` entries
/// that don't overlap any existing target in `existing`.
///
/// `next_id` is the first id to assign; the returned `u32` is the
/// post-scan counter, suitable for stuffing back into
/// `FrozenState::next_link_id`.
pub(crate) fn detect_plain_urls(
    lines: &[Line<'_>],
    existing: &[HyperlinkTarget],
    next_id: u32,
) -> (Vec<HyperlinkTarget>, u32) {
    detect_plain_urls_with_offset(lines, 0, existing, next_id)
}

/// Like [`detect_plain_urls`] but scans `lines` whose first element
/// represents document line `line_index_offset` (caller passes a tail
/// slice of `self.output.lines` and the index of its first element).
///
/// Lines fully inside `0..line_index_offset` are assumed to be in
/// `existing` already and are not re-scanned.  The dedup overlap check
/// still works correctly because emitted targets use document-absolute
/// `line_index = line_index_offset + i`, matching the indices already
/// present in `existing`.
pub(crate) fn detect_plain_urls_with_offset(
    lines: &[Line<'_>],
    line_index_offset: usize,
    existing: &[HyperlinkTarget],
    next_id: u32,
) -> (Vec<HyperlinkTarget>, u32) {
    let mut result = Vec::new();
    let mut current_id = next_id;
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url, LinkKind::Email]);

    for (i, line) in lines.iter().enumerate() {
        let line_index = line_index_offset + i;
        let mut display_col: usize = 0;

        for span in &line.spans {
            let span_text: &str = span.content.as_ref();

            for link in finder.links(span_text) {
                let start = link.start();
                let end = link.end();
                if start > end
                    || end > span_text.len()
                    || !span_text.is_char_boundary(start)
                    || !span_text.is_char_boundary(end)
                {
                    continue;
                }
                let before = &span_text[..start];
                let matched = &span_text[start..end];

                let col_start = display_col + unicode_display_width(before);
                let col_end = col_start + unicode_display_width(matched);
                let url = match link.kind() {
                    LinkKind::Email => {
                        // `git@github.com:org/repo` is an scp remote, not mail.
                        if matches!(span_text.as_bytes().get(end), Some(b':' | b'/')) {
                            continue;
                        }
                        format!("mailto:{}", link.as_str())
                    }
                    _ => link.as_str().to_string(),
                };

                // Dedup: skip if any existing or already-added target overlaps
                // on the same line. Overlap: cand.start < ex.end && ex.start < cand.end.
                let overlaps = existing.iter().chain(result.iter()).any(|h| {
                    h.line_index == line_index
                        && col_start < h.column_range.end
                        && h.column_range.start < col_end
                });

                if !overlaps {
                    result.push(HyperlinkTarget {
                        line_index,
                        column_range: col_start..col_end,
                        url,
                        id: current_id,
                    });
                    current_id += 1;
                }
            }

            display_col += unicode_display_width(span_text);
        }
    }

    (result, current_id)
}

/// Scan `lines` for bare absolute file paths like `/home/user/file` or `~/path`
/// and return new `HyperlinkTarget` entries with `file://` URLs.
/// Overlaps with `existing` are skipped.
pub(crate) fn detect_file_paths(
    lines: &[Line<'_>],
    existing: &[HyperlinkTarget],
    next_id: u32,
) -> (Vec<HyperlinkTarget>, u32) {
    detect_file_paths_with_offset(lines, 0, existing, next_id)
}

pub(crate) fn detect_file_paths_with_offset(
    lines: &[Line<'_>],
    line_index_offset: usize,
    existing: &[HyperlinkTarget],
    next_id: u32,
) -> (Vec<HyperlinkTarget>, u32) {
    let mut result = Vec::new();
    let mut current_id = next_id;
    for (i, line) in lines.iter().enumerate() {
        let line_index = line_index_offset + i;
        let mut display_col: usize = 0;
        for span in &line.spans {
            let span_text: &str = span.content.as_ref();
            // Byte index scan for file paths
            let bytes = span_text.as_bytes();
            let mut b = 0;
            while b < bytes.len() {
                // Check for '/' at word boundary or '~/' at word boundary
                let is_slash = bytes[b] == b'/';
                let is_tilde_slash =
                    b + 1 < bytes.len() && bytes[b] == b'~' && bytes[b + 1] == b'/';
                let start_b = if is_tilde_slash || is_slash {
                    b
                } else {
                    b += 1;
                    continue;
                };
                // Word boundary check: previous char is whitespace or start or punctuation that allows path start
                if start_b > 0 {
                    let prev = bytes[start_b - 1];
                    if !(prev == b' '
                        || prev == b'\t'
                        || prev == b'\n'
                        || prev == b'('
                        || prev == b'['
                        || prev == b'"'
                        || prev == b'\''
                        || prev == b'`'
                        || prev == b'<')
                    {
                        b = start_b + 1;
                        continue;
                    }
                }
                // Find end: consume allowed path chars
                let mut end_b = start_b;
                if is_tilde_slash {
                    end_b += 2; // include ~/
                } else {
                    end_b += 1; // include /
                }
                while end_b < bytes.len() {
                    let c = bytes[end_b] as char;
                    if c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | '~') {
                        end_b += c.len_utf8();
                    } else {
                        break;
                    }
                }
                // Need at least one more segment char beyond initial '/' or '~/'
                let matched_len = end_b - start_b;
                let min_len = if is_tilde_slash { 3 } else { 2 }; // e.g. ~/x or /x
                if matched_len < min_len {
                    b = start_b + 1;
                    continue;
                }
                // Trim trailing punctuation . , ; : ! ? ) ] } ` ' " that is not part of path
                while end_b > start_b {
                    let last_char = span_text[end_b - 1..].chars().next_back().unwrap();
                    if matches!(
                        last_char,
                        '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '\'' | '"' | '`'
                    ) {
                        end_b -= last_char.len_utf8();
                    } else {
                        break;
                    }
                }
                if end_b <= start_b {
                    b = start_b + 1;
                    continue;
                }
                let matched = &span_text[start_b..end_b];
                // Skip if matched looks like just "/" or contains ".." weird?
                if matched == "/" || matched == "~/" {
                    b = end_b;
                    continue;
                }
                let before = &span_text[..start_b];
                let col_start = display_col + unicode_display_width(before);
                let col_end = col_start + unicode_display_width(matched);
                // Dedup against existing hyperlinks
                let overlaps = existing.iter().chain(result.iter()).any(|h| {
                    h.line_index == line_index
                        && col_start < h.column_range.end
                        && h.column_range.start < col_end
                });
                if overlaps {
                    b = end_b;
                    continue;
                }
                // Build file:// URL
                let path = if let Some(stripped) = matched.strip_prefix("~/") {
                    if let Ok(home) = std::env::var("HOME") {
                        format!("{}/{}", home.trim_end_matches('/'), stripped)
                    } else {
                        matched.to_string()
                    }
                } else {
                    matched.to_string()
                };
                let url = format!("file://{path}");
                result.push(HyperlinkTarget {
                    line_index,
                    column_range: col_start..col_end,
                    url,
                    id: current_id,
                });
                current_id += 1;
                b = end_b;
            }
            display_col += unicode_display_width(span_text);
        }
    }
    (result, current_id)
}

/// Apply `link_style` (cyan + underline) to `lines` for every `hyperlink` range.
/// This makes plain URLs and file paths visually identical to markdown `[text](url)` links.
pub(crate) fn patch_lines_with_link_style(
    lines: &mut [Line<'_>],
    hyperlinks: &[HyperlinkTarget],
    link_style: Style,
) {
    use std::collections::HashMap;
    use unicode_width::UnicodeWidthChar;
    let mut by_line: HashMap<usize, Vec<&HyperlinkTarget>> = HashMap::new();
    for h in hyperlinks {
        by_line.entry(h.line_index).or_default().push(h);
    }
    for (line_idx, mut hs) in by_line {
        if line_idx >= lines.len() {
            continue;
        }
        hs.sort_by_key(|h| h.column_range.start);
        // Flatten line to chars with styles and column positions
        let line = &mut lines[line_idx];
        // Build char vector: each char, its style, and its start column
        let mut chars: Vec<char> = Vec::new();
        let mut styles: Vec<Style> = Vec::new();
        let mut col_for_char: Vec<usize> = Vec::new();
        let mut col = 0usize;
        for span in &line.spans {
            for ch in span.content.chars() {
                let w = UnicodeWidthChar::width(ch).unwrap_or(0);
                chars.push(ch);
                styles.push(span.style);
                col_for_char.push(col);
                col += w;
            }
        }
        if chars.is_empty() {
            continue;
        }
        // Patch styles for chars whose start column lies within any hyperlink range
        for h in &hs {
            for (idx, &c_start) in col_for_char.iter().enumerate() {
                if c_start >= h.column_range.start && c_start < h.column_range.end {
                    styles[idx] = styles[idx].patch(link_style);
                }
            }
        }
        // Rebuild spans: group consecutive chars with same style
        let mut new_spans: Vec<Span<'static>> = Vec::new();
        let mut cur_style = styles[0];
        let mut cur_buf = String::new();
        for (i, &ch) in chars.iter().enumerate() {
            if styles[i] != cur_style {
                new_spans.push(Span::styled(cur_buf, cur_style));
                cur_buf = String::new();
                cur_style = styles[i];
            }
            cur_buf.push(ch);
        }
        if !cur_buf.is_empty() {
            new_spans.push(Span::styled(cur_buf, cur_style));
        }
        // Preserve line-level style
        let line_style = line.style;
        *line = Line::from(new_spans).style(line_style);
    }
}

pub(crate) fn apply_link_styling(lines: &mut [Line<'_>], hyperlinks: &[HyperlinkTarget]) {
    patch_lines_with_link_style(lines, hyperlinks, link_style());
}
