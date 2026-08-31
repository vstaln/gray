use super::context::{format_context_length, friendly_model_name, model_context_info};

/// Snapshot of background UI to render dimmed underneath popups.
#[derive(Debug, Clone, Default)]
pub struct BackgroundSnapshot {
    pub transcript: Vec<ratatui::text::Line<'static>>,
    pub cwd: String,
    pub model_name: String,
    pub thinking_effort: String,
    pub prompt_text: String,
    pub used_tokens: usize,
    pub cache_hit_rate: f64,
}

impl BackgroundSnapshot {
    pub fn default_initial() -> Self {
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};

        let (cols, _) = crossterm::terminal::size().unwrap_or((80, 24));
        let w = cols as usize;
        let logo_raw = crate::tui::logo_lines();
        let l_rows = logo_raw.len().max(1) as f32;
        let max_logo_w = logo_raw.iter().map(|l| l.trim_end().chars().count()).max().unwrap_or(0);
        let l_cols = (max_logo_w as f32).max(1.0);
        let logo_pad = w.saturating_sub(max_logo_w) / 2;

        let base = Color::Rgb(110, 110, 110);
        let hilite = Color::Rgb(240, 240, 240);

        let mut welcome_lines: Vec<Line<'static>> = Vec::new();
        welcome_lines.push(Line::from(""));
        for (row, line) in logo_raw.iter().enumerate() {
            let trimmed = line.trim_end();
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.push(Span::raw(" ".repeat(logo_pad)));
            for (col, ch) in trimmed.chars().enumerate() {
                let diag = (col as f32 + (l_rows - 1.0 - row as f32)) / (l_cols + l_rows);
                let t = (0.15 + 0.85 * diag).clamp(0.0, 1.0);
                let color = crate::tui::blend_color(base, hilite, t);
                spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
            }
            welcome_lines.push(Line::from(spans));
        }
        welcome_lines.push(Line::from(""));
        let banner_raw = format!("gray {} \u{b7} Run /help for commands", env!("CARGO_PKG_VERSION"));
        let banner_len = banner_raw.chars().count();
        let pad = w.saturating_sub(banner_len) / 2;
        welcome_lines.push(Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled("gray", Style::default().add_modifier(Modifier::BOLD).fg(Color::Rgb(225, 225, 225))),
            Span::styled(format!(" {} \u{b7} Run /help for commands", env!("CARGO_PKG_VERSION")), Style::default().fg(Color::Rgb(140, 140, 140))),
        ]));
        welcome_lines.push(Line::from(""));

        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        Self {
            transcript: welcome_lines,
            cwd,
            model_name: String::new(),
            thinking_effort: "high".to_string(),
            prompt_text: String::new(),
            used_tokens: 0,
            cache_hit_rate: 0.0,
        }
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

pub fn dim_style(style: ratatui::style::Style) -> ratatui::style::Style {
    use ratatui::style::{Color, Modifier, Style};
    let mut s = Style::default().add_modifier(Modifier::DIM);
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
    new_line.style = dim_style(line.style);
    new_line
}

pub fn render_dimmed_background(frame: &mut ratatui::Frame, bg: &BackgroundSnapshot) {
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;

    let area = frame.area();
    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    let box_bg = Color::Rgb(10, 10, 10);
    let prompt_arrow_color = Color::Rgb(70, 70, 70);
    let text_dimmed_color = Color::Rgb(85, 85, 85);
    let footer_cwd_color = Color::Rgb(48, 48, 48);
    let footer_model_color = Color::Rgb(58, 58, 58);

    let arrow_span = Span::styled("❯ ", Style::default().fg(prompt_arrow_color).add_modifier(Modifier::DIM).bg(box_bg));
    let prompt_line = if bg.prompt_text.is_empty() {
        Line::from(vec![arrow_span]).style(Style::default().bg(box_bg))
    } else {
        Line::from(vec![
            arrow_span,
            Span::styled(bg.prompt_text.clone(), Style::default().fg(text_dimmed_color).add_modifier(Modifier::DIM).bg(box_bg)),
        ]).style(Style::default().bg(box_bg))
    };

    let bottom_box_lines = vec![
        Line::from("").style(Style::default().bg(box_bg)),
        prompt_line,
        Line::from("").style(Style::default().bg(box_bg)),
    ];

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
        Span::raw("  "),
        Span::styled(ctx_display, Style::default().fg(footer_cwd_color).add_modifier(Modifier::DIM)),
        Span::styled(" · ", Style::default().fg(footer_cwd_color).add_modifier(Modifier::DIM)),
        Span::styled(cache_display, Style::default().fg(footer_model_color).add_modifier(Modifier::DIM)),
        Span::raw(" ".repeat(pad_len)),
        Span::styled(right_text, Style::default().fg(footer_model_color).add_modifier(Modifier::DIM)),
    ]);

    let composer_h = 4usize;
    let transcript_avail_h = h.saturating_sub(composer_h);

    let mut full_screen_lines: Vec<Line<'static>> = Vec::with_capacity(h);

    let transcript = &bg.transcript;
    if transcript.len() <= transcript_avail_h {
        for l in transcript {
            full_screen_lines.push(dim_line(l));
        }
    } else {
        let skip = transcript.len() - transcript_avail_h;
        for l in &transcript[skip..] {
            full_screen_lines.push(dim_line(l));
        }
    }

    for l in bottom_box_lines {
        full_screen_lines.push(l);
    }

    full_screen_lines.push(footer_line);

    full_screen_lines.truncate(h);
    while full_screen_lines.len() < h {
        full_screen_lines.push(Line::from(""));
    }

    frame.render_widget(
        Paragraph::new(full_screen_lines),
        Rect::new(area.x, area.y, area.width, area.height),
    );
}
