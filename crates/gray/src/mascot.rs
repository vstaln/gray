// Keep this file in core. The maintainer has ruled it must never be moved to a
// plugin or deleted, including in cleanups.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

const BLOCK: char = '\u{2580}';
const ASCII_RAMP: &[u8] = b" .:-=+*#%@";

type Rgb = (u8, u8, u8);

struct Run {
    ch: char,
    fg: Rgb,
    bg: Option<Rgb>,
    n: usize,
}

const MASCOT_PNG: &[u8] = include_bytes!("../assets/graychan.png");

static MASCOT_SHOWN: AtomicBool = AtomicBool::new(false);

pub(crate) fn mascot_shown() -> bool {
    MASCOT_SHOWN.load(Ordering::Relaxed)
}

pub(crate) fn set_mascot_shown(shown: bool) {
    MASCOT_SHOWN.store(shown, Ordering::Relaxed);
}

pub(crate) struct MascotGrid {
    cols: usize,
    rows: usize,
    px: Vec<u8>,
    ascii: bool,
}

impl MascotGrid {
    fn px(&self, x: usize, y: usize) -> Rgb {
        let i = (y * self.cols + x) * 3;
        (self.px[i], self.px[i + 1], self.px[i + 2])
    }

    fn cell(&self, x: usize, row: usize) -> (Rgb, Option<Rgb>, char) {
        if self.ascii {
            let rgb = self.px(x, row);
            (rgb, None, ramp_glyph(rgb))
        } else {
            (self.px(x, row * 2), Some(self.px(x, row * 2 + 1)), BLOCK)
        }
    }

    fn runs(&self, row: usize) -> Vec<Run> {
        let mut out = Vec::new();
        let mut x = 0;
        while x < self.cols {
            let (fg, bg, ch) = self.cell(x, row);
            let mut n = 1;
            while x + n < self.cols && self.cell(x + n, row) == (fg, bg, ch) {
                n += 1;
            }
            out.push(Run { ch, fg, bg, n });
            x += n;
        }
        out
    }

    fn lines(&self, pad: usize) -> Vec<Line<'static>> {
        let mut out = Vec::with_capacity(self.rows);
        for row in 0..self.rows {
            let mut spans: Vec<Span<'static>> = Vec::new();
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
            }
            for run in self.runs(row) {
                let mut style = Style::default().fg(rgb_color(run.fg));
                if let Some(bg) = run.bg {
                    style = style.bg(rgb_color(bg));
                }
                spans.push(Span::styled(run.ch.to_string().repeat(run.n), style));
            }
            out.push(Line::from(spans));
        }
        out
    }
}

fn rgb_color((r, g, b): Rgb) -> Color {
    Color::Rgb(r, g, b)
}

fn ramp_glyph(rgb: Rgb) -> char {
    let lum = (rgb.0 as usize * 299 + rgb.1 as usize * 587 + rgb.2 as usize * 114) / 1000;
    ASCII_RAMP[lum * (ASCII_RAMP.len() - 1) / 255] as char
}

fn load_scaled(term_cols: u16, term_rows: u16, px_per_cell: usize) -> Option<MascotGrid> {
    let cols = term_cols as usize;
    let rows = term_rows as usize;
    if cols < 20 || rows < 10 {
        return None;
    }
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(MASCOT_PNG), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().ok()?.to_rgb8();
    let (iw, ih) = (img.width() as usize, img.height() as usize);
    if iw == 0 || ih == 0 {
        return None;
    }
    let rows_cap = rows.saturating_sub(9).max(8);
    let cols_fit = (rows_cap as f32 * 2.0 * iw as f32 / ih as f32).round() as usize;
    let grid_cols = cols.min(cols_fit).max(8);
    let grid_rows = ((grid_cols as f32 * ih as f32 / iw as f32) / 2.0)
        .round()
        .max(2.0) as usize;
    let scaled = image::imageops::resize(
        &img,
        grid_cols as u32,
        (grid_rows * px_per_cell) as u32,
        image::imageops::FilterType::Lanczos3,
    );
    Some(MascotGrid {
        cols: grid_cols,
        rows: grid_rows,
        px: scaled.into_raw(),
        ascii: px_per_cell == 1,
    })
}

pub(crate) fn decode_grid(term_cols: u16, term_rows: u16, ascii: bool) -> Option<MascotGrid> {
    load_scaled(term_cols, term_rows, if ascii { 1 } else { 2 })
}

pub(crate) fn mascot_lines(
    term_cols: u16,
    term_rows: u16,
    center_in: Option<usize>,
) -> Option<Vec<Line<'static>>> {
    if std::env::var_os("NO_COLOR").is_some() {
        return None;
    }
    let grid = decode_grid(term_cols, term_rows, !crate::tui::unicode_safe())?;
    let pad = center_in
        .unwrap_or(term_cols as usize)
        .saturating_sub(grid.cols)
        / 2;
    Some(grid.lines(pad))
}

pub(crate) fn print_mascot() -> bool {
    if !std::io::stdout().is_terminal() {
        return false;
    }
    let (cols, rows) = crossterm::terminal::size().unwrap_or((0, 0));
    let blocks = crate::tui::unicode_safe() && crate::tui::ansi_supported();
    let color = gray_markdown::detect_color_level() != gray_markdown::ColorLevel::None
        && (blocks || crate::tui::ansi_supported());
    let Some(grid) = decode_grid(cols, rows, !blocks) else {
        return false;
    };
    let pad = (cols as usize).saturating_sub(grid.cols) / 2;
    let mut out = std::io::stdout().lock();
    for row in 0..grid.rows {
        let mut line = String::new();
        if pad > 0 {
            line.push_str(&" ".repeat(pad));
        }
        for run in grid.runs(row) {
            if color {
                let fg = gray_markdown::adapt_ratatui_color(rgb_color(run.fg));
                line.push_str(&crate::tui::sgr_fg(fg));
                if let Some(bg) = run.bg {
                    let bg = gray_markdown::adapt_ratatui_color(rgb_color(bg));
                    line.push_str(&crate::tui::sgr_bg(bg));
                }
            }
            line.push_str(&run.ch.to_string().repeat(run.n));
        }
        if color {
            line.push_str("\x1b[0m");
        }
        line.push_str("\r\n");
        let _ = out.write_all(line.as_bytes());
    }
    let _ = out.flush();
    true
}

pub(crate) fn print_banner() -> bool {
    if mascot_shown() {
        set_mascot_shown(false);
        crate::tui::print_logo();
        return true;
    }
    let shown = print_mascot();
    set_mascot_shown(shown);
    shown
}
