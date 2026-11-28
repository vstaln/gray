use super::*;

pub(crate) fn stats_spans(stats: &str) -> Vec<Span<'static>> {
    vec![Span::styled(
        stats.to_string(),
        Style::default().fg(crate::theme::theme().text_dim),
    )]
}

pub(crate) fn home_column(w: usize) -> (usize, usize) {
    let col_w = w.saturating_sub(8).clamp(1, 78);
    (w.saturating_sub(col_w) / 2, col_w)
}

/// The mark draws and the title fades in over this window …
pub(super) const DRAW_MS: u64 = 1200;

pub(super) const INTRO_TOTAL_MS: u64 = DRAW_MS;

/// … then the steady home band (input box, model row, hints) fades in.
pub(crate) const HOME_REVEAL_MS: u64 = 300;

/// The mark fades up from the background over its first beat.
pub(super) const MARK_FADE_MS: f32 = 300.0;

/// The title fades in once the mark is mostly drawn.
pub(super) const TITLE_FADE_MS: (f32, f32) = (600.0, 1100.0);

pub(super) fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub(super) fn intro_progress(elapsed: Duration) -> f32 {
    let t = (elapsed.as_secs_f32() * 1000.0 / DRAW_MS as f32).clamp(0.0, 1.0);
    0.6 * t + 0.4 * (1.0 - (1.0 - t) * (1.0 - t))
}

/// Colour a fade starts from: the window background when it's a real
/// colour, black for the terminal default (the stock palette is dark).
pub(crate) fn fade_floor() -> ratatui::style::Color {
    match crate::theme::theme().window() {
        c @ ratatui::style::Color::Rgb(..) => c,
        _ => ratatui::style::Color::Rgb(0, 0, 0),
    }
}

/// `color` at `alpha` (0 = background, 1 = itself). Non-RGB colours
/// (NO_COLOR, named palettes) have nothing to blend and show as-is.
pub(crate) fn faded(color: ratatui::style::Color, alpha: f32) -> ratatui::style::Color {
    if alpha >= 1.0 {
        return color;
    }
    match color {
        ratatui::style::Color::Rgb(..) => crate::tui::blend_color(fade_floor(), color, alpha),
        other => other,
    }
}

/// Fade every cell of a drawn frame toward the background.
pub(crate) fn fade_buffer(buf: &mut ratatui::buffer::Buffer, alpha: f32) {
    if alpha >= 1.0 {
        return;
    }
    let area = buf.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            let fg = match cell.fg {
                ratatui::style::Color::Reset => crate::theme::theme().text_bright,
                c => c,
            };
            cell.fg = faded(fg, alpha);
            cell.bg = faded(cell.bg, alpha);
        }
    }
}

pub(super) fn welcome_body_lines(
    w: usize,
    progress: f32,
    intro: Option<Duration>,
) -> Vec<Line<'static>> {
    let (x, col_w) = home_column(w);
    welcome_body_lines_at(x, col_w, progress, intro)
}

pub(crate) fn welcome_body_lines_at(
    x: usize,
    col_w: usize,
    progress: f32,
    intro: Option<Duration>,
) -> Vec<Line<'static>> {
    let theme = crate::theme::theme();
    let version = env!("CARGO_PKG_VERSION").to_string();

    let centered = |segs: Vec<Span<'static>>, text_w: usize| -> Line<'static> {
        let pad = x + col_w.saturating_sub(text_w) / 2;
        let mut spans = Vec::with_capacity(segs.len() + 1);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.extend(segs);
        Line::from(spans)
    };

    let mut body: Vec<Line<'static>> = Vec::new();

    let time = match intro {
        Some(elapsed) => crate::logo::intro_time(intro_progress(elapsed)),
        None => crate::logo::intro_time(progress),
    };
    let ms = intro.map(|e| e.as_secs_f32() * 1000.0);
    let mark_alpha = ms.map_or(1.0, |ms| ease_out_cubic(ms / MARK_FADE_MS));
    let title_alpha = ms.map_or(1.0, |ms| {
        ease_out_cubic((ms - TITLE_FADE_MS.0) / (TITLE_FADE_MS.1 - TITLE_FADE_MS.0))
    });
    let mark = crate::logo::mark_at(time);
    let mark_w = mark.first().map_or(0, Vec::len);
    let logo_pad = x + col_w.saturating_sub(mark_w) / 2;
    let safe = crate::tui::unicode_safe();
    for cells in &mark {
        let mut spans: Vec<Span<'static>> = Vec::new();
        if logo_pad > 0 {
            spans.push(Span::raw(" ".repeat(logo_pad)));
        }
        for cell in cells {
            if cell.glyph == ' ' {
                spans.push(Span::raw(" "));
                continue;
            }
            let glyph = if safe {
                cell.glyph
            } else {
                crate::logo::ascii(cell.glyph)
            };
            let color = crate::tui::blend_color(
                ratatui::style::Color::Rgb(100, 100, 100),
                theme.text_bright,
                cell.glow,
            );
            spans.push(Span::styled(
                glyph.to_string(),
                Style::default().fg(faded(color, mark_alpha)),
            ));
        }
        body.push(Line::from(spans));
    }
    body.push(Line::from(""));

    let mut title = match intro {
        Some(elapsed) => draw::shimmer_spans("gray", elapsed),
        None => vec![Span::styled(
            "gray",
            Style::default().bold().fg(theme.text_bright),
        )],
    };
    title.push(Span::raw(" "));
    if safe {
        title.push(Span::styled(
            "\u{25c6}",
            Style::default().fg(theme.text_muted),
        ));
        title.push(Span::raw(" "));
    }
    let title_w =
        display_width(if safe { "gray \u{25c6} " } else { "gray " }) + display_width(&version);
    title.push(Span::styled(version, Style::default().fg(theme.text_muted)));
    if title_alpha < 1.0 {
        for span in &mut title {
            if let Some(fg) = span.style.fg {
                span.style.fg = Some(faded(fg, title_alpha));
            }
        }
    }
    body.push(centered(title, title_w));
    body
}

pub(crate) fn welcome_lines_at(
    w: usize,
    progress: f32,
    intro: Option<Duration>,
) -> Vec<Line<'static>> {
    let body = welcome_body_lines(w, progress, intro);
    let rows = crossterm::terminal::size()
        .map(|(_, r)| r as usize)
        .unwrap_or(0);
    let top = rows.saturating_sub(body.len() + draw::home_band_h(w)) / 2;
    let mut welcome_lines = Vec::with_capacity(top + body.len());
    for _ in 0..top {
        welcome_lines.push(Line::from(""));
    }
    welcome_lines.extend(body);
    welcome_lines
}

pub fn build_welcome_lines(w: usize) -> Vec<Line<'static>> {
    welcome_lines_at(w, 1.0, None)
}

pub(super) fn intro_stage(
    terminal: &mut Term,
    w: usize,
    h: usize,
    welcome_h: usize,
) -> Option<u16> {
    let y0 = terminal.viewport_area.y as usize;
    let overflow = (y0 + welcome_h + draw::home_band_h(w)).saturating_sub(h);
    if overflow > y0 {
        return None;
    }
    if overflow > 0 {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\x1b[{h};1H{}", "\n".repeat(overflow));
        let _ = out.flush();
    }

    terminal.set_viewport_area(ratatui::layout::Rect::new(0, 0, w as u16, h as u16));

    for _ in 0..64 {
        match crossterm::event::poll(Duration::ZERO) {
            Ok(true) => {
                let _ = crossterm::event::read();
            }
            _ => break,
        }
    }
    Some(0)
}
