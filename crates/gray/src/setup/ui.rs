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
                    lines.extend(crate::composer::transcript::format_user_prompt_lines(text, attached, w));
                }
                crate::composer::TranscriptEntry::ToolBox { header, body } => {
                    if crate::composer::transcript::is_gateway_boot_header(header) {
                        lines.extend(crate::composer::transcript::format_gateway_boot_card(header.clone(), body, w));
                    } else {
                        lines.extend(crate::composer::transcript::format_tool_box_lines(header.clone(), body, w));
                    }
                }
                crate::composer::TranscriptEntry::StyledLines { lines: styled, hyperlinks: _ } => {
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

/// Opaque backdrop for alternate-screen modals. Every dimmed style carries
/// an explicit bg so terminal transparency / stale alt-screen cells can't
/// bleed through the backdrop ("broken thing behind the popup").
pub(crate) const BACKDROP_BG: ratatui::style::Color = ratatui::style::Color::Rgb(0, 0, 0);

pub fn dim_style(style: ratatui::style::Style) -> ratatui::style::Style {
    use ratatui::style::{Color, Modifier, Style};
    let mut s = Style::default().add_modifier(Modifier::DIM).bg(BACKDROP_BG);
    if let Some(fg) = style.fg {
        s = s.fg(dim_color(fg));
    } else {
        s = s.fg(Color::Rgb(70, 70, 70));
    }
    if let Some(bg) = style.bg {
        s = s.bg(dim_color(bg));
    }
    s
}

pub fn dim_line(line: &ratatui::text::Line<'_>) -> ratatui::text::Line<'static> {
    use ratatui::text::{Line, Span};
    let spans: Vec<Span<'static>> = line.spans.iter().map(|span| {
        Span::styled(span.content.to_string(), dim_style(span.style))
    }).collect();
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
fn pad_backdrop_line(mut line: ratatui::text::Line<'static>, w: usize) -> ratatui::text::Line<'static> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    let bg = line.style.bg.unwrap_or(BACKDROP_BG);
    let used = line.width();
    if used < w {
        line.spans.push(Span::styled(
            " ".repeat(w - used),
            Style::default().bg(bg),
        ));
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

    // Composer gray: the live input box and every modal use (22,22,22);
    // near-black here read as "no overlay" behind modals.
    let box_bg = Color::Rgb(22, 22, 22);
    let prompt_arrow_color = Color::Rgb(70, 70, 70);
    let text_dimmed_color = Color::Rgb(85, 85, 85);
    let footer_cwd_color = Color::Rgb(48, 48, 48);
    let footer_model_color = Color::Rgb(58, 58, 58);

    let arrow_span = Span::styled("❯ ", Style::default().fg(prompt_arrow_color).add_modifier(Modifier::DIM).bg(box_bg));
    let cont_span = Span::styled("  ", Style::default().bg(box_bg));
    // Mirror the composer input box: wrap the live prompt so a long /
    // multi-line draft grows the box instead of breaking a single row.
    let content_w = w.saturating_sub(4).max(1);
    let mut prompt_rows: Vec<Line<'static>> = Vec::new();
    if bg.prompt_text.is_empty() {
        prompt_rows.push(Line::from(vec![arrow_span]).style(Style::default().bg(box_bg)));
    } else {
        for (li, logical) in bg.prompt_text.split('\n').enumerate() {
            let prefix = if li == 0 { arrow_span.clone() } else { cont_span.clone() };
            if logical.is_empty() {
                prompt_rows.push(Line::from(vec![prefix]).style(Style::default().bg(box_bg)));
                continue;
            }
            let chars: Vec<char> = logical.chars().collect();
            for (ci, chunk) in chars.chunks(content_w).enumerate() {
                let s: String = chunk.iter().collect();
                let p = if li == 0 && ci == 0 { arrow_span.clone() } else { cont_span.clone() };
                prompt_rows.push(Line::from(vec![
                    p,
                    Span::styled(s, Style::default().fg(text_dimmed_color).add_modifier(Modifier::DIM).bg(box_bg)),
                ]).style(Style::default().bg(box_bg)));
            }
        }
    }

    let mut bottom_box_lines = vec![
        Line::from("").style(Style::default().bg(box_bg)),
    ];
    bottom_box_lines.extend(prompt_rows);
    bottom_box_lines.push(Line::from("").style(Style::default().bg(box_bg)));

    let (_, max_label) = model_context_info(&bg.model_name);
    let ctx_display = format!("{}/{}", format_context_length(bg.used_tokens), max_label);
    let cache_display = format!("{:.1}% cache", bg.cache_hit_rate * 100.0);

    let model_display = friendly_model_name(&bg.model_name);
    let effort_display = if bg.thinking_effort.is_empty() { "high" } else { &bg.thinking_effort };
    let right_text = if model_display.is_empty() {
        effort_display.to_string()
    } else {
        format!("{model_display} · {effort_display}")
    };
    let left_len = 2 + ctx_display.chars().count() + 3 + cache_display.chars().count();
    let pad_len = w.saturating_sub(left_len + right_text.chars().count());

    let footer_line = Line::from(vec![
        Span::styled("  ", Style::default().bg(BACKDROP_BG)),
        Span::styled(ctx_display, Style::default().fg(footer_cwd_color).add_modifier(Modifier::DIM).bg(BACKDROP_BG)),
        Span::styled(" · ", Style::default().fg(footer_cwd_color).add_modifier(Modifier::DIM).bg(BACKDROP_BG)),
        Span::styled(cache_display, Style::default().fg(footer_model_color).add_modifier(Modifier::DIM).bg(BACKDROP_BG)),
        Span::styled(" ".repeat(pad_len), Style::default().bg(BACKDROP_BG)),
        Span::styled(right_text, Style::default().fg(footer_model_color).add_modifier(Modifier::DIM).bg(BACKDROP_BG)),
    ])
    .style(Style::default().bg(BACKDROP_BG));

    // Dynamic: the box grows with wrapped prompt rows (fixed 4 was the
    // old blank+prompt+blank+footer); reserving fewer rows than pushed made
    // truncate() eat the footer + box bottom behind modals.
    let composer_h = bottom_box_lines.len() + 1;
    let transcript_avail_h = h.saturating_sub(composer_h);

    let mut full_screen_lines: Vec<Line<'static>> = Vec::with_capacity(h);

    // Live-TUI order: transcript, input box right below it, footer, then
    // empty filler. Pinning the box to the bottom edge (filler in the
    // middle) moved the text area away from where the live TUI keeps it.
    let transcript = bg.rebuild_transcript(w);
    let tail: &[Line<'static>] = if transcript.len() <= transcript_avail_h {
        &transcript
    } else {
        &transcript[transcript.len() - transcript_avail_h..]
    };
    for l in tail {
        full_screen_lines.push(pad_backdrop_line(dim_line(l), w));
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
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(area.x, y, area.width, 1),
        );
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
        // Live TUI: transcript, input box right below it, footer, then empty
        // space. The backdrop used to pin the box to the bottom edge (filler
        // between transcript and box, footer truncated for long drafts) and
        // painted the box near-black instead of the composer gray.
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
        // 4 wrapped prompt rows + top/bottom blank = 6 box rows, footer next.
        assert!(rows[6].contains("cache"), "footer follows the box: {rows:?}");
        assert!(rows[7..].iter().all(|r| r.trim().is_empty()), "filler after footer: {rows:?}");
        assert!(rows[1].contains("❯"), "prompt box right after transcript: {rows:?}");
        let box_bg = terminal.backend().buffer()[(0, 1)].bg;
        assert_eq!(box_bg, ratatui::style::Color::Rgb(22, 22, 22), "box matches composer gray");
    }
}
