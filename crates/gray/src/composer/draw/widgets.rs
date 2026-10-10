//! Viewport widgets: input box, shimmer, status dock (split from `draw`).

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
    // Theme-driven sweep: base role → bright role. Non-Rgb theme colors
    // (e.g. the `terminal` theme's Reset passthrough) fall back to the
    // generic blender, which holds the base color steady.
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

/// Input box render state: styled lines (own internal top/bottom margin
/// rows, `❯` prompt) plus the cursor position within them. `cur_row` is an
/// index into `lines`, so it carries the top pad's offset.
pub(crate) struct InputBox {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) cur_row: usize,
    pub(crate) cur_col: usize,
}

/// Builds the prompt box lines from textarea state. Hoisted out of the draw
/// closure so the dock height (and thus the viewport) can be sized before
/// `terminal.draw` runs.
pub(crate) fn build_input_box(
    text: &str,
    cursor: usize,
    w: usize,
    ghost: Option<&str>,
) -> InputBox {
    let content_w = w.saturating_sub(4).max(1);

    // Neutral Gray palette (no blue, transparent bg — text only)
    let prompt_color = crate::theme::theme().text_soft;
    let text_primary = crate::theme::theme().text_body;
    let text_dim = crate::theme::theme().text_dim;

    let mut box_lines: Vec<Line<'static>> = Vec::new();

    // Top padding inside the box: the blank row the box owns above its
    // `❯` row, so the prompt never sits flush against the row above it.
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
        // Ghost hint (display-only, never submitted): e.g. "Please continue…"
        // while a resume is pending. Same single row either way.
        let mut spans = vec![arrow_span.clone()];
        if let Some(hint) = ghost {
            spans.push(Span::styled(
                hint.to_string(),
                Style::default().fg(text_dim),
            ));
        }
        box_lines.push(Line::from(spans));
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
                    let end = crate::text_width::word_window_end(&chars, start, content_w);
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
        // Content rows were counted from the `❯` row; the top pad shifts
        // every one of them down a row.
        cur_row: cur_row + 1,
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

/// Height the band reserves above its queued, live and input rows:
///   seam    1 row, when the transcript ends in content (see [`needs_seam`])
///   status  1 row, the shimmer text, while a turn runs
///   breath  1 row below the status, so it never melts into the input box
pub(crate) fn status_dock_h(has_status: bool, needs_seam: bool) -> u16 {
    u16::from(needs_seam) + if has_status { 2 } else { 0 }
}

/// Whether the band opens with the seam row: the one gap between the
/// transcript and whatever the band shows first, the status mid-turn or the
/// input box when idle. Gaps are paid as the prefix of the next block, never
/// left as a trailing blank (`transcript::margins`), so the transcript ends
/// in content and the seam is that gap; the only blank tail is the welcome
/// banner's own trailing row. Pure for testability (`Tui::new` needs a TTY).
pub(crate) fn needs_seam(transcript: &[Line<'static>]) -> bool {
    !transcript.is_empty() && !transcript_ends_blank(transcript)
}

/// Queued follow-up inputs held while a turn is in flight (codex
/// `PendingInputPreview` parity, minimal): header + `↳` dim-italic rows,
/// then one blank row so the queue never glues onto the live tool card.
/// One row per queued message, first line only, truncated to `w`.
pub(crate) fn queued_preview_lines(
    queued: &std::collections::VecDeque<(String, Vec<std::path::PathBuf>)>,
    w: usize,
) -> Vec<Line<'static>> {
    if queued.is_empty() {
        return Vec::new();
    }
    let dim = Style::default().fg(crate::theme::theme().text_muted);
    let dim_italic = Style::default()
        .fg(crate::theme::theme().text_muted)
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
    // The separator belongs to the queued block's measured height: a command
    // preview stays visually distinct from the running tool below it.
    lines.push(Line::default());
    lines
}

#[path = "widgets_tests.rs"]
#[cfg(test)]
mod tests;
