//! Transcript row helpers: wrapping, prompt formatting (split from `transcript`).

use super::*;

pub(crate) fn thinking_style() -> Style {
    Style::default()
        .fg(Color::Rgb(140, 140, 140))
        .add_modifier(Modifier::ITALIC)
}

/// Left padding, omp-style: routed through the global tight flag.
pub(crate) fn left_pad() -> Span<'static> {
    Span::raw(" ".repeat(crate::tui::padding_x(1)))
}

pub(crate) fn strip_ansi(s: &str) -> String {
    crate::tui::strip_ansi(s)
}

fn str_display_width(s: &str) -> usize {
    crate::tui::visible_width(s)
}

/// Redact secrets from slash-command echo cards: `/gateway connect <platform> <token>`
/// renders as `/gateway connect <platform> ••••`, same for `/gateway pairing approve`
/// codes. Everything else passes through untouched. Execution always uses the raw
/// string — only the visible card is redacted.
pub fn redact_command_echo(text: &str) -> String {
    let mut it = text.split_whitespace();
    match (it.next(), it.next(), it.next(), it.next(), it.next()) {
        (Some("/gateway"), Some("connect"), Some(plat), Some(_), _) => {
            format!("/gateway connect {plat} ••••")
        }
        (Some("/gateway"), Some("pairing"), Some("approve"), Some(plat), Some(_)) => {
            format!("/gateway pairing approve {plat} ••••")
        }
        _ => text.to_string(),
    }
}

fn slice_line_spans<'a>(
    original: &'a Line<'a>,
    span_bounds: &[(Range<usize>, Style)],
    range: &Range<usize>,
) -> Line<'a> {
    let start_byte = range.start;
    let end_byte = range.end;
    let mut acc: Vec<Span<'a>> = Vec::new();
    for (i, (r, style)) in span_bounds.iter().enumerate() {
        let s = r.start;
        let e = r.end;
        if e <= start_byte {
            continue;
        }
        if s >= end_byte {
            break;
        }
        let seg_start = start_byte.max(s);
        let seg_end = end_byte.min(e);
        if seg_end > seg_start {
            let local_start = seg_start - s;
            let local_end = seg_end - s;
            let content = original.spans[i].content.as_ref();
            // ensure boundaries are char boundaries (they should be since we only cut at word boundaries which are char boundaries)
            let slice = &content[local_start..local_end];
            acc.push(Span {
                style: *style,
                content: std::borrow::Cow::Borrowed(slice),
            });
        }
        if e >= end_byte {
            break;
        }
    }
    Line {
        style: original.style,
        alignment: original.alignment,
        spans: acc,
    }
}

/// Word-aware wrapping: preserves styles, respects hyperlink guards,
/// splits at word boundaries (space) and falls back to char chunk for long words.
pub(crate) fn wrap_styled_line(line: Line<'static>, max_w: usize) -> Vec<Line<'static>> {
    wrap_styled_line_with_ranges(line, max_w)
        .into_iter()
        .map(|(l, _)| l)
        .collect()
}

/// Same as [`wrap_styled_line`] but also returns each output row's source
/// byte range on the original (unwrapped) flat line, so callers can map
/// absolute columns (hyperlinks) onto wrapped rows.
pub(crate) fn wrap_styled_line_with_ranges(
    line: Line<'static>,
    max_w: usize,
) -> Vec<(Line<'static>, Range<usize>)> {
    // Don't wrap lines with OSC 8 hyperlinks
    if line.spans.iter().any(|s| s.content.contains("\x1b]8;;")) {
        return vec![(line, 0..usize::MAX)];
    }
    if line.width() <= max_w {
        return vec![(line, 0..usize::MAX)];
    }
    // Flatten line for word analysis
    let mut flat = String::new();
    let mut span_bounds: Vec<(Range<usize>, Style)> = Vec::new();
    let mut acc = 0usize;
    for s in &line.spans {
        let text = s.content.as_ref();
        let start = acc;
        flat.push_str(text);
        acc += text.len();
        span_bounds.push((start..acc, s.style));
    }
    if flat.is_empty() {
        return vec![(line, 0..usize::MAX)];
    }

    // Detect if this line has a gutter prefix (e.g. " 12 | " or "    | ")
    let gutter_info = if let Some(bar_idx) = flat.find(" | ") {
        if bar_idx <= 14
            && flat[..bar_idx]
                .chars()
                .all(|c| c.is_ascii_digit() || c == ' ')
        {
            let gutter_end = bar_idx + 3;
            let gutter_style = span_bounds
                .iter()
                .find(|(r, _)| r.start <= bar_idx && bar_idx < r.end)
                .map(|(_, s)| *s)
                .unwrap_or(line.style);
            let cont_gutter_str = format!("{:>width$} | ", "", width = bar_idx);
            Some((gutter_end, cont_gutter_str, gutter_style))
        } else {
            None
        }
    } else {
        None
    };

    let (content_start, cont_gutter_str, gutter_style, eff_max_w) =
        if let Some((g_end, ref c_str, g_style)) = gutter_info {
            let g_width = str_display_width(&flat[..g_end]);
            let avail = max_w.saturating_sub(g_width).max(10);
            (g_end, Some(c_str.clone()), Some(g_style), avail)
        } else {
            (0, None, None, max_w)
        };

    // Tokenize content into words (non-space runs) with byte ranges
    let mut words: Vec<Range<usize>> = Vec::new();
    let mut i = content_start;
    while i < flat.len() {
        while i < flat.len() && flat[i..].starts_with(' ') {
            let ch = flat[i..].chars().next().unwrap();
            i += ch.len_utf8();
        }
        if i >= flat.len() {
            break;
        }
        let start = i;
        while i < flat.len() {
            let ch = flat[i..].chars().next().unwrap();
            if ch == ' ' {
                break;
            }
            i += ch.len_utf8();
        }
        words.push(start..i);
    }
    if words.is_empty() {
        // column mapping is approximate here; word-split lines are the norm.
        return char_chunk_fallback(line, max_w, flat)
            .into_iter()
            .map(|l| (l, 0..usize::MAX))
            .collect();
    }

    // Build wrapped ranges word-aware
    let mut out_ranges: Vec<Range<usize>> = Vec::new();
    let mut cur_start: Option<usize> = None;
    let mut cur_end: usize = 0;
    let mut cur_w: usize = 0;
    for w_range in words {
        let word_str = &flat[w_range.clone()];
        let word_w = str_display_width(word_str);
        if word_w > eff_max_w {
            if let Some(s) = cur_start.take() {
                out_ranges.push(s..cur_end);
                cur_w = 0;
            }
            let chars: Vec<char> = word_str.chars().collect();
            let mut byte_offset = w_range.start;
            let mut idx = 0;
            while idx < chars.len() {
                let take = eff_max_w.min(chars.len() - idx);
                let chunk_chars = &chars[idx..idx + take];
                let chunk_str: String = chunk_chars.iter().collect();
                let byte_len = chunk_str.len();
                out_ranges.push(byte_offset..byte_offset + byte_len);
                byte_offset += byte_len;
                idx += take;
            }
            continue;
        }
        if cur_start.is_none() {
            cur_start = Some(w_range.start);
            cur_end = w_range.end;
            cur_w = word_w;
        } else {
            let needed = 1 + word_w;
            if cur_w + needed <= eff_max_w {
                cur_end = w_range.end;
                cur_w += needed;
            } else {
                out_ranges.push(cur_start.take().unwrap()..cur_end);
                cur_start = Some(w_range.start);
                cur_end = w_range.end;
                cur_w = word_w;
            }
        }
    }
    if let Some(s) = cur_start {
        out_ranges.push(s..cur_end);
    }
    if out_ranges.is_empty() {
        return vec![(Line::from("").style(line.style), 0..0)];
    }

    // Convert ranges to Lines preserving styles and prepending gutter
    let mut result: Vec<(Line<'static>, Range<usize>)> = Vec::new();
    for (row_idx, r) in out_ranges.into_iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if let (Some(c_str), Some(g_style)) = (&cont_gutter_str, gutter_style) {
            if row_idx == 0 {
                let g_sliced = slice_line_spans(&line, &span_bounds, &(0..content_start));
                for s in g_sliced.spans {
                    spans.push(Span::styled(
                        s.content.into_owned(),
                        s.style.patch(line.style),
                    ));
                }
            } else {
                spans.push(Span::styled(c_str.clone(), g_style.patch(line.style)));
            }
        }
        let sliced = slice_line_spans(&line, &span_bounds, &r);
        for s in sliced.spans {
            spans.push(Span::styled(
                s.content.into_owned(),
                s.style.patch(line.style),
            ));
        }
        let mut new_line = Line::from(spans).style(line.style);
        new_line.alignment = line.alignment;
        result.push((new_line, r));
    }
    if result.is_empty() {
        result.push((Line::from("").style(line.style), 0..0));
    }
    result
}

fn char_chunk_fallback(line: Line<'static>, max_w: usize, _flat: String) -> Vec<Line<'static>> {
    let line_style = line.style;
    let mut result = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_w = 0usize;
    for span in line.spans {
        let span_w = span.width();
        if current_w + span_w <= max_w {
            current_w += span_w;
            current_spans.push(span);
        } else {
            let chars: Vec<char> = span.content.chars().collect();
            let style = span.style;
            let mut i = 0;
            while i < chars.len() {
                let avail = max_w.saturating_sub(current_w);
                if avail == 0 {
                    if !current_spans.is_empty() {
                        result
                            .push(Line::from(std::mem::take(&mut current_spans)).style(line_style));
                    }
                    current_w = 0;
                    continue;
                }
                let take = avail.min(chars.len() - i);
                let chunk: String = chars[i..i + take].iter().collect();
                current_spans.push(Span::styled(chunk, style));
                current_w += take;
                i += take;
                if current_w >= max_w {
                    result.push(Line::from(std::mem::take(&mut current_spans)).style(line_style));
                    current_w = 0;
                }
            }
        }
    }
    if !current_spans.is_empty() {
        result.push(Line::from(current_spans).style(line_style));
    }
    if result.is_empty() {
        result.push(Line::from("").style(line_style));
    }
    result
}

/// Cut point for flushing the live thinking buffer: the last space within
/// the first `max_w` chars so rows break between words, never mid-word
/// ("r|espond"). Falls back to a hard cut at `max_w` when there is no
/// space (single overlong word) — same as the wrapper's long-word path.
pub(crate) fn word_flush_cut(chars: &[char], max_w: usize) -> usize {
    let end = max_w.min(chars.len());
    if end < chars.len()
        && let Some(sp) = chars[..end].iter().rposition(|c| *c == ' ')
        && sp > 0
    {
        return sp + 1; // keep the space at the row end; rest starts clean
    }
    end
}

pub(crate) fn format_user_prompt_lines(
    text: &str,
    attached: &[std::path::PathBuf],
    width: usize,
) -> Vec<Line<'static>> {
    let sanitized = crate::tui::sanitize_user_text(text);
    let prompt_color = Color::Rgb(180, 180, 180);
    let text_primary = Color::Rgb(225, 225, 225);
    let dim_color = Color::Rgb(140, 140, 140);
    let bg_style = Style::default().bg(Color::Rgb(22, 22, 22));
    let mut lines = Vec::new();
    lines.push(Line::from("").style(bg_style));
    let arrow_span = Span::styled(
        " ❯ ",
        Style::default()
            .fg(prompt_color)
            .add_modifier(Modifier::BOLD),
    );
    let max_w = width.saturating_sub(4).max(1);
    let lines_raw: Vec<&str> = sanitized.split('\n').collect();
    for (i, raw_line) in lines_raw.iter().enumerate() {
        let prefix = if i == 0 {
            arrow_span.clone()
        } else {
            Span::raw("   ")
        };
        if raw_line.is_empty() {
            lines.push(Line::from(vec![prefix]).style(bg_style));
        } else {
            let chars: Vec<char> = raw_line.chars().collect();
            let mut start = 0usize;
            let mut first_row = true;
            while start < chars.len() {
                // Prefer a word boundary (last space in the window) over a
                // mid-word char cut; hard-cut only a single overlong word.
                let mut end = (start + max_w).min(chars.len());
                if end < chars.len()
                    && let Some(sp) = chars[start..end].iter().rposition(|c| *c == ' ')
                    && sp > 0
                {
                    end = start + sp + 1;
                }
                let row_prefix = if first_row {
                    prefix.clone()
                } else {
                    Span::raw("   ")
                };
                first_row = false;
                lines.push(
                    Line::from(vec![
                        row_prefix,
                        Span::styled(
                            chars[start..end].iter().collect::<String>(),
                            Style::default().fg(text_primary),
                        ),
                    ])
                    .style(bg_style),
                );
                start = end;
            }
        }
    }
    if !attached.is_empty() {
        let names = attached
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(
            Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    format!("↳ attached: {names}"),
                    Style::default().fg(dim_color),
                ),
            ])
            .style(bg_style),
        );
    }
    lines.push(Line::from("").style(bg_style));
    lines
}
