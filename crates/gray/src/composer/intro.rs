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

/// Frame index for `elapsed` into the intro. Frames advance at an even
/// cadence (smooth), with a light ease-out: quick off the mark, a gentle
/// landing, and no frozen stretch at either end.
pub(super) fn intro_frame_index(elapsed: Duration, last: usize) -> usize {
    let t = (elapsed.as_secs_f32() * 1000.0 / DRAW_MS as f32).clamp(0.0, 1.0);
    let eased = 0.6 * t + 0.4 * (1.0 - (1.0 - t) * (1.0 - t));
    ((eased * last as f32).round() as usize).min(last)
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

pub(super) const INTRO_FRAMES_TXT: &str = include_str!("../../assets/logo-intro-frames.txt");

pub(super) fn intro_frames() -> &'static [(Vec<String>, Vec<String>)] {
    static FRAMES: std::sync::OnceLock<Vec<(Vec<String>, Vec<String>)>> =
        std::sync::OnceLock::new();
    FRAMES.get_or_init(|| {
        let mut frames: Vec<(Vec<String>, Vec<String>)> = INTRO_FRAMES_TXT
            .replace("\r\n", "\n")
            .split("\n---\n")
            .filter_map(|block| {
                let rows: Vec<String> = block
                    .lines()
                    .filter(|l| !l.starts_with('#'))
                    .map(str::to_string)
                    .collect();
                (rows.len() == 24).then(|| {
                    let (mark, edge) = rows.split_at(12);
                    (mark.to_vec(), edge.to_vec())
                })
            })
            .collect();

        // The last baked frame is the finished mark; the settle grows into
        // it from the frame before, so drop it rather than jump to it and
        // then retract (the old backstep). Blank lead-in frames and the
        // baked hold (runs of identical frames) only stall the draw.
        let full = frames.pop();
        let blank = |rows: &[String]| {
            rows.iter()
                .all(|l| l.chars().all(|c| c == '\u{2800}' || c == ' '))
        };
        let mut timeline: Vec<(Vec<String>, Vec<String>)> = Vec::with_capacity(frames.len());
        for f in frames {
            if blank(&f.0) || timeline.last().is_some_and(|p| p.0 == f.0) {
                continue;
            }
            timeline.push(f);
        }
        let settle = timeline
            .last()
            .map(|(mark, _)| intro_settle_frames(mark))
            .unwrap_or_default();
        if settle.is_empty() {
            timeline.extend(full);
        } else {
            timeline.extend(settle);
        }
        // A settle from an already-complete frame is all repeats; every
        // repeated frame is dead time on the clock.
        timeline.dedup_by(|later, earlier| later.0 == earlier.0);
        timeline
    })
}

pub(super) const BRAILLE_BITS: [(usize, usize, u32); 8] = [
    (0, 0, 0x01),
    (0, 1, 0x02),
    (0, 2, 0x04),
    (0, 3, 0x40),
    (1, 0, 0x08),
    (1, 1, 0x10),
    (1, 2, 0x20),
    (1, 3, 0x80),
];

pub(super) const MARK_CELLS: (usize, usize) = (32, 12);

pub(super) const INTRO_SETTLE_FRAMES: usize = 12;

pub(super) fn braille_dots(rows: &[String]) -> std::collections::BTreeSet<(usize, usize)> {
    let mut out = std::collections::BTreeSet::new();
    for (r, line) in rows.iter().enumerate() {
        for (c, ch) in line.chars().enumerate() {
            if !('\u{2800}'..='\u{28ff}').contains(&ch) {
                continue;
            }
            let bits = ch as u32 - 0x2800;
            for &(dx, dy, bit) in &BRAILLE_BITS {
                if bits & bit != 0 {
                    out.insert((c * 2 + dx, r * 4 + dy));
                }
            }
        }
    }
    out
}

pub(super) fn dots_braille(dots: &std::collections::BTreeSet<(usize, usize)>) -> Vec<String> {
    let (cx, cy) = MARK_CELLS;
    (0..cy)
        .map(|r| {
            (0..cx)
                .map(|c| {
                    let bits = BRAILLE_BITS.iter().fold(0, |bits, &(dx, dy, bit)| {
                        if dots.contains(&(c * 2 + dx, r * 4 + dy)) {
                            bits | bit
                        } else {
                            bits
                        }
                    });
                    char::from_u32(0x2800 + bits).unwrap()
                })
                .collect()
        })
        .collect()
}

pub(super) fn intro_settle_frames(from: &[String]) -> Vec<(Vec<String>, Vec<String>)> {
    let (cx, cy) = MARK_CELLS;
    // Settle math stays in braille space on every terminal: the render pass
    // maps cells to an ASCII ramp when unicode is unsafe, and LOGO_ASCII rows
    // carry no dot data for braille_dots() to settle toward.
    let blank = '\u{2800}';
    let mut target: Vec<String> = crate::tui::logo_mark()
        .iter()
        .map(|l| {
            let mut s = l.to_string();
            while s.chars().count() < cx {
                s.push(blank);
            }
            s
        })
        .collect();
    target.resize(cy, blank.to_string().repeat(cx));
    let want = braille_dots(&target);
    let mut ink = braille_dots(from);
    if ink.difference(&want).next().is_some() {
        return Vec::new();
    }

    let mut frontier = ink.clone();
    let mut missing: std::collections::BTreeSet<(usize, usize)> =
        want.difference(&ink).copied().collect();
    let mut ordered = Vec::with_capacity(missing.len());
    while !missing.is_empty() {
        let mut ring: Vec<(usize, usize)> = missing
            .iter()
            .copied()
            .filter(|&(x, y)| {
                (-1isize..=1).any(|ox| {
                    (-1isize..=1).any(|oy| {
                        x.checked_add_signed(ox)
                            .zip(y.checked_add_signed(oy))
                            .is_some_and(|p| frontier.contains(&p))
                    })
                })
            })
            .collect();
        if ring.is_empty() {
            ring = missing.iter().copied().collect();
        }
        ring.sort_by_key(|&(x, y)| (y, x));
        for d in ring {
            frontier.insert(d);
            missing.remove(&d);
            ordered.push(d);
        }
    }
    let grow = INTRO_SETTLE_FRAMES - 2;
    (0..INTRO_SETTLE_FRAMES)
        .map(|i| {
            let add: &[(usize, usize)] = if i < grow {
                &ordered[i * ordered.len() / grow..(i + 1) * ordered.len() / grow]
            } else {
                &[]
            };
            ink.extend(add.iter().copied());
            let fresh: std::collections::BTreeSet<(usize, usize)> =
                add.iter().map(|&(x, y)| (x / 2, y / 4)).collect();
            let mark = if ink == want {
                target.clone()
            } else {
                dots_braille(&ink)
            };
            let edge = (0..cy)
                .map(|r| {
                    (0..cx)
                        .map(|c| if fresh.contains(&(c, r)) { '1' } else { '0' })
                        .collect()
                })
                .collect();
            (mark, edge)
        })
        .collect()
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

    let base = theme.text_dim;
    let hilite = theme.text_bright;
    let frames = intro_frames();
    let last = frames.len().saturating_sub(1);
    let frame_idx = match intro {
        Some(elapsed) => intro_frame_index(elapsed, last),
        None => ((progress.clamp(0.0, 1.0) * last as f32).round() as usize).min(last),
    };
    let ms = intro.map(|e| e.as_secs_f32() * 1000.0);
    let mark_alpha = ms.map_or(1.0, |ms| ease_out_cubic(ms / MARK_FADE_MS));
    let title_alpha = ms.map_or(1.0, |ms| {
        ease_out_cubic((ms - TITLE_FADE_MS.0) / (TITLE_FADE_MS.1 - TITLE_FADE_MS.0))
    });
    let empty = (Vec::new(), Vec::new());
    let (mark_rows, edge_rows) = frames.get(frame_idx).unwrap_or(&empty);
    let l_rows = mark_rows.len().max(1) as f32;
    let max_logo_w = mark_rows
        .iter()
        .map(|l| display_width(l.trim()))
        .max()
        .unwrap_or(0);
    let l_cols = (max_logo_w as f32).max(1.0);
    let logo_pad = x + col_w.saturating_sub(max_logo_w) / 2;

    let safe = crate::tui::unicode_safe();
    // Only while drawing: the settled banner is the plain mark.
    let heat =
        (safe && intro.is_some() && frame_idx < last).then(|| glow::heat_map(frames, frame_idx));
    for (row, line) in mark_rows.iter().enumerate() {
        // Fresh-ink highlights belong to the draw; the settled banner is
        // one even gradient.
        let edge = match intro {
            Some(_) => edge_rows.get(row).map(String::as_str).unwrap_or(""),
            None => "",
        };
        let mut spans: Vec<Span<'static>> = Vec::new();
        if logo_pad > 0 {
            spans.push(Span::raw(" ".repeat(logo_pad)));
        }
        for (col, ch) in line.chars().enumerate() {
            let ch = if safe {
                ch
            } else {
                crate::tui::braille_fallback(ch)
            };
            let h = heat
                .as_ref()
                .and_then(|m| m.get(row)?.get(col).copied())
                .unwrap_or(0.0);
            let ch = glow::heat_glyph(ch, h);
            if ch.is_whitespace() || ch == ' ' {
                spans.push(Span::raw(" "));
                continue;
            }
            let mut color = if edge.chars().nth(col) == Some('1') {
                hilite
            } else {
                let diag = (col as f32 + (l_rows - 1.0 - row as f32)) / (l_cols + l_rows);
                let t = (0.15 + 0.85 * diag).clamp(0.0, 1.0);
                crate::tui::blend_color(base, hilite, t)
            };
            if h > 0.0 {
                color = crate::tui::blend_color(color, hilite, h);
            }
            spans.push(Span::styled(
                ch.to_string(),
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
    let start = y0 - overflow;
    if overflow > 0 {
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\x1b[{h};1H{}", "\n".repeat(overflow));
        let _ = out.flush();
    }

    terminal.set_viewport_area(ratatui::layout::Rect::new(
        0,
        start as u16,
        w as u16,
        (h - start) as u16,
    ));

    for _ in 0..64 {
        match crossterm::event::poll(Duration::ZERO) {
            Ok(true) => {
                let _ = crossterm::event::read();
            }
            _ => break,
        }
    }
    Some(start as u16)
}
