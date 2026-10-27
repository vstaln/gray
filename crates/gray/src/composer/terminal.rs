use std::collections::VecDeque;
use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::widgets::Widget;

pub struct Frame<'a> {
    pub(crate) cursor_position: Option<Position>,
    pub(crate) viewport_area: Rect,
    pub(crate) buffer: &'a mut Buffer,
}

impl Frame<'_> {
    pub const fn area(&self) -> Rect {
        self.viewport_area
    }

    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scrollback {
    Regions,

    FullScreen,
}

impl Scrollback {
    pub(crate) fn detect() -> Self {
        if std::env::var_os("WT_SESSION").is_some() {
            Self::FullScreen
        } else {
            Self::Regions
        }
    }
}

pub struct CustomTerminal<B>
where
    B: Backend + Write,
{
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    pub hidden_cursor: bool,
    pub viewport_area: Rect,
    pub last_known_screen_size: Size,
    scrollback: Scrollback,
    covered: u16,
    /// The last screen-height of transcript rows, newest last; the final
    /// `covered` of them sit hidden under the band.
    tail: VecDeque<Vec<Cell>>,
}

impl<B> Drop for CustomTerminal<B>
where
    B: Backend + Write,
{
    fn drop(&mut self) {
        if self.hidden_cursor {
            let _ = self.backend.show_cursor();
        }
        let _ = Backend::flush(&mut self.backend);
    }
}

impl<B> CustomTerminal<B>
where
    B: Backend + Write,
{
    pub fn with_options(mut backend: B) -> io::Result<Self> {
        let screen_size = backend.size()?;
        let cursor_pos = backend
            .get_cursor_position()
            .unwrap_or(Position { x: 0, y: 0 });
        Ok(Self::at_row(
            backend,
            screen_size,
            cursor_pos.y,
            Scrollback::detect(),
        ))
    }

    pub(crate) fn at_row(backend: B, screen_size: Size, y: u16, scrollback: Scrollback) -> Self {
        let y = y.min(screen_size.height.saturating_sub(1));
        let viewport_area = Rect::new(0, y, screen_size.width, 0);
        let mut term = Self {
            backend,
            buffers: [Buffer::empty(Rect::ZERO), Buffer::empty(Rect::ZERO)],
            current: 0,
            hidden_cursor: false,
            viewport_area,
            last_known_screen_size: screen_size,
            scrollback,
            covered: 0,
            tail: VecDeque::new(),
        };
        term.set_viewport_area(viewport_area);
        term
    }

    pub fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport_area = area;
    }

    pub fn set_viewport_height(
        &mut self,
        height: u16,
        overlay: u16,
        screen_size: Size,
    ) -> io::Result<()> {
        let screen_shrank = screen_size.height < self.last_known_screen_size.height;
        if screen_size != self.last_known_screen_size {
            self.covered = 0;
            self.tail.clear();
        }
        self.last_known_screen_size = screen_size;
        let old = self.viewport_area;
        let mut area = old;
        area.height = height.min(screen_size.height);
        area.width = screen_size.width;
        let bottom = screen_size.height.saturating_sub(area.height);
        // The band sits directly under the transcript tail; there is never
        // a blank row between them. Growth extends it downward into free
        // rows first, then rises — letting an overlay cover transcript rows
        // it can repaint later, and scrolling history only for the rest.
        // A shrink first gives back the rows an overlay was covering, then
        // keeps its top row and frees rows *below* itself; the next insert
        // pushes it back down. The one exception is the bottom-pinned slide
        // below, which repaints every row above the band from `tail` (never
        // a bare slide: scrollback can't be edited, so shifting without
        // repainting would leave blank rows at the scrollback seam).
        let shrink = old.height.saturating_sub(area.height);
        let uncover = if old.bottom() >= screen_size.height {
            self.covered.min(shrink)
        } else {
            0
        };
        area.y = (old.y + uncover).min(bottom);
        // A band pinned to the screen bottom that shrinks with nothing
        // covered (deleting draft lines after growth pushed history up)
        // slides back down and repaints the history rows it had pushed off,
        // from `tail`, instead of leaving blank rows under the footer. The
        // repainted rows also sit in scrollback, so scrolling back can show
        // them twice; a few repeated rows read better than a dead strip.
        let slide = if uncover == 0
            && self.covered == 0
            && !screen_shrank
            && old.bottom() >= screen_size.height
        {
            let spare = u16::try_from(self.tail.len().saturating_sub(usize::from(old.y)))
                .unwrap_or(u16::MAX);
            shrink.min(spare).min(bottom.saturating_sub(area.y))
        } else {
            0
        };
        area.y += slide;
        let rise = old.y.saturating_sub(area.y);
        let history_end = old.y.saturating_add(self.covered);
        let repaintable = u16::try_from(self.tail.len())
            .unwrap_or(u16::MAX)
            .min(history_end);
        let hide = rise
            .min(overlay.saturating_sub(self.covered))
            .min(repaintable.saturating_sub(self.covered));
        self.covered += hide;
        if rise > hide && !screen_shrank {
            self.scroll_history_up(old.y, rise - hide)?;
        }

        if area != self.viewport_area {
            let from = if self.viewport_area.is_empty() {
                area.y
            } else {
                area.y.min(self.viewport_area.y)
            };
            self.set_viewport_area(area);
            self.clear_after_position(Position::new(0, from))?;
        }
        if slide > 0 {
            let n = usize::from(area.y);
            let rows: Vec<Vec<Cell>> = self.tail.range(self.tail.len() - n..).cloned().collect();
            let rows: Vec<&[Cell]> = rows.iter().map(Vec::as_slice).collect();
            self.draw_rows(0, &rows)?;
        }
        if uncover > 0 {
            let start = self.tail.len() - usize::from(self.covered);
            let rows: Vec<Vec<Cell>> = self
                .tail
                .range(start..start + usize::from(uncover))
                .cloned()
                .collect();
            self.covered -= uncover;
            let rows: Vec<&[Cell]> = rows.iter().map(Vec::as_slice).collect();
            self.draw_rows(old.y, &rows)?;
        }
        Ok(())
    }

    pub fn clear_after_position(&mut self, position: Position) -> io::Result<()> {
        if position.y < self.last_known_screen_size.height {
            self.backend.set_cursor_position(position)?;
            self.backend.clear_region(ClearType::AfterCursor)?;
        }

        self.buffers[1 - self.current].reset();
        Ok(())
    }

    pub fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.clear_after_position(self.viewport_area.as_position())
    }

    pub(crate) fn clear_scrollback_and_screen(&mut self, screen_size: Size) -> io::Result<()> {
        write!(self.backend, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        crate::composer::kitty::reset_transmitted();
        Backend::flush(&mut self.backend)?;
        self.last_known_screen_size = screen_size;
        self.covered = 0;
        self.tail.clear();
        self.set_viewport_area(Rect::new(0, 0, screen_size.width, 0));
        self.buffers[1 - self.current].reset();
        Ok(())
    }

    pub fn hide_cursor(&mut self) -> io::Result<()> {
        self.backend.hide_cursor()?;
        self.hidden_cursor = true;
        Ok(())
    }

    pub fn show_cursor(&mut self) -> io::Result<()> {
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.backend.set_cursor_position(position)?;
        Ok(())
    }

    pub fn swap_buffers(&mut self) {
        self.buffers[1 - self.current].reset();
        self.current = 1 - self.current;
    }

    /// All console output — frame diffs here, inserted transcript rows via
    /// draw_rows — gets transliterated to ASCII when the terminal cannot
    /// shape decorative glyphs. Every mapped glyph is single-width, so
    /// positions from the diff still line up after transliteration.
    pub fn flush(&mut self) -> io::Result<()> {
        let updates = {
            let previous_buffer = &self.buffers[1 - self.current];
            let current_buffer = &self.buffers[self.current];
            previous_buffer.diff(current_buffer)
        };
        if crate::tui::unicode_safe() {
            return self.backend.draw(updates.into_iter());
        }
        let owned: Vec<(u16, u16, Cell)> = updates
            .into_iter()
            .map(|(x, y, cell)| (x, y, ascii_cell(cell)))
            .collect();
        self.backend.draw(owned.iter().map(|(x, y, c)| (*x, *y, c)))
    }

    pub fn get_frame(&mut self) -> Frame<'_> {
        Frame {
            cursor_position: None,
            viewport_area: self.viewport_area,
            buffer: &mut self.buffers[self.current],
        }
    }

    pub fn draw<F>(&mut self, render_callback: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        // A 0×0 viewport (hidden pane, transient zero-size resize) hands the
        // callback an empty buffer — any unconditional write past row 0
        // panics on index-out-of-bounds. Nothing can be painted anyway.
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        let mut frame = self.get_frame();
        render_callback(&mut frame);
        // Uploads queued while composing this frame must land before the
        // placeholder cells that reference them — terminals don't all
        // retro-fill placeholders drawn ahead of the image.
        crate::composer::kitty::flush_transmits();

        let cursor_position = frame.cursor_position;

        self.flush()?;

        match cursor_position {
            None => self.hide_cursor()?,
            Some(position) => {
                self.set_cursor_position(position)?;
                self.show_cursor()?;
            }
        }

        self.swap_buffers();
        Backend::flush(&mut self.backend)?;
        Ok(())
    }

    pub fn insert_before<F>(&mut self, height: u16, draw_fn: F) -> io::Result<()>
    where
        F: FnOnce(&mut Buffer),
    {
        let width = self.viewport_area.width.max(1);
        if height == 0 || self.last_known_screen_size.height == 0 {
            return Ok(());
        }
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
        draw_fn(&mut buffer);
        crate::composer::kitty::flush_transmits();
        let hidden: Vec<Vec<Cell>> = self
            .tail
            .drain(self.tail.len() - usize::from(self.covered)..)
            .collect();
        self.covered = 0;
        let all_rows: Vec<&[Cell]> = hidden
            .iter()
            .map(Vec::as_slice)
            .chain(buffer.content.chunks(width as usize))
            .collect();
        let keep = usize::from(self.last_known_screen_size.height);
        let recent: Vec<Vec<Cell>> = all_rows[all_rows.len().saturating_sub(keep)..]
            .iter()
            .map(|row| row.to_vec())
            .collect();
        let mut rows = all_rows.as_slice();

        if self.scrollback == Scrollback::Regions {
            let screen_h = self.last_known_screen_size.height;
            let mut area = self.viewport_area;
            if area.bottom() < screen_h {
                let shift = rows.len().min(usize::from(screen_h - area.bottom()));
                if !area.is_empty() {
                    write!(self.backend, "\x1b[{};{}r", area.top() + 1, screen_h)?;
                    self.backend
                        .set_cursor_position(Position::new(0, area.top()))?;
                    for _ in 0..shift {
                        write!(self.backend, "\x1bM")?;
                    }
                    write!(self.backend, "\x1b[r")?;
                }
                self.draw_rows(area.top(), &rows[..shift])?;
                rows = &rows[shift..];
                area.y += shift as u16;
                self.set_viewport_area(area);
            }
            let top = area.top();
            if !rows.is_empty() && top > 1 {
                for chunk in rows.chunks(usize::from(top)) {
                    let n = chunk.len() as u16;
                    self.scroll_region_up(top, n)?;
                    self.draw_rows(top - n, chunk)?;
                }
                rows = &[];
            }
        }
        if !rows.is_empty() {
            self.insert_full_screen(rows)?;
        }
        Backend::flush(&mut self.backend)?;
        self.tail.extend(recent);
        let excess = self.tail.len().saturating_sub(keep);
        self.tail.drain(..excess);
        crate::composer::kitty::flush_transmits();
        Ok(())
    }

    fn insert_full_screen(&mut self, mut rows: &[&[Cell]]) -> io::Result<()> {
        let screen_h = self.last_known_screen_size.height;
        let mut area = self.viewport_area;
        self.clear_after_position(Position::new(0, area.top()))?;
        let mut y = area.top();
        while !rows.is_empty() {
            if y >= screen_h {
                let k = (rows.len() as u16).min(screen_h);
                self.scroll_screen_up(k)?;
                y = screen_h - k;
            }
            let k = rows.len().min(usize::from(screen_h - y));
            self.draw_rows(y, &rows[..k])?;
            y += k as u16;
            rows = &rows[k..];
        }
        let overflow = (y + area.height).saturating_sub(screen_h);
        self.scroll_screen_up(overflow)?;
        area.y = y - overflow;
        self.set_viewport_area(area);
        Ok(())
    }

    fn scroll_history_up(&mut self, top: u16, n: u16) -> io::Result<()> {
        if n == 0 || top == 0 {
            return Ok(());
        }
        if self.scrollback == Scrollback::Regions && top > 1 {
            return self.scroll_region_up(top, n);
        }

        self.clear_after_position(Position::new(0, top))?;
        self.scroll_screen_up(n)
    }

    fn scroll_region_up(&mut self, top: u16, n: u16) -> io::Result<()> {
        write!(self.backend, "\x1b[1;{top}r")?;
        self.backend
            .set_cursor_position(Position::new(0, top - 1))?;
        for _ in 0..n {
            write!(self.backend, "\r\n")?;
        }
        write!(self.backend, "\x1b[r")?;
        Ok(())
    }

    fn scroll_screen_up(&mut self, n: u16) -> io::Result<()> {
        if n == 0 {
            return Ok(());
        }
        let last_row = self.last_known_screen_size.height.saturating_sub(1);
        self.backend
            .set_cursor_position(Position::new(0, last_row))?;
        for _ in 0..n {
            write!(self.backend, "\r\n")?;
        }
        Ok(())
    }

    fn draw_rows(&mut self, y: u16, rows: &[&[Cell]]) -> io::Result<()> {
        let safe = crate::tui::unicode_safe();
        let mut cells = Vec::new();
        let mut owned: Vec<(u16, u16, Cell)> = Vec::new();
        for (dy, row) in rows.iter().enumerate() {
            let mut covered = 0usize;
            for (x, cell) in row.iter().enumerate() {
                if covered > 0 {
                    covered -= 1;
                    continue;
                }
                if safe {
                    covered = glyph_width(cell.symbol()).saturating_sub(1);
                    cells.push((x as u16, y + dy as u16, cell));
                } else {
                    let cell = ascii_cell(cell);
                    covered = glyph_width(cell.symbol()).saturating_sub(1);
                    owned.push((x as u16, y + dy as u16, cell));
                }
            }
        }
        if safe {
            self.backend.draw(cells.into_iter())
        } else {
            self.backend.draw(owned.iter().map(|(x, y, c)| (*x, *y, c)))
        }
    }
}

/// Clone a cell with its symbol transliterated to ASCII. Symbols that have
/// no decorative glyphs are cloned unchanged.
fn ascii_cell(cell: &Cell) -> Cell {
    let mut cell = cell.clone();
    if let Some(symbol) = crate::tui::ascii_symbol(cell.symbol()) {
        cell.set_symbol(&symbol);
    }
    cell
}

fn glyph_width(symbol: &str) -> usize {
    let glyph = symbol
        .strip_prefix("\x1b]8;;")
        .and_then(|rest| rest.split_once('\x07'))
        .map(|(_, rest)| rest.split('\x1b').next().unwrap_or(rest))
        .unwrap_or(symbol);
    crate::text_width::display_width(glyph)
}

/// Wraps a [`Backend`] and downgrades every drawn cell's colors to the
/// terminal's detected level (`gray_markdown::detect_color_level`). The TUI
/// authors its chrome in 24-bit RGB; this is the single point where frames
/// are quantized to xterm-256 or ANSI-16 — or flattened to `Reset` under
/// `NO_COLOR`. Byte-level writes (scroll-region setup, kitty transmits)
/// pass through untouched.
pub(crate) struct LeveledBackend<B> {
    inner: B,
}

impl<B> LeveledBackend<B> {
    pub(crate) fn new(inner: B) -> Self {
        Self { inner }
    }
}

impl<B: Write> Write for LeveledBackend<B> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.inner)
    }
}

impl<B: Backend> Backend for LeveledBackend<B> {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        if gray_markdown::detect_color_level() == gray_markdown::ColorLevel::TrueColor {
            return self.inner.draw(content);
        }
        let owned: Vec<(u16, u16, Cell)> = content
            .map(|(x, y, cell)| (x, y, leveled_cell(cell)))
            .collect();
        self.inner.draw(owned.iter().map(|(x, y, c)| (*x, *y, c)))
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

/// Clone a cell with fg/bg/underline colors downgraded to the detected
/// terminal color level.
fn leveled_cell(cell: &Cell) -> Cell {
    let fg = gray_markdown::adapt_ratatui_color(cell.fg);
    let bg = gray_markdown::adapt_ratatui_color(cell.bg);
    let underline = gray_markdown::adapt_ratatui_color(cell.underline_color);
    let mut out = cell.clone();
    out.set_fg(fg);
    out.set_bg(bg);
    out.underline_color = underline;
    out
}
