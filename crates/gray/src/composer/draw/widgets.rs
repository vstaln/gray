use super::*;

pub(crate) fn shimmer_spans(text: &str, elapsed: Duration) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let sweep_seconds = 2.0f32;
    let pos = ((elapsed.as_secs_f32() % sweep_seconds) / sweep_seconds * (period as f32)) as usize;
    let band_half_width = 5.0f32;

    let base = crate::theme::theme().shimmer_base;
    let hilite = crate::theme::theme().text_bright;
    chars
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let dist = ((i as isize + padding as isize) - pos as isize).abs() as f32;
            let t = if dist <= band_half_width {
                0.5 * (1.0 + (std::f32::consts::PI * (dist / band_half_width)).cos())
            } else {
                0.0
            };
            let k = t.clamp(0.0, 1.0) * 0.9;
            let style = Style::default()
                .fg(crate::tui::blend_color(base, hilite, k))
                .add_modifier(if t > 0.3 {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

pub(crate) struct InputBox {
    pub(crate) lines: Vec<Line<'static>>,

    pub(crate) band_h: usize,

    pub(crate) lead_h: usize,

    pub(crate) x: usize,
    pub(crate) width: usize,
    pub(crate) cur_row: usize,
    pub(crate) cur_col: usize,
}

pub(crate) type InputCell = (char, Style);

fn marker_kind_label(marker_text: &str) -> &'static str {
    match marker_text
        .get(1..marker_text.find(':').unwrap_or(1))
        .unwrap_or_default()
    {
        "image" => "Image",
        "video" => "Video",
        "audio" => "Audio",
        "pdf" => "PDF",
        _ => "File",
    }
}

pub(crate) fn chip_cells(
    chips: &[(&str, &str)],
    pill: Style,
    label: Style,
    gap: Style,
) -> Vec<InputCell> {
    let mut cells: Vec<InputCell> = Vec::new();
    for (i, (kind, name)) in chips.iter().enumerate() {
        if i > 0 {
            cells.push((' ', gap));
        }
        cells.extend(format!(" {kind} ").chars().map(|c| (c, pill)));
        cells.extend(format!(" {name} ").chars().map(|c| (c, label)));
    }
    cells
}

type WrapResult = (Vec<(usize, usize)>, Option<(usize, usize)>);

pub(crate) fn wrap_line(chars: &[char], content_w: usize, cursor: Option<usize>) -> WrapResult {
    let mut rows: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = crate::text_width::word_window_end(chars, start, content_w);
        rows.push((start, end));
        start = end;
    }
    if rows.is_empty() {
        rows.push((0, 0));
    }
    let at = cursor.map(|c| {
        let row = rows
            .iter()
            .position(|&(_, end)| c <= end)
            .unwrap_or(rows.len() - 1);
        let (s, e) = rows[row];
        (
            row,
            crate::text_width::chars_width(&chars[s..c.clamp(s, e)]),
        )
    });
    (rows, at)
}

fn draft_cursor(text: &str, cursor: usize) -> (usize, usize) {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut line_start = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let line_end = line_start + line.len();
        if cursor <= line_end || i + 1 == lines.len() {
            let within = cursor.saturating_sub(line_start).min(line.len());
            let chars = line.char_indices().take_while(|(b, _)| *b < within).count();
            return (i, chars);
        }
        line_start = line_end + 1;
    }
    (0, 0)
}

pub(crate) fn styled_runs(cells: &[InputCell]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut style: Option<Style> = None;
    for &(ch, st) in cells {
        if style != Some(st) {
            if let Some(prev) = style {
                spans.push(Span::styled(std::mem::take(&mut run), prev));
            }
            style = Some(st);
        }
        run.push(ch);
    }
    if let Some(prev) = style {
        spans.push(Span::styled(run, prev));
    }
    spans
}

pub(crate) fn restyle_attachment_markers(cells: &mut [InputCell]) {
    let t = crate::theme::theme();
    let pill = Style::default().fg(t.text_bright).bg(t.input_bg);
    let label = Style::default().fg(t.text_muted).bg(t.surface_bg);
    let n = cells.len();
    let mut i = 0;
    while i < n {
        if cells[i].0 != '[' {
            i += 1;
            continue;
        }
        let matched = crate::repl::attachments::MARKER_PREFIXES.iter().find(|p| {
            p.chars()
                .enumerate()
                .all(|(k, c)| cells.get(i + 1 + k).is_some_and(|cell| cell.0 == c))
        });
        let Some(prefix) = matched else {
            i += 1;
            continue;
        };
        let klen = prefix.len() - 2;
        let name_start = i + 1 + prefix.len();
        let mut j = name_start;
        while j < n && cells[j].0 != ']' && cells[j].0 != '\n' {
            j += 1;
        }
        if j < n && cells[j].0 == ']' && j > name_start {
            cells[i].0 = ' ';
            for (k, c) in prefix[..klen].to_uppercase().chars().enumerate() {
                cells[i + 1 + k].0 = c;
            }
            cells[i + klen + 1].0 = ' ';
            cells[j].0 = ' ';
            for cell in &mut cells[i..i + klen + 2] {
                cell.1 = pill;
            }
            for cell in &mut cells[i + klen + 2..=j] {
                cell.1 = label;
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
}

pub(crate) fn build_input_box(
    text: &str,
    cursor: usize,
    w: usize,
    ghost: Option<&str>,
    home: Option<HomeRows>,
    chips: &[InputCell],
) -> InputBox {
    let (x, w) = home.map_or((0, w), |_| crate::composer::home_column(w));
    let content_w = w.saturating_sub(4).max(1);

    let prompt_color = crate::theme::theme().text_soft;
    let text_primary = crate::theme::theme().text_body;
    let text_dim = crate::theme::theme().text_dim;

    let mut box_lines: Vec<Line<'static>> = Vec::new();

    box_lines.push(Line::from(""));

    let arrow_span = Span::styled(
        format!(" {} ", crate::tui::arrow_glyph()),
        Style::default()
            .fg(prompt_color)
            .add_modifier(Modifier::BOLD),
    );
    let draft_style = Style::default().fg(text_primary);
    let ghost_style = Style::default().fg(text_dim);

    let (cur_line, cur_char) = draft_cursor(text, cursor);
    let mut cur_row = 0usize;
    let mut cur_col = 0usize;

    let line_count = text.split('\n').count();
    for (i, raw_line) in text.split('\n').enumerate() {
        let mut cells: Vec<InputCell> = Vec::new();
        let mut vis_of_char: Vec<usize> = Vec::new();
        if i == 0 && text.is_empty() && chips.is_empty() {
            if let Some(hint) = ghost {
                cells.extend(hint.chars().map(|c| (c, ghost_style)));
            }
            vis_of_char.push(0);
        } else {
            // attachment markers render as chips at their position in the
            // draft — vis_of_char maps raw char indices to cell indices so
            // the caret lands right of a chip anywhere inside or past it
            let markers = crate::repl::attachments::attachment_markers(raw_line);
            let t = crate::theme::theme();
            let pill = Style::default().fg(t.text_bright).bg(t.input_bg);
            let label = Style::default().fg(t.text_muted).bg(t.surface_bg);
            let gap = Style::default().bg(t.surface_bg);
            let mut m = 0usize;
            for (byte_i, ch) in raw_line.char_indices() {
                while m < markers.len() && byte_i >= markers[m].0.end {
                    m += 1;
                }
                if m < markers.len() && byte_i == markers[m].0.start {
                    vis_of_char.push(cells.len());
                    let (range, name) = &markers[m];
                    let kind = marker_kind_label(&raw_line[range.clone()]);
                    cells.extend(chip_cells(&[(kind, name.as_str())], pill, label, gap));
                    continue;
                }
                let in_marker =
                    m < markers.len() && byte_i > markers[m].0.start && byte_i < markers[m].0.end;
                vis_of_char.push(cells.len());
                if !in_marker {
                    cells.push((ch, draft_style));
                }
            }
            vis_of_char.push(cells.len());
        }
        if i + 1 == line_count {
            cells.extend_from_slice(chips);
        }

        let chars: Vec<char> = cells.iter().map(|c| c.0).collect();
        let at_here =
            (i == cur_line).then(|| vis_of_char.get(cur_char).copied().unwrap_or(cells.len()));
        let (rows, at) = wrap_line(&chars, content_w, at_here);
        let first_row = box_lines.len();
        for (r, &(s, e)) in rows.iter().enumerate() {
            let prefix = if i == 0 && r == 0 {
                arrow_span.clone()
            } else {
                Span::raw("   ")
            };
            let mut spans = vec![prefix];
            spans.extend(styled_runs(&cells[s..e]));
            box_lines.push(Line::from(spans));
        }
        if let Some((r, col)) = at {
            cur_row = first_row + r;
            cur_col = col;
        }
    }

    box_lines.push(Line::from(""));

    let mut lead: Vec<Line<'static>> = Vec::new();
    if let Some(home) = home {
        for text in [home.above, home.session].into_iter().flatten() {
            lead.push(Line::from(Span::styled(
                text.to_string(),
                Style::default().fg(text_dim),
            )));
        }
    }
    let lead_h = lead.len();
    for (i, line) in lead.into_iter().enumerate() {
        box_lines.insert(i, line);
    }
    let band_h = box_lines.len() - lead_h;
    if let Some(home) = home {
        box_lines.extend(home_rows(home, w));
    }
    InputBox {
        lines: box_lines,
        band_h,
        lead_h,
        x,
        width: w,

        cur_row: cur_row + lead_h,
        cur_col,
    }
}

#[derive(Default, Clone, Copy)]
pub(crate) struct HomeRows<'a> {
    pub(crate) notice: Option<&'a str>,

    pub(crate) stats: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) model_tail: Option<&'a str>,
    pub(crate) quality_of_life: Option<&'a str>,

    pub(crate) above: Option<&'a str>,

    pub(crate) session: Option<&'a str>,
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum()
}

fn right_aligned_line(w: usize, spans: Vec<Span<'static>>) -> Line<'static> {
    let pad = w.saturating_sub(spans_width(&spans));
    let mut out = Vec::with_capacity(spans.len() + 1);
    if pad > 0 {
        out.push(Span::raw(" ".repeat(pad)));
    }
    out.extend(spans);
    Line::from(out)
}

fn left_right_rows(
    w: usize,
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
) -> Vec<Line<'static>> {
    if right.is_empty() {
        return vec![Line::from(left)];
    }
    let lw = spans_width(&left);
    let rw = spans_width(&right);
    if lw + 2 + rw <= w {
        let mut spans = left;
        spans.push(Span::raw(" ".repeat(w - lw - rw)));
        spans.extend(right);
        vec![Line::from(spans)]
    } else {
        vec![Line::from(left), right_aligned_line(w, right)]
    }
}

fn home_rows(home: HomeRows, w: usize) -> Vec<Line<'static>> {
    let mut rows: Vec<Line<'static>> = Vec::new();
    let t = crate::theme::theme();
    let mut model: Vec<Span<'static>> = Vec::new();
    if let Some(m) = home.model {
        model.push(Span::styled(
            m.to_string(),
            Style::default().fg(t.text_bright),
        ));
    }
    if let Some(tail) = home.model_tail {
        model.push(Span::styled(
            tail.to_string(),
            Style::default().fg(t.text_muted),
        ));
    }
    let stats = home
        .stats
        .map(crate::composer::stats_spans)
        .unwrap_or_default();
    if !model.is_empty() || !stats.is_empty() {
        rows.extend(left_right_rows(w, model, stats));
    }
    if home.notice.is_some() && !rows.is_empty() {
        rows.push(Line::default());
    }
    if let Some(notice) = home.notice {
        let mut parts = notice.split(" · ");
        let headline = vec![Span::styled(
            parts.next().unwrap_or_default().to_string(),
            Style::default().fg(t.text_bright),
        )];
        rows.push(Line::from(headline));
        let cmds: Vec<(&str, &str)> = parts
            .map(|p| p.split_once(" to ").unwrap_or((p, "")))
            .collect();
        let key_w = cmds
            .iter()
            .map(|(k, _)| display_width(k))
            .max()
            .unwrap_or(0);
        for (key, action) in cmds {
            rows.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{key:<key_w$}   "),
                    Style::default().fg(t.text_body),
                ),
                Span::styled(action.to_string(), Style::default().fg(t.text_muted)),
            ]));
        }
    }
    if let Some(quality_of_life) = home.quality_of_life {
        if rows.is_empty() {
            rows.push(Line::default());
        }
        rows.push(Line::from(Span::styled(
            quality_of_life.to_string(),
            Style::default().fg(t.text_muted),
        )));
    }
    rows
}

pub(crate) fn home_band_h(w: usize) -> usize {
    build_input_box(
        "",
        0,
        w,
        None,
        Some(HomeRows {
            model: Some(" "),
            ..HomeRows::default()
        }),
        &[],
    )
    .lines
    .len()
        + 2
}

pub(crate) fn transcript_ends_blank(transcript: &[Line<'static>]) -> bool {
    transcript.last().is_some_and(|l| {
        l.style.bg.is_none()
            && l.spans
                .iter()
                .all(|s| s.style.bg.is_none() && s.content.trim().is_empty())
    })
}

pub(crate) fn status_dock_h(has_status: bool, needs_seam: bool) -> u16 {
    u16::from(needs_seam) + if has_status { 2 } else { 0 }
}

pub(crate) fn needs_seam(transcript: &[Line<'static>]) -> bool {
    !transcript.is_empty() && !transcript_ends_blank(transcript)
}

pub(crate) fn tail_gap_h(has_status: bool, tail: &[Line<'static>]) -> u16 {
    u16::from(has_status && !tail.is_empty() && !transcript_ends_blank(tail))
}

pub(crate) fn tail_block_h(kept: u16, gap: u16) -> u16 {
    if kept > gap { kept } else { 0 }
}

pub(crate) fn queued_preview_lines(
    queued: &std::collections::VecDeque<(String, Vec<std::path::PathBuf>)>,
    sends: &[(String, Vec<std::path::PathBuf>)],
    w: usize,
) -> Vec<Line<'static>> {
    let total = queued.len() + sends.len();
    if total == 0 {
        return Vec::new();
    }
    let dim = Style::default().fg(crate::theme::theme().text_muted);
    let dim_italic = Style::default()
        .fg(crate::theme::theme().text_muted)
        .add_modifier(Modifier::DIM)
        .add_modifier(Modifier::ITALIC);
    let mut lines = vec![Line::from(vec![
        Span::styled("• ", dim),
        Span::styled(format!("Queued follow-up inputs ({total})"), dim),
    ])];

    let max_show = 3usize;
    for (text, attached) in queued.iter().chain(sends.iter()).take(max_show) {
        let mut it = text.lines();
        let first = it.next().unwrap_or("").trim();
        // Sends store "↳ from <sender>\n<body>" — preview sender + first
        // body line, not the bare header.
        let body = if let Some(from) = first.strip_prefix(crate::session_send::SEND_FROM_PREFIX) {
            let rest = it.find(|l| !l.trim().is_empty()).unwrap_or("").trim();
            format!("↳ {}: {rest}", from.trim())
        } else {
            first.to_string()
        };
        let mut preview: String = body.chars().take(w.saturating_sub(6).max(8)).collect();
        if body.chars().count() > preview.chars().count() {
            preview.push('…');
        }
        if !attached.is_empty() {
            let n = attached.len();
            preview.push_str(&format!(" +{n} image{}", if n == 1 { "" } else { "s" }));
        }
        lines.push(Line::from(vec![
            Span::styled("  ↳ ", dim),
            Span::styled(preview, dim_italic),
        ]));
    }
    let overflow = total.saturating_sub(max_show);
    if overflow > 0 {
        lines.push(Line::from(vec![Span::styled(
            format!("    … +{overflow} more"),
            dim_italic,
        )]));
    }

    lines.push(Line::default());
    lines
}

#[path = "widgets_tests.rs"]
#[cfg(test)]
mod tests;
