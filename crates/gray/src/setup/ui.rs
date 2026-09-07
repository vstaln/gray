use super::context::{format_context_length, friendly_model_name, model_context_info};

/// Snapshot of background UI to render dimmed underneath popups.
#[derive(Debug, Clone, Default)]
pub struct BackgroundSnapshot {
    pub transcript: Vec<ratatui::text::Line<'static>>,
    pub history_entries: Vec<crate::composer::TranscriptEntry>,
    pub cwd: String,
    pub model_name: String,
    pub thinking_effort: String,
    pub prompt_text: String,
    pub used_tokens: usize,
    pub cache_hit_rate: f64,
}

impl BackgroundSnapshot {
    pub fn default_initial() -> Self {
        let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
        let welcome_lines = crate::composer::build_welcome_lines(cols as usize);
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        Self {
            transcript: welcome_lines,
            history_entries: vec![crate::composer::TranscriptEntry::Welcome],
            cwd,
            model_name: String::new(),
            thinking_effort: "high".to_string(),
            prompt_text: String::new(),
            used_tokens: 0,
            cache_hit_rate: 0.0,
        }
    }

    pub fn rebuild_transcript(&self, w: usize) -> Vec<ratatui::text::Line<'static>> {
        if self.history_entries.is_empty() {
            return self.transcript.clone();
        }
        let mut lines = Vec::new();
        for entry in &self.history_entries {
            match entry {
                crate::composer::TranscriptEntry::Welcome => {
                    lines.extend(crate::composer::build_welcome_lines(w));
                }
                crate::composer::TranscriptEntry::UserPrompt(text, attached) => {
                    lines.extend(crate::composer::transcript::format_user_prompt_lines(
                        text, attached, w,
                    ));
                }
                crate::composer::TranscriptEntry::ToolBox { header, body } => {
                    lines.extend(crate::composer::transcript::format_tool_box_lines(
                        header.clone(),
                        body,
                        w,
                    ));
                }
                crate::composer::TranscriptEntry::StyledLines {
                    lines: styled,
                    hyperlinks: _,
                } => {
                    lines.extend(styled.clone());
                }
                crate::composer::TranscriptEntry::Gap(n) => {
                    for _ in 0..*n {
                        lines.push(ratatui::text::Line::from(""));
                    }
                }
            }
        }
        lines
    }
}

pub fn dim_color(c: ratatui::style::Color) -> ratatui::style::Color {
    use ratatui::style::Color;
    match c {
        Color::Rgb(r, g, b) => {
            let k = 0.38f32;
            let r2 = ((r as f32) * k).round() as u8;
            let g2 = ((g as f32) * k).round() as u8;
            let b2 = ((b as f32) * k).round() as u8;
            Color::Rgb(r2, g2, b2)
        }
        Color::White => Color::Rgb(85, 85, 85),
        Color::Gray => Color::Rgb(60, 60, 60),
        Color::DarkGray => Color::Rgb(40, 40, 40),
        Color::Black => Color::Rgb(8, 8, 8),
        Color::Green => Color::Rgb(30, 90, 50),
        Color::Yellow => Color::Rgb(95, 80, 30),
        Color::Blue => Color::Rgb(35, 50, 90),
        Color::Magenta => Color::Rgb(70, 35, 70),
        Color::Cyan => Color::Rgb(35, 70, 70),
        Color::Red => Color::Rgb(90, 35, 35),
        Color::Reset => Color::Rgb(60, 60, 60),
        other => other,
    }
}

/// Backdrop for alternate-screen modals: the terminal default (`Reset`), so the
/// dimmed background matches the inline composer (transparent transcript over
/// the terminal bg) instead of painting a mismatched pure-black screen. Every
/// dimmed style still carries an explicit bg so no unpainted cells remain; the
/// initial `Clear` plus full-height row-by-row rendering keeps stale
/// alt-screen cells from bleeding through behind the popup.
pub(crate) const BACKDROP_BG: ratatui::style::Color = ratatui::style::Color::Reset;

pub fn dim_style(style: ratatui::style::Style) -> ratatui::style::Style {
    use ratatui::style::{Color, Modifier, Style};
    let mut s = Style::default().add_modifier(Modifier::DIM);
    if let Some(fg) = style.fg {
        s = s.fg(dim_color(fg));
    } else {
        s = s.fg(Color::Rgb(70, 70, 70));
    }
    if let Some(bg) = style.bg {
        // Composer gray: user prompt cards and the input box share Rgb(22, 22, 22).
        // Preserving this background ensures user message cards retain their
        // visible card box ("overlay") behind modals instead of crushing to near-black.
        if bg == Color::Rgb(22, 22, 22) {
            s = s.bg(bg);
        } else {
            s = s.bg(dim_color(bg));
        }
    }
    s
}

pub fn dim_line(line: &ratatui::text::Line<'_>) -> ratatui::text::Line<'static> {
    use ratatui::text::{Line, Span};
    let spans: Vec<Span<'static>> = line
        .spans
        .iter()
        .map(|span| Span::styled(span.content.to_string(), dim_style(span.style)))
        .collect();
    let mut new_line = Line::from(spans);
    let mut st = dim_style(line.style);
    if st.bg.is_none() {
        st = st.bg(BACKDROP_BG);
    }
    new_line.style = st;
    new_line
}

/// Pads a backdrop line to the full width with opaque spaces so no
/// transparent cells remain (composer `draw.rs` popup-row parity).
/// Padding keeps the row's own bg (prompt box stays composer gray, transcript black).
fn pad_backdrop_line(
    mut line: ratatui::text::Line<'static>,
    w: usize,
) -> ratatui::text::Line<'static> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    let bg = line.style.bg.unwrap_or(BACKDROP_BG);
    let used = line.width();
    if used < w {
        line.spans
            .push(Span::styled(" ".repeat(w - used), Style::default().bg(bg)));
    }
    line.style = line.style.bg(bg);
    line
}

pub fn render_dimmed_background(frame: &mut ratatui::Frame, bg: &BackgroundSnapshot) {
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Clear, Paragraph};

    let area = frame.area();
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }
    // Opaque base: without this, backdrop rows with transparent cells leave
    // stale alt-screen content / terminal transparency visible behind modals.
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKDROP_BG)),
        area,
    );

    // Composer gray: the live input box and every modal use (22,22,22).
    // The backdrop's copy of the INPUT BOX is chrome, not content: paint it
    // dimmed so the textarea visibly drops behind modals. Transcript user
    // cards keep full gray via dim_style's preservation branch below.
    let box_bg = Color::Rgb(22, 22, 22);
    let input_bg = dim_color(box_bg);
    let prompt_arrow_color = Color::Rgb(70, 70, 70);
    let text_dimmed_color = Color::Rgb(85, 85, 85);
    let footer_cwd_color = Color::Rgb(48, 48, 48);
    let footer_model_color = Color::Rgb(58, 58, 58);

    let arrow_span = Span::styled(
        " ❯ ",
        Style::default()
            .fg(prompt_arrow_color)
            .add_modifier(Modifier::DIM)
            .bg(input_bg),
    );
    let cont_span = Span::styled("   ", Style::default().bg(input_bg));
    // Mirror the composer input box: wrap the live prompt so a long /
    // multi-line draft grows the box instead of breaking a single row.
    let content_w = w.saturating_sub(4).max(1);
    let mut prompt_rows: Vec<Line<'static>> = Vec::new();
    if bg.prompt_text.is_empty() {
        prompt_rows.push(Line::from(vec![arrow_span]).style(Style::default().bg(input_bg)));
    } else {
        for (li, logical) in bg.prompt_text.split('\n').enumerate() {
            let prefix = if li == 0 {
                arrow_span.clone()
            } else {
                cont_span.clone()
            };
            if logical.is_empty() {
                prompt_rows.push(Line::from(vec![prefix]).style(Style::default().bg(input_bg)));
                continue;
            }
            let chars: Vec<char> = logical.chars().collect();
            for (ci, chunk) in chars.chunks(content_w).enumerate() {
                let s: String = chunk.iter().collect();
                let p = if li == 0 && ci == 0 {
                    arrow_span.clone()
                } else {
                    cont_span.clone()
                };
                prompt_rows.push(
                    Line::from(vec![
                        p,
                        Span::styled(
                            s,
                            Style::default()
                                .fg(text_dimmed_color)
                                .add_modifier(Modifier::DIM)
                                .bg(input_bg),
                        ),
                    ])
                    .style(Style::default().bg(input_bg)),
                );
            }
        }
    }

    let mut bottom_box_lines = vec![Line::from("").style(Style::default().bg(input_bg))];
    bottom_box_lines.extend(prompt_rows);
    bottom_box_lines.push(Line::from("").style(Style::default().bg(input_bg)));

    let (_, max_label) = model_context_info(&bg.model_name);
    let ctx_display = format!("{}/{}", format_context_length(bg.used_tokens), max_label);
    let cache_display = format!("{:.1}% cache", bg.cache_hit_rate * 100.0);

    let model_display = friendly_model_name(&bg.model_name);
    // Provider-driven (opencode parity): no effort badge when the provider
    // says this model doesn't reason. Unknown → show, as before.
    let show_effort = super::context::model_supports_reasoning(&bg.model_name) != Some(false);
    let right_text = if model_display.is_empty() {
        if show_effort {
            let effort_display = if bg.thinking_effort.is_empty() {
                "high"
            } else {
                &bg.thinking_effort
            };
            effort_display.to_string()
        } else {
            String::new()
        }
    } else if show_effort {
        let effort_display = if bg.thinking_effort.is_empty() {
            "high"
        } else {
            &bg.thinking_effort
        };
        format!("{model_display} · {effort_display}")
    } else {
        model_display
    };
    let left_len = 2 + ctx_display.chars().count() + 3 + cache_display.chars().count();
    let pad_len = w.saturating_sub(left_len + right_text.chars().count());

    let footer_line = Line::from(vec![
        Span::styled("  ", Style::default().bg(BACKDROP_BG)),
        Span::styled(
            ctx_display,
            Style::default()
                .fg(footer_cwd_color)
                .add_modifier(Modifier::DIM)
                .bg(BACKDROP_BG),
        ),
        Span::styled(
            " · ",
            Style::default()
                .fg(footer_cwd_color)
                .add_modifier(Modifier::DIM)
                .bg(BACKDROP_BG),
        ),
        Span::styled(
            cache_display,
            Style::default()
                .fg(footer_model_color)
                .add_modifier(Modifier::DIM)
                .bg(BACKDROP_BG),
        ),
        Span::styled(" ".repeat(pad_len), Style::default().bg(BACKDROP_BG)),
        Span::styled(
            right_text,
            Style::default()
                .fg(footer_model_color)
                .add_modifier(Modifier::DIM)
                .bg(BACKDROP_BG),
        ),
    ])
    .style(Style::default().bg(BACKDROP_BG));

    let transcript = bg.rebuild_transcript(w);

    // Live TUI parity: ensure breathing room (a gap row) between transcript and
    // composer input box whenever transcript does not already end blank.
    let transcript_ends_blank = transcript.last().is_some_and(|l| {
        (l.style.bg.is_none() || l.style.bg == Some(BACKDROP_BG))
            && l.spans.iter().all(|s| {
                (s.style.bg.is_none() || s.style.bg == Some(BACKDROP_BG))
                    && s.content.trim().is_empty()
            })
    });
    let needs_gap = !transcript.is_empty() && !transcript_ends_blank;
    let gap_h: usize = if needs_gap { 1 } else { 0 };

    let composer_h = bottom_box_lines.len() + 1 + gap_h;
    let transcript_avail_h = h.saturating_sub(composer_h);

    let tail: &[Line<'static>] = if transcript.len() <= transcript_avail_h {
        &transcript
    } else {
        &transcript[transcript.len() - transcript_avail_h..]
    };

    let mut full_screen_lines: Vec<Line<'static>> = Vec::with_capacity(h);

    // Live TUI: the transcript starts at the top of the terminal,
    // followed by the gap, composer input box, and footer, with empty filler
    // at the bottom of the screen. Anchoring filler at the top (top_pad)
    // caused the whole UI to jump down to the bottom of the screen.
    for l in tail {
        full_screen_lines.push(pad_backdrop_line(dim_line(l), w));
    }
    if needs_gap {
        full_screen_lines.push(pad_backdrop_line(Line::from(""), w));
    }
    for l in bottom_box_lines {
        full_screen_lines.push(pad_backdrop_line(l, w));
    }
    full_screen_lines.push(pad_backdrop_line(footer_line, w));

    while full_screen_lines.len() < h {
        full_screen_lines.push(pad_backdrop_line(Line::from(""), w));
    }
    full_screen_lines.truncate(h);

    // Row-by-row (composer `draw.rs` parity): a single multi-line Paragraph
    // would wrap long transcript lines and shift the whole backdrop.
    for (i, line) in full_screen_lines.into_iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        frame.render_widget(Paragraph::new(line), Rect::new(area.x, y, area.width, 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer_rows(backend: &ratatui::backend::TestBackend, w: u16, h: u16) -> Vec<String> {
        let buf = backend.buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    #[test]
    fn backdrop_mirrors_live_layout_with_multiline_prompt() {
        // Live TUI: the composer input box and footer start below any transcript,
        // followed by empty filler space at the bottom of the screen.
        let backend = ratatui::backend::TestBackend::new(40, 10);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        let bg = BackgroundSnapshot {
            transcript: Vec::new(),
            history_entries: Vec::new(),
            prompt_text: "abcdefghijklmnopqrstuvwxyz0123456789!@#$ ".repeat(3),
            ..Default::default()
        };
        terminal
            .draw(|frame| render_dimmed_background(frame, &bg))
            .expect("draw");
        let rows = buffer_rows(terminal.backend(), 40, 10);
        // 4 wrapped prompt rows + top/bottom blank = 6 box rows, 1 footer row = 7 rows total.
        // In a 10-row viewport with 0 transcript rows, the box starts at row 0,
        // footer is at row 6, and rows 7..10 are trailing filler.
        assert!(rows[1].contains("❯"), "prompt box starts at top: {rows:?}");
        assert!(
            rows[6].contains("cache"),
            "footer follows the box: {rows:?}"
        );
        assert!(
            rows[7..].iter().all(|r| r.trim().is_empty()),
            "trailing filler after footer: {rows:?}"
        );
        let box_bg = terminal.backend().buffer()[(0, 1)].bg;
        assert_eq!(
            box_bg,
            // NOTE: updated with the input-box dim fix; UNRUN (cargo test
            // banned under X) — verify in TTY/CI. dim_color((22,22,22)).
            ratatui::style::Color::Rgb(8, 8, 8),
            "backdrop input box is dimmed composer gray"
        );
    }

    #[test]
    fn backdrop_preserves_card_box_and_inserts_gap_before_input() {
        let backend = ratatui::backend::TestBackend::new(40, 15);
        let mut terminal = ratatui::Terminal::new(backend).expect("test terminal");
        let bg = BackgroundSnapshot {
            history_entries: vec![crate::composer::TranscriptEntry::UserPrompt(
                "/thinking".to_string(),
                Vec::new(),
            )],
            ..Default::default()
        };
        terminal
            .draw(|frame| render_dimmed_background(frame, &bg))
            .expect("draw");
        let rows = buffer_rows(terminal.backend(), 40, 15);
        // Prompt card: 3 rows (margin, ' ❯ /thinking', margin)
        assert!(
            rows[1].contains("/thinking"),
            "card contains command: {rows:?}"
        );
        // Card background is preserved (not crushed to near-black)
        let card_bg = terminal.backend().buffer()[(0, 1)].bg;
        assert_eq!(
            card_bg,
            ratatui::style::Color::Rgb(22, 22, 22),
            "card matches composer gray overlay"
        );
        // Row 3 is the gap row between card and input box
        assert!(
            rows[3].trim().is_empty(),
            "gap row between sent text and input box: {rows:?}"
        );
        // Row 4 is top margin of input box, row 5 is input prompt arrow
        assert!(
            rows[5].contains("❯"),
            "input box arrow follows gap row: {rows:?}"
        );
    }
}
