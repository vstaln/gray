use std::collections::VecDeque;
use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType};
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
    vacated_above: u16,
    covered: u16,
    /// Rows at the screen top that are not history of their own: copies of
    /// rows already in scrollback, or blanks, slid in when the transcript was
    /// pulled down over vacated rows. They are deleted, never scrolled, so
    /// scrollback does not get them twice.
    ghost_top: u16,
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
            vacated_above: 0,
            covered: 0,
            ghost_top: 0,
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
            self.ghost_top = 0;
            self.tail.clear();
        }
        self.last_known_screen_size = screen_size;
        let old = self.viewport_area;
        let mut area = old;
        area.height = height.min(screen_size.height);
        area.width = screen_size.width;
        let bottom = screen_size.height.saturating_sub(area.height);
        // A band pinned to the screen bottom stays pinned: a shrink (the
        // palette closing, live rows collapsing) drops the band back down,
        // repaints the transcript rows an overlay was covering, and leaves
        // any other vacated rows above it for insert_before to reclaim; a
        // growth rises into those vacated rows first, lets the overlay
        // cover transcript rows it can repaint later, and scrolls history
        // only for what is left over. A floating band — the intro state,
        // or a composer that never reached the bottom — keeps its top row
        // and vacates below instead.
        if old.bottom() >= screen_size.height {
            area.y = bottom;
        } else {
            self.vacated_above = 0;
            area.y = old.y.min(bottom);
        }
        let rise = old.y.saturating_sub(area.y);
        let consumed = self.vacated_above.min(rise);
        self.vacated_above -= consumed;
        let rest = rise - consumed;
        let history_end = old.y - consumed + self.covered;
        let repaintable = u16::try_from(self.tail.len())
            .unwrap_or(u16::MAX)
            .min(history_end);
        let hide = rest
            .min(overlay.saturating_sub(self.covered))
            .min(repaintable.saturating_sub(self.covered));
        self.covered += hide;
        if rest > hide && !screen_shrank {
            let ghosts = self.delete_ghost_rows(old.y, rest - hide)?;
            self.scroll_history_up(old.y, rest - hide - ghosts)?;
        }
        let drop = area.y.saturating_sub(old.y);
        let uncover = self.covered.min(drop);
        self.vacated_above += drop - uncover;

        if area != self.viewport_area {
            let from = if self.viewport_area.is_empty() {
                area.y
            } else {
                area.y.min(self.viewport_area.y)
            };
            self.set_viewport_area(area);
            self.clear_after_position(Position::new(0, from))?;
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
        self.pull_history_down()
    }

    /// Close the vacated rows a grounded shrink left between the transcript
    /// and the band: slide the transcript down onto the band and refill the
    /// rows that open at the screen top from the tail. Those rows are already
    /// in scrollback (or unknown, and left blank), so they become ghost rows.
    fn pull_history_down(&mut self) -> io::Result<()> {
        let top = self.viewport_area.top();
        let n = self.vacated_above;
        if self.scrollback != Scrollback::Regions || n == 0 || self.covered > 0 || top < 2 {
            return Ok(());
        }
        write!(self.backend, "\x1b[1;{top}r")?;
        self.backend.set_cursor_position(Position::new(0, 0))?;
        write!(self.backend, "\x1b[{n}L\x1b[r")?;
        let visible = usize::from(top - n);
        let above = self.tail.len().saturating_sub(visible).min(usize::from(n));
        if self.tail.len() >= visible && above > 0 {
            let end = self.tail.len() - visible;
            let rows: Vec<Vec<Cell>> = self.tail.range(end - above..end).cloned().collect();
            let rows: Vec<&[Cell]> = rows.iter().map(Vec::as_slice).collect();
            self.draw_rows(n - above as u16, &rows)?;
        }
        self.vacated_above = 0;
        self.ghost_top = (self.ghost_top + n).min(top);
        Ok(())
    }

    /// Delete up to `n` ghost rows from the top of the history region above
    /// `top`, shifting the rest up without feeding scrollback. Returns how
    /// many were deleted.
    fn delete_ghost_rows(&mut self, top: u16, n: u16) -> io::Result<u16> {
        let g = self.ghost_top.min(n).min(top);
        if g == 0 || self.scrollback != Scrollback::Regions || top < 2 {
            return Ok(0);
        }
        write!(self.backend, "\x1b[1;{top}r")?;
        self.backend.set_cursor_position(Position::new(0, 0))?;
        write!(self.backend, "\x1b[{g}M\x1b[r")?;
        self.ghost_top -= g;
        Ok(g)
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
        Backend::flush(&mut self.backend)?;
        self.last_known_screen_size = screen_size;
        self.vacated_above = 0;
        self.covered = 0;
        self.ghost_top = 0;
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
        let mut frame = self.get_frame();
        render_callback(&mut frame);

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

        // Rows a shrinking band vacated above itself get refilled before
        // anything scrolls: the transcript tail continues there.
        let reclaim = rows.len().min(usize::from(self.vacated_above));
        if reclaim > 0 {
            let y = self.viewport_area.top().saturating_sub(self.vacated_above);
            self.draw_rows(y, &rows[..reclaim])?;
            self.vacated_above -= reclaim as u16;
            rows = &rows[reclaim..];
        }

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
            let ghosts = usize::from(
                self.delete_ghost_rows(top, rows.len().min(usize::from(u16::MAX)) as u16)?,
            );
            if ghosts > 0 {
                self.draw_rows(top - ghosts as u16, &rows[..ghosts])?;
                rows = &rows[ghosts..];
            }
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
        crate::quality_of_life::kitty::flush_transmits();
        Ok(())
    }

    fn insert_full_screen(&mut self, mut rows: &[&[Cell]]) -> io::Result<()> {
        self.ghost_top = 0;
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

#[path = "terminal_tests.rs"]
#[cfg(all(test, not(windows)))]
mod tests;
