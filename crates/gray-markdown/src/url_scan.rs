use linkify::{LinkFinder, LinkKind};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::buffers::unicode_display_width;
use crate::output::{HyperlinkTarget, MarkdownRenderOutput};

fn link_style() -> Style {
    let fg = crate::style::palette()
        .map(|p| Color::Rgb(p.link[0], p.link[1], p.link[2]))
        .unwrap_or(Color::Rgb(125, 207, 255));
    Style::default().fg(fg).add_modifier(Modifier::UNDERLINED)
}

pub(crate) fn detect_plain_urls(
    lines: &[Line<'_>],
    existing: &[HyperlinkTarget],
    next_id: u32,
) -> (Vec<HyperlinkTarget>, u32) {
    detect_plain_urls_with_offset(lines, 0, existing, next_id)
}

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
                        if matches!(span_text.as_bytes().get(end), Some(b':' | b'/')) {
                            continue;
                        }
                        format!("mailto:{}", link.as_str())
                    }
                    _ => link.as_str().to_string(),
                };

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

            let bytes = span_text.as_bytes();
            let mut b = 0;
            while b < bytes.len() {
                let is_slash = bytes[b] == b'/';
                let is_tilde_slash =
                    b + 1 < bytes.len() && bytes[b] == b'~' && bytes[b + 1] == b'/';
                let start_b = if is_tilde_slash || is_slash {
                    b
                } else {
                    b += 1;
                    continue;
                };

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

                let mut end_b = start_b;
                if is_tilde_slash {
                    end_b += 2;
                } else {
                    end_b += 1;
                }
                while let Some(c) = span_text[end_b..].chars().next() {
                    if c.is_alphanumeric() || matches!(c, '/' | '.' | '-' | '_' | '~') {
                        end_b += c.len_utf8();
                    } else {
                        break;
                    }
                }

                let matched_len = end_b - start_b;
                let min_len = if is_tilde_slash { 3 } else { 2 };
                if matched_len < min_len {
                    b = start_b + 1;
                    continue;
                }

                while end_b > start_b {
                    let last_char = span_text[..end_b].chars().next_back().unwrap();
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

                if matched == "/" || matched == "~/" {
                    b = end_b;
                    continue;
                }
                let before = &span_text[..start_b];
                let col_start = display_col + unicode_display_width(before);
                let col_end = col_start + unicode_display_width(matched);

                let overlaps = existing.iter().chain(result.iter()).any(|h| {
                    h.line_index == line_index
                        && col_start < h.column_range.end
                        && h.column_range.start < col_end
                });
                if overlaps {
                    b = end_b;
                    continue;
                }

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

pub(crate) fn attach_links(output: &mut MarkdownRenderOutput, next_link_id: u32) -> u32 {
    let (urls, next) = detect_plain_urls(&output.lines, &output.hyperlinks, next_link_id);
    output.hyperlinks.extend(urls);
    let (paths, next) = detect_file_paths(&output.lines, &output.hyperlinks, next);
    output.hyperlinks.extend(paths);
    output
        .hyperlinks
        .sort_by_key(|h| (h.line_index, h.column_range.start));
    apply_link_styling(&mut output.lines, &output.hyperlinks);
    next
}

pub(crate) fn apply_link_styling(lines: &mut [Line<'_>], hyperlinks: &[HyperlinkTarget]) {
    use std::collections::HashMap;
    use unicode_width::UnicodeWidthChar;
    let link_style = link_style();
    let mut by_line: HashMap<usize, Vec<&HyperlinkTarget>> = HashMap::new();
    for h in hyperlinks {
        by_line.entry(h.line_index).or_default().push(h);
    }
    for (line_idx, mut hs) in by_line {
        if line_idx >= lines.len() {
            continue;
        }
        hs.sort_by_key(|h| h.column_range.start);

        let line = &mut lines[line_idx];

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

        for h in &hs {
            for (idx, &c_start) in col_for_char.iter().enumerate() {
                if c_start >= h.column_range.start && c_start < h.column_range.end {
                    styles[idx] = styles[idx].patch(link_style);
                }
            }
        }

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

        let line_style = line.style;
        *line = Line::from(new_spans).style(line_style);
    }
}
