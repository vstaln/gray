//! The graychan mascot: the anime art, painted into the terminal as
//! half-blocks. `/hehe` only — the welcome screen stays the plain ASCII
//! logo unless the user asks for graychan.
//!
//! One character cell carries two vertical pixels — `▀` takes the top
//! pixel as its foreground and the bottom pixel as its background — so a
//! truecolor terminal shows the art at the full cell grid with no graphics
//! protocol and no extra dependency beyond the `image` crate the view tool
//! already uses. Non-truecolor terminals (`NO_COLOR`) and tiny terminals
//! decline it, so the art never degenerates into escape-code soup.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Upper half block: top pixel becomes the glyph's fg, bottom pixel its bg.
const BLOCK: char = '\u{2580}';

/// One horizontal same-color cell run: top RGB, bottom RGB, width in cells.
type MascotRun = ((u8, u8, u8), (u8, u8, u8), usize);

/// The mascot, cropped to its content box and downscaled to 1024px wide
/// (the original is 4082px): wide enough for any terminal, small enough to
/// `include_bytes!` at ~330 KB. Aspect 1024:902 is preserved at render time.
const MASCOT_PNG: &[u8] = include_bytes!("../assets/graychan.png");

/// `/hehe` toggle state: graychan is in the transcript (`true`) vs the
/// default gray ASCII banner (`false`). Process-wide because the composer
/// and the piped path are two skins of the same session and both need the
/// toggle. Nothing in the default render path reads it, so the startup
/// screen never depends on it.
static MASCOT_SHOWN: AtomicBool = AtomicBool::new(false);

/// Is the graychan art currently in the transcript?
pub(crate) fn mascot_shown() -> bool {
    MASCOT_SHOWN.load(Ordering::Relaxed)
}

/// Record whether the graychan art is in the transcript.
pub(crate) fn set_mascot_shown(shown: bool) {
    MASCOT_SHOWN.store(shown, Ordering::Relaxed);
}

/// Decoded + rescaled mascot: `cols` cell columns, `rows` cell rows, and an
/// RGB buffer of `cols * rows * 2` pixels (two pixel rows per cell row).
pub(crate) struct MascotGrid {
    cols: usize,
    rows: usize,
    px: Vec<u8>,
}

impl MascotGrid {
    fn px(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let i = (y * self.cols + x) * 3;
        (self.px[i], self.px[i + 1], self.px[i + 2])
    }

    /// Horizontal same-color runs of one cell row as `(top, bottom, width)`.
    /// Both renderers walk this so the run merging lives in one place: the
    /// white margins cost a single styled run, not one span per column.
    fn runs(&self, row: usize) -> Vec<MascotRun> {
        let mut out = Vec::new();
        let mut x = 0;
        while x < self.cols {
            let top = self.px(x, row * 2);
            let bot = self.px(x, row * 2 + 1);
            let mut n = 1;
            while x + n < self.cols
                && self.px(x + n, row * 2) == top
                && self.px(x + n, row * 2 + 1) == bot
            {
                n += 1;
            }
            out.push((top, bot, n));
            x += n;
        }
        out
    }

    /// Cell rows as half-block lines, each row prefixed with `pad` spaces.
    fn lines(&self, pad: usize) -> Vec<Line<'static>> {
        let mut out = Vec::with_capacity(self.rows);
        for row in 0..self.rows {
            let mut spans: Vec<Span<'static>> = Vec::new();
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
            }
            for (top, bot, n) in self.runs(row) {
                spans.push(Span::styled(
                    BLOCK.to_string().repeat(n),
                    Style::default()
                        .fg(Color::Rgb(top.0, top.1, top.2))
                        .bg(Color::Rgb(bot.0, bot.1, bot.2)),
                ));
            }
            out.push(Line::from(spans));
        }
        out
    }
}

/// Decodes and rescales the mascot to a half-block grid that fits
/// `term_cols` x `term_rows`. The grid takes every column it can use but
/// keeps 9 rows of headroom so the version banner and the input viewport
/// stay on screen under it (a fully visible mascot beats a scrolled one).
/// Too-small terminals (or an undecodable asset) yield `None` — the caller
/// then falls back to the ASCII logo.
pub(crate) fn decode_grid(term_cols: u16, term_rows: u16) -> Option<MascotGrid> {
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
        (grid_rows * 2) as u32,
        image::imageops::FilterType::Lanczos3,
    );
    Some(MascotGrid {
        cols: grid_cols,
        rows: grid_rows,
        px: scaled.into_raw(),
    })
}

/// Mascot lines for a terminal of `term_cols` x `term_rows`, horizontally
/// centered within `center_in` columns (the transcript width). Returns
/// `None` when the terminal can't show it (`NO_COLOR`, too small) so the
/// caller can say so instead of drawing nothing.
pub(crate) fn mascot_lines(
    term_cols: u16,
    term_rows: u16,
    center_in: Option<usize>,
) -> Option<Vec<Line<'static>>> {
    if std::env::var_os("NO_COLOR").is_some() {
        return None;
    }
    mascot_lines_unchecked(term_cols, term_rows, center_in)
}

/// The grid→lines render without the `NO_COLOR` gate — tests and callers
/// that have already decided the terminal can paint.
fn mascot_lines_unchecked(
    term_cols: u16,
    term_rows: u16,
    center_in: Option<usize>,
) -> Option<Vec<Line<'static>>> {
    let grid = decode_grid(term_cols, term_rows)?;
    let pad = center_in
        .unwrap_or(term_cols as usize)
        .saturating_sub(grid.cols)
        / 2;
    Some(grid.lines(pad))
}

/// Paints the mascot straight to stdout as ANSI half-blocks (the non-composer
/// path: piped/`-p` runs where no TUI owns the terminal). Returns `false`
/// when nothing was drawn so callers can explain why.
pub(crate) fn print_mascot() -> bool {
    if !std::io::stdout().is_terminal() {
        return false;
    }
    let (cols, rows) = crossterm::terminal::size().unwrap_or((0, 0));
    let Some(grid) = decode_grid(cols, rows) else {
        return false;
    };
    let pad = (cols as usize).saturating_sub(grid.cols) / 2;
    let mut out = std::io::stdout().lock();
    for row in 0..grid.rows {
        let mut line = String::new();
        if pad > 0 {
            line.push_str(&" ".repeat(pad));
        }
        for (top, bot, n) in grid.runs(row) {
            line.push_str(&format!(
                "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m{}",
                top.0,
                top.1,
                top.2,
                bot.0,
                bot.1,
                bot.2,
                BLOCK.to_string().repeat(n)
            ));
        }
        line.push_str("\x1b[0m\r\n");
        let _ = out.write_all(line.as_bytes());
    }
    let _ = out.flush();
    true
}

/// The piped `/hehe` toggle: graychan while it is off, the plain ASCII
/// logo when it is on again (the revert). Returns false when graychan
/// can't be painted (no tty, `NO_COLOR`, tiny terminal), which also leaves
/// the toggle off so the next press tries again.
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

#[path = "mascot_tests.rs"]
#[cfg(test)]
mod tests;
