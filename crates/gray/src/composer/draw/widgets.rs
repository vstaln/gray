//! Viewport widgets: input box, shimmer, status dock (split from `draw`).

use super::*;

pub(crate) fn thinking_style() -> Style {
    Style::default()
        .fg(Color::Rgb(140, 140, 140))
        .add_modifier(Modifier::ITALIC)
}

pub(crate) fn shimmer_spans(text: &str, elapsed: Duration, truecolor: bool) -> Vec<Span<'static>> {
    use ratatui::style::Color;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let padding = 10usize;
    let period = chars.len() + padding * 2;
    let sweep_seconds = 2.0f32;
    let pos = ((elapsed.as_secs_f32() % sweep_seconds) / sweep_seconds * (period as f32)) as usize;
    let band_half_width = 5.0f32;
    const BASE: (u8, u8, u8) = (150, 148, 144);
    const HIGHLIGHT: (u8, u8, u8) = (255, 255, 255);
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
            let style = if truecolor {
                let k = t.clamp(0.0, 1.0) * 0.9;
                let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k) as u8;
                Style::default()
                    .fg(Color::Rgb(
                        lerp(BASE.0, HIGHLIGHT.0),
                        lerp(BASE.1, HIGHLIGHT.1),
                        lerp(BASE.2, HIGHLIGHT.2),
                    ))
                    .add_modifier(if t > 0.3 {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    })
            } else if t < 0.2 {
                Style::default().add_modifier(Modifier::DIM)
            } else if t < 0.6 {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

/// Input box render state: styled lines (own internal top/bottom margin rows,
/// `❯` prompt) plus the cursor position within them.
pub(crate) struct InputBox {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) cur_row: usize,
    pub(crate) cur_col: usize,
}

/// Builds the prompt box lines from textarea state. Hoisted out of the draw
/// closure so the dock height (and thus the viewport) can be sized before
/// `terminal.draw` runs.
pub(crate) fn build_input_box(text: &str, cursor: usize, w: usize) -> InputBox {
    let content_w = w.saturating_sub(4).max(1);

    // Neutral Gray palette (no blue, transparent bg — text only)
    let prompt_color = Color::Rgb(180, 180, 180);
    let text_primary = Color::Rgb(225, 225, 225);

    let mut box_lines: Vec<Line<'static>> = Vec::new();

    // Top padding inside the box
    box_lines.push(Line::from(""));

    // Prompt input rows
    let prompt_arrow = " ❯ ";
    let arrow_span = Span::styled(
        prompt_arrow,
        Style::default()
            .fg(prompt_color)
            .add_modifier(Modifier::BOLD),
    );

    let mut cur_row = 0usize;
    let mut cur_col = 0usize;

    if text.is_empty() {
        box_lines.push(Line::from(vec![arrow_span.clone()]));
    } else {
        let lines_raw: Vec<&str> = text.split('\n').collect();
        let mut cursor_found = false;
        let mut current_byte_pos = 0usize;
        let mut row_count = 0usize;

        for (i, raw_line) in lines_raw.iter().enumerate() {
            let prefix_span = if i == 0 {
                arrow_span.clone()
            } else {
                Span::raw("   ")
            };

            let line_len_bytes = raw_line.len();
            let line_end_bytes = current_byte_pos + line_len_bytes;
            let has_cursor =
                !cursor_found && (cursor <= line_end_bytes || i == lines_raw.len() - 1);

            if raw_line.is_empty() {
                box_lines.push(Line::from(vec![prefix_span]));
                if has_cursor {
                    cur_row = row_count;
                    cur_col = 0;
                    cursor_found = true;
                }
                row_count += 1;
            } else {
                // Word-aware wrap (question-panel `wrap_plain` parity): break
                // at the last space in the window, hard-cut only a single
                // overlong word — words never split across rows.
                let chars: Vec<char> = raw_line.chars().collect();
                let mut windows: Vec<(usize, usize)> = Vec::new();
                let mut start = 0usize;
                while start < chars.len() {
                    let mut end = (start + content_w).min(chars.len());
                    if end < chars.len()
                        && let Some(sp) = chars[start..end].iter().rposition(|c| *c == ' ')
                        && sp > 0
                    {
                        end = start + sp + 1;
                    }
                    windows.push((start, end));
                    start = end;
                }
                // Byte offset of each char start (plus total at the end).
                let mut byte_at: Vec<usize> = Vec::with_capacity(chars.len() + 1);
                let mut b = 0usize;
                for ch in &chars {
                    byte_at.push(b);
                    b += ch.len_utf8();
                }
                byte_at.push(b);

                let mut line_byte_offset = byte_at[0];
                let last_idx = windows.len().saturating_sub(1);

                for (chunk_idx, (cs, ce)) in windows.iter().enumerate() {
                    let chunk: Vec<char> = chars[*cs..*ce].to_vec();
                    let s: String = chunk.iter().collect();
                    let chunk_byte_len = byte_at[*ce] - byte_at[*cs];

                    if chunk_idx == 0 {
                        box_lines.push(Line::from(vec![
                            prefix_span.clone(),
                            Span::styled(s, Style::default().fg(text_primary)),
                        ]));
                    } else {
                        box_lines.push(Line::from(vec![
                            Span::raw("   "),
                            Span::styled(s, Style::default().fg(text_primary)),
                        ]));
                    }

                    if has_cursor && !cursor_found {
                        let cursor_in_line_bytes = cursor.saturating_sub(current_byte_pos);
                        if cursor_in_line_bytes <= line_byte_offset + chunk_byte_len
                            || chunk_idx == last_idx
                        {
                            cur_row = row_count;
                            let bytes_into_chunk =
                                cursor_in_line_bytes.saturating_sub(line_byte_offset);
                            let mut col = 0usize;
                            let mut b = 0usize;
                            for ch in chunk {
                                if b >= bytes_into_chunk {
                                    break;
                                }
                                b += ch.len_utf8();
                                col += 1;
                            }
                            cur_col = col;
                            cursor_found = true;
                        }
                    }

                    line_byte_offset += chunk_byte_len;
                    row_count += 1;
                }
            }

            current_byte_pos = line_end_bytes + 1;
        }
    }

    // Bottom padding inside the box
    box_lines.push(Line::from(""));

    InputBox {
        lines: box_lines,
        cur_row,
        cur_col,
    }
}

/// True when the last scrollback row is a bare blank (no bg, no glyphs):
/// the transcript already left breathing room above the viewport (a
/// paragraph separator, `ensure_gap`, …). Same predicate `ensure_gap` uses.
pub(crate) fn transcript_ends_blank(transcript: &[Line<'static>]) -> bool {
    transcript.last().is_some_and(|l| {
        l.style.bg.is_none()
            && l.spans
                .iter()
                .all(|s| s.style.bg.is_none() && s.content.trim().is_empty())
    })
}

/// Height reserved above the input box for the live status:///   seam    1 row — ONLY when the transcript's last row is not already blank
///   status  1 row — the plain shimmer text
///   breath  1 row — bare space below, so it never melts into the input box
///
/// The seam is dynamic on purpose. A fixed seam (b377846) stacked with the
/// blank a paragraph checkpoint leaves behind (2-row gap, hence 8ec9af0);
/// no seam (current) jams thinking rows, list items, code fences and
/// partial paragraphs flush against `⬡ Working…`. Deciding per frame gives
/// exactly one blank row above the status — `ensure_gap(1)` for the dock.
pub(crate) fn status_dock_h(has_status: bool, question_active: bool, needs_seam: bool) -> u16 {
    if !has_status || question_active {
        return 0;
    }
    2 + u16::from(needs_seam)
}

/// Queued follow-up inputs held while a turn is in flight (codex
/// `PendingInputPreview` parity, minimal): header + `↳` dim-italic rows.
/// One row per queued message, first line only, truncated to `w`.
pub(crate) fn queued_preview_lines(
    queued: &std::collections::VecDeque<(String, Vec<std::path::PathBuf>)>,
    w: usize,
) -> Vec<Line<'static>> {
    if queued.is_empty() {
        return Vec::new();
    }
    let dim = Style::default().fg(Color::Rgb(140, 140, 140));
    let dim_italic = Style::default()
        .fg(Color::Rgb(140, 140, 140))
        .add_modifier(Modifier::DIM)
        .add_modifier(Modifier::ITALIC);
    let mut lines = vec![Line::from(vec![
        Span::styled("• ", dim),
        Span::styled(format!("Queued follow-up inputs ({})", queued.len()), dim),
    ])];
    // Viewport is only 10 rows — cap preview so input + footer stay visible.
    let max_show = 3usize;
    for (text, attached) in queued.iter().take(max_show) {
        let first = text.lines().next().unwrap_or("").trim();
        let mut preview: String = first.chars().take(w.saturating_sub(6).max(8)).collect();
        if first.chars().count() > preview.chars().count() {
            preview.push('…');
        }
        if !attached.is_empty() {
            preview.push_str(&format!(" [+{} image]", attached.len()));
        }
        lines.push(Line::from(vec![
            Span::styled("  ↳ ", dim),
            Span::styled(preview, dim_italic),
        ]));
    }
    if queued.len() > max_show {
        lines.push(Line::from(vec![Span::styled(
            format!("    … +{} more", queued.len() - max_show),
            dim_italic,
        )]));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_texts(ibox: &InputBox) -> Vec<String> {
        ibox.lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn input_box_wraps_at_word_boundaries() {
        // Narrow box forces a wrap inside "...with colors." — the word must
        // move whole to the next row, never split as "c" / "olors.".
        let text = "aa bb cc dd ee ff with colors.";
        let ibox = build_input_box(text, text.len(), 20);
        let rows = row_texts(&ibox);
        let joined = rows.join("\n");
        assert!(
            joined.contains("colors."),
            "word must survive whole: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.ends_with('c') && r.contains("with ")),
            "must not split mid-word: {rows:?}"
        );
        // Cursor at end must land on the last content row (top/bottom
        // margins excluded from cur_row).
        assert_eq!(ibox.cur_row, rows.len() - 3, "rows: {rows:?}");
    }

    #[test]
    fn input_box_hard_cuts_only_overlong_words() {
        let text = "ok abcdefghijklmnopqrstuvwxyz0129 end";
        let ibox = build_input_box(text, 0, 20);
        let rows = row_texts(&ibox);
        assert!(rows.iter().any(|r| r.contains("ok ")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("end")), "{rows:?}");
    }
}
