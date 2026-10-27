use super::context::{format_context_length, friendly_model_name, model_context_info};

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
    pub home: bool,
    pub home_notice: Option<String>,
    pub plugin_stats: Option<String>,
    pub home_update: Option<String>,
    pub home_quality_of_life: Option<String>,
    pub home_session: Option<String>,
    pub attachments: Vec<std::path::PathBuf>,
    /// Memoized `rebuild_transcript` keyed on (cols, rows). Modal loops
    /// repaint this backdrop every frame, but the snapshot is frozen —
    /// shared across clones so a modal's copy stays warm.
    #[allow(clippy::type_complexity)]
    pub transcript_cache: std::rc::Rc<
        std::cell::RefCell<Option<(usize, u16, std::rc::Rc<Vec<ratatui::text::Line<'static>>>)>>,
    >,
    /// Memoized painted backdrop keyed on (area, theme); see
    /// [`render_dimmed_background`].
    pub backdrop_cache: std::rc::Rc<
        std::cell::RefCell<
            Option<(
                ratatui::layout::Rect,
                crate::theme::UiTheme,
                ratatui::buffer::Buffer,
            )>,
        >,
    >,
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
            home: true,
            home_notice: None,
            plugin_stats: None,
            home_update: None,
            home_quality_of_life: None,
            home_session: None,
            attachments: Vec::new(),
            transcript_cache: Default::default(),
            backdrop_cache: Default::default(),
        }
    }

    pub fn rebuild_transcript(&self, w: usize) -> std::rc::Rc<Vec<ratatui::text::Line<'static>>> {
        // Skip the ioctl off-tty (tests, pipes): the fallback is constant and
        // a terminal's ownness can't change for stdout.
        static IS_TTY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let rows = if *IS_TTY.get_or_init(|| std::io::IsTerminal::is_terminal(&std::io::stdout())) {
            crossterm::terminal::size().map(|(_, r)| r).unwrap_or(24)
        } else {
            24
        };
        if let Some((cw, cr, cached)) = &*self.transcript_cache.borrow()
            && *cw == w
            && *cr == rows
        {
            return cached.clone();
        }
        let lines = if self.history_entries.is_empty() {
            self.transcript.clone()
        } else {
            crate::composer::transcript::layout_history_tail(
                &self.history_entries,
                w,
                rows,
                usize::from(rows),
            )
        };
        let rc = std::rc::Rc::new(lines);
        *self.transcript_cache.borrow_mut() = Some((w, rows, rc.clone()));
        rc
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
        Color::DarkGray => crate::theme::theme().text_faint,
        Color::Black => Color::Rgb(8, 8, 8),
        Color::Green => Color::Rgb(30, 90, 50),
        Color::Yellow => Color::Rgb(95, 80, 30),
        Color::Blue => Color::Rgb(35, 50, 90),
        Color::Magenta => Color::Rgb(70, 35, 70),
        Color::Cyan => Color::Rgb(35, 70, 70),
        Color::Red => Color::Rgb(90, 35, 35),
        Color::Reset => crate::theme::theme().text_faint,
        other => other,
    }
}

pub(crate) fn backdrop_bg() -> ratatui::style::Color {
    let w = crate::theme::theme().window_bg;
    if w == ratatui::style::Color::Reset {
        w
    } else {
        dim_color(w)
    }
}

pub fn dim_style(style: ratatui::style::Style) -> ratatui::style::Style {
    use ratatui::style::{Modifier, Style};
    let mut s = Style::default().add_modifier(Modifier::DIM);
    if let Some(fg) = style.fg {
        s = s.fg(dim_color(fg));
    } else {
        s = s.fg(crate::theme::theme().text_faint);
    }
    if let Some(bg) = style.bg {
        s = s.bg(dim_color(bg));
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
        st = st.bg(backdrop_bg());
    }
    new_line.style = st;
    new_line
}

fn pad_backdrop_line(
    mut line: ratatui::text::Line<'static>,
    w: usize,
) -> ratatui::text::Line<'static> {
    use ratatui::style::Style;
    use ratatui::text::Span;
    let bg = line.style.bg.unwrap_or(backdrop_bg());
    let used = line.width();
    if used < w {
        line.spans
            .push(Span::styled(" ".repeat(w - used), Style::default().bg(bg)));
    }
    line.style = line.style.bg(bg);
    line
}

/// Paint the frozen backdrop behind a modal. Modal loops call this every
/// frame, but the snapshot never changes while the modal is open, so the
/// painted cells are memoized per (area, theme) and copied in afterwards.
pub fn render_dimmed_background(frame: &mut ratatui::Frame, bg: &BackgroundSnapshot) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = crate::theme::theme();
    let mut cache = bg.backdrop_cache.borrow_mut();
    if !matches!(&*cache, Some((a, t, _)) if *a == area && *t == theme) {
        let mut buf = ratatui::buffer::Buffer::empty(area);
        paint_dimmed_background(&mut buf, area, bg);
        *cache = Some((area, theme, buf));
    }
    if let Some((_, _, painted)) = &*cache {
        let target = frame.buffer_mut();
        if target.area == painted.area {
            target.content.clone_from_slice(&painted.content);
        } else {
            target.merge(painted);
        }
    }
}

fn paint_dimmed_background(
    buf: &mut ratatui::buffer::Buffer,
    area: ratatui::layout::Rect,
    bg: &BackgroundSnapshot,
) {
    use ratatui::layout::Rect;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Widget;
    use ratatui::widgets::{Block, Clear, Paragraph};

    let w = area.width as usize;
    let h = area.height as usize;
    if w == 0 || h == 0 {
        return;
    }

    Clear.render(area, buf);
    Block::default()
        .style(Style::default().bg(backdrop_bg()))
        .render(area, buf);

    let box_bg = crate::theme::theme().surface_bg;
    let input_bg = dim_color(box_bg);

    let faint_dimmed = dim_color(crate::theme::theme().text_faint);
    let prompt_arrow_color = faint_dimmed;
    let text_dimmed_color = faint_dimmed;
    let footer_cwd_color = faint_dimmed;
    let footer_model_color = faint_dimmed;

    let arrow_span = Span::styled(
        format!(" {} ", crate::tui::arrow_glyph()),
        Style::default()
            .fg(prompt_arrow_color)
            .add_modifier(Modifier::DIM)
            .bg(input_bg),
    );
    let cont_span = Span::styled("   ", Style::default().bg(input_bg));

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
        model_display.clone()
    };
    let left_len = 2 + ctx_display.chars().count() + 3 + cache_display.chars().count();
    let pad_len = w.saturating_sub(left_len + right_text.chars().count());

    let footer_line = Line::from(vec![
        Span::styled("  ", Style::default().bg(backdrop_bg())),
        Span::styled(
            ctx_display,
            Style::default()
                .fg(footer_cwd_color)
                .add_modifier(Modifier::DIM)
                .bg(backdrop_bg()),
        ),
        Span::styled(
            " · ",
            Style::default()
                .fg(footer_cwd_color)
                .add_modifier(Modifier::DIM)
                .bg(backdrop_bg()),
        ),
        Span::styled(
            cache_display,
            Style::default()
                .fg(footer_model_color)
                .add_modifier(Modifier::DIM)
                .bg(backdrop_bg()),
        ),
        Span::styled(" ".repeat(pad_len), Style::default().bg(backdrop_bg())),
        Span::styled(
            right_text,
            Style::default()
                .fg(footer_model_color)
                .add_modifier(Modifier::DIM)
                .bg(backdrop_bg()),
        ),
    ])
    .style(Style::default().bg(backdrop_bg()));

    let transcript = bg.rebuild_transcript(w);

    if bg.home {
        let home = crate::composer::draw::HomeRows {
            notice: bg.home_notice.as_deref(),
            stats: bg.plugin_stats.as_deref(),
            model: Some(model_display.as_str()),
            model_tail: None,
            above: bg.home_update.as_deref(),
            session: bg.home_session.as_deref(),
            quality_of_life: bg.home_quality_of_life.as_deref(),
        };
        let chips = crate::composer::draw::attachment_cells(&bg.attachments, &bg.prompt_text);
        let ibox =
            crate::composer::draw::build_input_box(&bg.prompt_text, 0, w, None, Some(home), &chips);
        let band = ibox.lines;
        let transcript_avail_h = h.saturating_sub(band.len() + 1);
        let tail: &[Line<'static>] = if transcript.len() <= transcript_avail_h {
            &transcript
        } else {
            &transcript[transcript.len() - transcript_avail_h..]
        };

        let mut full_screen_lines: Vec<Line<'static>> = Vec::with_capacity(h);
        for l in tail {
            full_screen_lines.push(pad_backdrop_line(dim_line(l), w));
        }
        // The composer's text is intentionally not painted: a dimmed `❯` or
        // draft peeking out beside a centered modal reads as a stray glyph on
        // the modal's rows. Keep the band's blank silhouette instead.
        for l in band {
            let mut line = Line::from("").style(l.style);
            if line.style.bg.is_none() {
                line.style = line.style.bg(backdrop_bg());
            }
            full_screen_lines.push(pad_backdrop_line(line, w));
        }
        while full_screen_lines.len() + 1 < h {
            full_screen_lines.push(pad_backdrop_line(Line::from(""), w));
        }
        full_screen_lines.push(pad_backdrop_line(footer_line, w));
        full_screen_lines.truncate(h);

        for (i, line) in full_screen_lines.into_iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }
            Paragraph::new(line).render(Rect::new(area.x, y, area.width, 1), buf);
        }
        return;
    }

    let transcript_ends_blank = transcript.last().is_some_and(|l| {
        (l.style.bg.is_none() || l.style.bg == Some(backdrop_bg()))
            && l.spans.iter().all(|s| {
                (s.style.bg.is_none() || s.style.bg == Some(backdrop_bg()))
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

    for l in tail {
        full_screen_lines.push(pad_backdrop_line(dim_line(l), w));
    }
    if needs_gap {
        full_screen_lines.push(pad_backdrop_line(Line::from(""), w));
    }
    // Same as the home band above: keep the composer box's silhouette but
    // drop its text so a dimmed prompt can't peek out beside the modal.
    for l in bottom_box_lines {
        let mut line = Line::from("").style(l.style);
        if line.style.bg.is_none() {
            line.style = line.style.bg(backdrop_bg());
        }
        full_screen_lines.push(pad_backdrop_line(line, w));
    }
    full_screen_lines.push(pad_backdrop_line(footer_line, w));

    while full_screen_lines.len() < h {
        full_screen_lines.push(pad_backdrop_line(Line::from(""), w));
    }
    full_screen_lines.truncate(h);

    for (i, line) in full_screen_lines.into_iter().enumerate() {
        let y = area.y + i as u16;
        if y >= area.y + area.height {
            break;
        }
        Paragraph::new(line).render(Rect::new(area.x, y, area.width, 1), buf);
    }
}
