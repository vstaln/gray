use super::*;
use crate::text_width::{chars_width, fit_char_count, word_window_end};

pub(crate) fn thinking_style() -> Style {
    Style::default()
        .fg(crate::theme::theme().text_muted)
        .add_modifier(Modifier::ITALIC)
}

pub(crate) fn transcript_row_is_blank(line: &Line<'static>) -> bool {
    line.style.bg.is_none()
        && line
            .spans
            .iter()
            .all(|s| s.style.bg.is_none() && s.content.trim().is_empty())
}

pub(crate) const GUTTER: usize = 1;

pub(crate) const RIGHT_MARGIN: usize = 1;

pub(crate) fn prose_width(width: usize) -> usize {
    width.saturating_sub(GUTTER + RIGHT_MARGIN).max(1)
}

pub(crate) fn left_pad() -> Span<'static> {
    Span::raw(" ".repeat(GUTTER))
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

pub(crate) fn wrap_styled_line(line: Line<'static>, max_w: usize) -> Vec<Line<'static>> {
    wrap_styled_line_with_ranges(line, max_w)
        .into_iter()
        .map(|(l, _)| l)
        .collect()
}

pub(crate) fn wrap_styled_line_with_ranges(
    line: Line<'static>,
    max_w: usize,
) -> Vec<(Line<'static>, Range<usize>)> {
    if line.spans.iter().any(|s| s.content.contains("\x1b]8;;")) {
        return vec![(line, 0..usize::MAX)];
    }
    if line.width() <= max_w {
        return vec![(line, 0..usize::MAX)];
    }

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

    let gutter_info = if let Some(bar_idx) = flat.find(" | ") {
        if bar_idx <= 24
            && flat[..bar_idx]
                .chars()
                .all(|c| c.is_ascii_digit() || c == ' ')
        {
            let mut gutter_end = bar_idx + 3;
            let mut cont_pad = 0;
            if let Some(rest) = flat.get(gutter_end..)
                && (rest.starts_with("+ ") || rest.starts_with("- "))
            {
                gutter_end += 2;
                cont_pad = 2;
            }
            let gutter_style = span_bounds
                .iter()
                .find(|(r, _)| r.start <= bar_idx && bar_idx < r.end)
                .map(|(_, s)| *s)
                .unwrap_or(line.style);
            let cont_gutter_str = format!(
                "{:>width$} | {:pad$}",
                "",
                "",
                width = bar_idx,
                pad = cont_pad
            );
            Some((gutter_end, cont_gutter_str, gutter_style))
        } else {
            None
        }
    } else {
        None
    };

    let lead_end = if gutter_info.is_none() {
        let ws = flat.len() - flat.trim_start().len();
        ws + list_marker_len(&flat[ws..])
    } else {
        0
    };
    let lead_w = crate::tui::visible_width(&flat[..lead_end]);
    let lead_style = span_bounds.first().map(|(_, s)| *s).unwrap_or(line.style);

    let (content_start, cont_gutter_str, gutter_style, eff_max_w) =
        if let Some((g_end, ref c_str, g_style)) = gutter_info {
            let g_width = crate::tui::visible_width(&flat[..g_end]);
            let avail = max_w.saturating_sub(g_width).max(10);
            (g_end, Some(c_str.clone()), Some(g_style), avail)
        } else {
            (lead_end, None, None, max_w.saturating_sub(lead_w).max(10))
        };

    let mut words: Vec<Range<usize>> = Vec::new();
    let mut i = content_start;
    while i < flat.len() {
        while i < flat.len() && flat[i..].chars().next().is_some_and(|c| c.is_whitespace()) {
            let ch = flat[i..].chars().next().unwrap();
            i += ch.len_utf8();
        }
        if i >= flat.len() {
            break;
        }
        let start = i;
        while i < flat.len() {
            let ch = flat[i..].chars().next().unwrap();
            if ch.is_whitespace() {
                break;
            }
            i += ch.len_utf8();
        }
        words.push(start..i);
    }
    if words.is_empty() {
        return char_chunk_fallback(line, max_w)
            .into_iter()
            .map(|l| (l, 0..usize::MAX))
            .collect();
    }

    let mut out_ranges: Vec<Range<usize>> = Vec::new();
    let mut cur_start: Option<usize> = None;
    let mut cur_end: usize = 0;
    let mut cur_w: usize = 0;
    for w_range in words {
        let word_str = &flat[w_range.clone()];
        let word_w = crate::tui::visible_width(word_str);
        if word_w > eff_max_w {
            if let Some(s) = cur_start.take() {
                out_ranges.push(s..cur_end);
                cur_w = 0;
            }
            let chars: Vec<char> = word_str.chars().collect();
            let mut byte_offset = w_range.start;
            let mut idx = 0;
            while idx < chars.len() {
                let take = fit_char_count(&chars[idx..], eff_max_w);
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

    let mut result: Vec<(Line<'static>, Range<usize>)> = Vec::new();
    for (row_idx, r) in out_ranges.into_iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut slice_range = r.clone();
        let mut out_range = r.clone();
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
        } else if lead_end > 0 {
            if row_idx == 0 {
                slice_range = 0..r.end;
                out_range = 0..r.end;
            } else {
                spans.push(Span::styled(
                    " ".repeat(lead_w),
                    lead_style.patch(line.style),
                ));
                out_range = r.start.saturating_sub(lead_w)..r.end;
            }
        }
        let sliced = slice_line_spans(&line, &span_bounds, &slice_range);
        for s in sliced.spans {
            spans.push(Span::styled(
                s.content.into_owned(),
                s.style.patch(line.style),
            ));
        }
        let mut new_line = Line::from(spans).style(line.style);
        new_line.alignment = line.alignment;
        result.push((new_line, out_range));
    }
    if result.is_empty() {
        result.push((Line::from("").style(line.style), 0..0));
    }
    result
}

fn list_marker_len(rest: &str) -> usize {
    for bullet in ["• ", "- ", "* ", "+ "] {
        if rest.starts_with(bullet) {
            return bullet.len();
        }
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=3).contains(&digits) && [". ", ") "].iter().any(|m| rest[digits..].starts_with(m)) {
        return digits + 2;
    }
    0
}

fn char_chunk_fallback(line: Line<'static>, max_w: usize) -> Vec<Line<'static>> {
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
                let take = fit_char_count(&chars[i..], avail);
                let chunk: String = chars[i..i + take].iter().collect();
                current_spans.push(Span::styled(chunk, style));
                current_w += chars_width(&chars[i..i + take]);
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

pub(crate) fn split_thinking_run(text: &str, mut tail_blank: bool) -> Vec<String> {
    let mut pieces: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        pieces.pop();
    }
    let mut out = Vec::new();
    for p in pieces {
        let line = p.trim_end_matches('\r');
        if line.trim().is_empty() {
            if tail_blank {
                continue;
            }
            tail_blank = true;
        } else {
            tail_blank = false;
        }
        out.push(line.to_string());
    }
    out
}

pub(crate) fn thinking_run_rows(text: &str, max_w: usize, tail_blank: bool) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for logical in split_thinking_run(text, tail_blank) {
        let line = Line::from(vec![Span::styled(logical, thinking_style())]);
        for mut row in wrap_styled_line(line, max_w) {
            if !row.spans.is_empty() {
                row.spans.insert(0, left_pad());
            }
            out.push(row);
        }
    }
    out
}

pub(crate) fn word_flush_cut(chars: &[char], max_w: usize) -> usize {
    let fit = fit_char_count(chars, max_w).min(chars.len());
    if fit > 0 && chars.get(fit) == Some(&' ') {
        let mut end = fit;
        while chars.get(end) == Some(&' ') {
            end += 1;
        }
        return end;
    }
    word_window_end(chars, 0, max_w)
}

pub(crate) fn thinking_paint_text(fragment: &str, mid_line: bool) -> Option<&str> {
    if !mid_line {
        return Some(fragment);
    }
    let rest = fragment.trim_start_matches(' ');
    (!rest.trim().is_empty()).then_some(rest)
}

pub(crate) fn thinking_replay_lines(text: &str) -> Vec<Line<'static>> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    text.lines()
        .map(|l| Line::from(vec![Span::styled(l.to_string(), thinking_style())]))
        .collect()
}

pub(crate) fn format_user_prompt_lines(
    text: &str,
    attached: &[std::path::PathBuf],
    width: usize,
) -> Vec<Line<'static>> {
    let sanitized = crate::tui::sanitize_user_text(text);
    let prompt_color = crate::theme::theme().text_soft;
    let text_primary = crate::theme::theme().text_body;
    let bg_style = Style::default().bg(crate::theme::theme().surface_bg);

    let margin_row = || -> Line<'static> {
        Line::from(Span::styled(" ".repeat(width.max(1)), bg_style)).style(bg_style)
    };
    let mut lines = Vec::new();

    let hang = " ".repeat(GUTTER + 2);
    let arrow_span = Span::styled(
        format!("{}{} ", " ".repeat(GUTTER), crate::tui::arrow_glyph()),
        Style::default()
            .fg(prompt_color)
            .add_modifier(Modifier::BOLD),
    );
    let max_w = width.saturating_sub(GUTTER + 2 + RIGHT_MARGIN).max(1);
    let lines_raw: Vec<&str> = sanitized.split('\n').collect();
    for (i, raw_line) in lines_raw.iter().enumerate() {
        let prefix = if i == 0 {
            arrow_span.clone()
        } else {
            Span::raw(hang.clone())
        };
        if raw_line.is_empty() {
            lines.push(Line::from(vec![prefix]).style(bg_style));
        } else {
            let mut cells: Vec<crate::composer::draw::widgets::InputCell> = raw_line
                .chars()
                .map(|c| (c, Style::default().fg(text_primary)))
                .collect();
            crate::composer::draw::widgets::restyle_attachment_markers(&mut cells);
            let chars: Vec<char> = cells.iter().map(|c| c.0).collect();
            let mut start = 0usize;
            let mut first_row = true;
            while start < chars.len() {
                let end = word_window_end(&chars, start, max_w);
                let row_prefix = if first_row {
                    prefix.clone()
                } else {
                    Span::raw(hang.clone())
                };
                first_row = false;
                let mut spans = vec![row_prefix];
                spans.extend(crate::composer::draw::widgets::styled_runs(
                    &cells[start..end],
                ));
                lines.push(Line::from(spans).style(bg_style));
                start = end;
            }
        }
    }
    let previews = attached
        .iter()
        .filter_map(|path| image_preview_lines(path, max_w))
        .collect();
    for band in tile_previews(previews, max_w) {
        if !lines.is_empty() {
            lines.push(margin_row());
        }
        lines.extend(band);
    }
    lines.insert(0, margin_row());
    lines.push(margin_row());
    lines
}

pub(crate) fn viewed_image_lines(
    name: &str,
    args: Option<&serde_json::Value>,
    cwd: &std::path::Path,
    max_w: usize,
) -> Option<Vec<Line<'static>>> {
    let args = args?;
    const MAX_PREVIEWS: usize = 4;
    let collected: Vec<String> = if matches!(name, "view" | "read") {
        args.get("path")
            .and_then(|v| v.as_str())
            .into_iter()
            .chain(
                args.get("paths")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str()),
            )
            .map(String::from)
            .collect()
    } else if matches!(name, "bash" | "shell") {
        args.get("command")
            .and_then(|v| v.as_str())
            .and_then(gray_tools::shell::label::read_only)
            .filter(|r| r.look == gray_tools::shell::label::Look::Viewed)
            .map(|r| r.targets)
            .unwrap_or_default()
    } else {
        return None;
    };
    let raw_paths = collected.into_iter().take(MAX_PREVIEWS);
    let mut previews = Vec::new();
    for raw in raw_paths {
        let path = std::path::Path::new(&raw);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        let is_image = gray_tools::images::is_image_extension(&path);
        if !is_image {
            continue;
        }
        previews.extend(image_preview_lines(&path, max_w));
    }
    let mut lines = Vec::new();
    for band in tile_previews(previews, max_w) {
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.extend(band);
    }
    (!lines.is_empty()).then_some(lines)
}

const PREVIEW_GAP: usize = 2;

/// Lay image previews out left to right, `PREVIEW_GAP` cells apart, starting
/// a new band when the next one would pass `max_w`. Each preview row is an
/// indent span followed by one span per cell, so a row's width is its span
/// count minus one; shorter previews are padded with blank cells.
fn tile_previews(previews: Vec<Vec<Line<'static>>>, max_w: usize) -> Vec<Vec<Line<'static>>> {
    let surface = Style::default().bg(crate::theme::theme().surface_bg);
    let blank = |n: usize| Span::styled(" ".repeat(n), surface);
    let mut bands: Vec<Vec<(usize, Vec<Line<'static>>)>> = Vec::new();
    let mut used = 0;
    for p in previews {
        let w = p
            .iter()
            .map(|l| l.spans.len().saturating_sub(1))
            .max()
            .unwrap_or(0);
        match bands.last_mut() {
            Some(band) if used + PREVIEW_GAP + w <= max_w => {
                used += PREVIEW_GAP + w;
                band.push((w, p));
            }
            _ => {
                used = w;
                bands.push(vec![(w, p)]);
            }
        }
    }
    bands
        .into_iter()
        .map(|band| {
            let height = band.iter().map(|(_, p)| p.len()).max().unwrap_or(0);
            (0..height)
                .map(|y| {
                    let mut spans = vec![blank(GUTTER + 2)];
                    for (i, (w, p)) in band.iter().enumerate() {
                        if i > 0 {
                            spans.push(blank(PREVIEW_GAP));
                        }
                        let cells: Vec<Span<'static>> = p
                            .get(y)
                            .map(|l| l.spans.iter().skip(1).cloned().collect())
                            .unwrap_or_default();
                        let pad = w.saturating_sub(cells.len());
                        spans.extend(cells);
                        if pad > 0 {
                            spans.push(blank(pad));
                        }
                    }
                    Line::from(spans)
                })
                .collect()
        })
        .collect()
}

fn image_preview_lines(path: &std::path::Path, max_w: usize) -> Option<Vec<Line<'static>>> {
    if let Some(rows) = crate::quality_of_life::kitty::preview_lines(path, max_w, GUTTER + 2) {
        return Some(rows);
    }
    let img = image::open(path).ok()?;
    let cols = max_w.clamp(8, 48) as u32;
    let thumb = img.thumbnail(cols, 48).to_rgba8();
    let (tw, th) = thumb.dimensions();
    if tw == 0 || th == 0 {
        return None;
    }
    let surface = crate::theme::theme().surface_bg;
    let to_color = |px: &image::Rgba<u8>| -> ratatui::style::Color {
        if px[3] < 128 {
            surface
        } else {
            ratatui::style::Color::Rgb(px[0], px[1], px[2])
        }
    };
    let hang = " ".repeat(GUTTER + 2);
    let mut lines = Vec::new();
    let mut y = 0u32;
    while y < th {
        let mut spans = vec![Span::styled(hang.clone(), Style::default().bg(surface))];
        for x in 0..tw {
            let top = thumb.get_pixel(x, y);
            let bottom = thumb.get_pixel(x, (y + 1).min(th - 1));
            spans.push(Span::styled(
                "\u{2580}",
                Style::default().fg(to_color(top)).bg(to_color(bottom)),
            ));
        }
        lines.push(Line::from(spans));
        y += 2;
    }
    Some(lines)
}
