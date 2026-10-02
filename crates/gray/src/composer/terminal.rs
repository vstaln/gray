//! Inline terminal for the composer band, on codex's viewport model
//! (`codex-rs/tui/src/custom_terminal.rs`, `insert_history.rs`,
//! `tui.rs::draw`, `tui/scrollback.rs`).
//!
//! The band (status dock, live cards, input box, footer) is an inline
//! viewport that sits directly under the last transcript row, wherever that
//! row is: on a short session it is near the top of the screen, and it only
//! reaches the bottom once the transcript fills the rows above it. It is
//! never pinned to the bottom, so there is never a dead gap between the
//! transcript and the input box. Three rules move it:
//!
//! - **A height change keeps its top.** Growth past the screen bottom
//!   scrolls the transcript above it into scrollback by the overflow; a
//!   shrink clears the rows it gave up. The band repaints in full either
//!   way (`set_viewport_height`).
//! - **A history insert never touches the band's cells.** While there are
//!   free rows under it, the band is shifted down into them (reverse index
//!   inside a scroll region from its top to the screen bottom) and the new
//!   rows are drawn in the gap it left. Once it sits on the bottom, line
//!   feeds at the bottom margin of a scroll region ending just above it
//!   push the oldest rows into scrollback, and the new rows land in the
//!   freed rows (`insert_before`).
//! - **Nothing re-anchors it from a cursor probe.** An alternate-screen
//!   modal leaves the main screen as it was, so the band is cleared and
//!   repainted where it already is (`clear`); a resize rebuilds the whole
//!   transcript from the top (`Tui::reflow_on_resize`).
//!
//! Where a partial scroll region drops rows instead of moving them into
//! scrollback (Windows Terminal), and for a one-row history region (DECSTBM
//! needs two rows), codex's full-screen path runs instead: the band is
//! cleared, the rows are written from its top with whole-screen scrolling,
//! and the next frame repaints the band under them.

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

/// How rows reach scrollback on this terminal (codex `ScrollbackStrategy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scrollback {
    /// DEC scroll regions and line feeds: the band's cells never move
    /// under it, so an insert costs no repaint.
    Regions,
    /// Whole-screen scrolling. Windows Terminal drops the rows a partial
    /// scroll region scrolls out instead of keeping them in scrollback.
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
    /// An empty band on the cursor's row: the first frame gives it its
    /// height, and the first inserts push it down under the banner.
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

    /// An empty band at row `y` of a `screen_size` screen, no terminal
    /// query: a resize reflow knows the cursor is home, and tests drive a
    /// screen model that has no tty to probe.
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
        };
        term.set_viewport_area(viewport_area);
        term
    }

    pub fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport_area = area;
    }

    /// Gives the band `height` rows (codex `Tui::draw`). Its top stays put,
    /// so it keeps sitting under the last transcript row: growth past the
    /// screen bottom scrolls the transcript above it into scrollback by the
    /// overflow, a shrink leaves blank rows below the footer for the next
    /// inserts to shift it into. Any change clears from the band's top, so
    /// the frame repaints it whole and no stale row survives the move.
    pub fn set_viewport_height(&mut self, height: u16, screen_size: Size) -> io::Result<()> {
        // A shorter screen is about to be reflowed from the top
        // (`Tui::reflow_on_resize`); scrolling now would push rows into
        // scrollback that the reflow writes again.
        let screen_shrank = screen_size.height < self.last_known_screen_size.height;
        self.last_known_screen_size = screen_size;

        let mut area = self.viewport_area;
        area.height = height.min(screen_size.height);
        area.width = screen_size.width;
        area.y = area.y.min(screen_size.height.saturating_sub(area.height));
        let wanted_bottom = self.viewport_area.y.saturating_add(area.height);
        if wanted_bottom > screen_size.height && !screen_shrank {
            self.scroll_history_up(self.viewport_area.y, wanted_bottom - screen_size.height)?;
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
        Ok(())
    }

    pub fn clear_after_position(&mut self, position: Position) -> io::Result<()> {
        self.backend.set_cursor_position(position)?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        // The screen under the band is blank now: the next frame diffs
        // against blank and repaints every cell it draws.
        self.buffers[1 - self.current].reset();
        Ok(())
    }

    /// Erases the band and everything under it; the next frame repaints
    /// the band in place. This is the whole re-anchor after a modal: the
    /// alternate screen left the main screen and the band's row as they
    /// were, so a cursor probe could only get the position wrong.
    pub fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.clear_after_position(self.viewport_area.as_position())
    }

    /// Purges scrollback and the screen and puts an empty band on row 0
    /// (codex `clear_scrollback_and_visible_screen_ansi`): the start of a
    /// resize reflow, which writes the whole transcript back from the top.
    /// The cursor is known to be home, so nothing is probed.
    pub(crate) fn clear_scrollback_and_screen(&mut self, screen_size: Size) -> io::Result<()> {
        // Reset scroll region and style, home, clear screen, purge scrollback.
        write!(self.backend, "\x1b[r\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[H")?;
        Backend::flush(&mut self.backend)?;
        self.last_known_screen_size = screen_size;
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

    pub fn flush(&mut self) -> io::Result<()> {
        let previous_buffer = &self.buffers[1 - self.current];
        let current_buffer = &self.buffers[self.current];
        let updates = previous_buffer.diff(current_buffer);
        self.backend.draw(updates.into_iter())
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

    /// Writes `height` transcript rows, rendered by `draw_fn`, above the
    /// band (codex `insert_history_lines`). Free rows under the band are
    /// spent first by shifting it down into them; after that the rows above
    /// it scroll. Either way the band's own cells are moved by the terminal,
    /// never erased, so the frame after an insert has nothing to repaint.
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
        let all_rows: Vec<&[Cell]> = buffer.content.chunks(width as usize).collect();
        let mut rows = all_rows.as_slice();

        if self.scrollback == Scrollback::Regions {
            let screen_h = self.last_known_screen_size.height;
            let mut area = self.viewport_area;
            if area.bottom() < screen_h {
                let shift = rows.len().min(usize::from(screen_h - area.bottom()));
                if !area.is_empty() {
                    // Rows top..screen bottom slide down by `shift`: the band
                    // moves with its cells, and only blank rows fall off.
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
        Ok(())
    }

    /// Codex's full-screen insertion: clear the band so no stale composer
    /// row scrolls into scrollback, write the rows from its top, scrolling
    /// the whole screen as they run out of room, and leave the band its
    /// height under them. The cleared band repaints on the next frame.
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

    /// Scrolls the transcript above a band whose top is `top` up by `n`
    /// rows (codex `ScrollbackStrategy::grow_viewport`): the oldest `n` go
    /// into scrollback and the `n` rows just above the band come free.
    fn scroll_history_up(&mut self, top: u16, n: u16) -> io::Result<()> {
        if n == 0 || top == 0 {
            return Ok(());
        }
        if self.scrollback == Scrollback::Regions && top > 1 {
            return self.scroll_region_up(top, n);
        }
        // The band rides the whole-screen scroll: blank it first, so its
        // rows move up as blanks and none of them lands in scrollback.
        self.clear_after_position(Position::new(0, top))?;
        self.scroll_screen_up(n)
    }

    /// Line feeds at the bottom margin of the scroll region `0..top`.
    /// `CSI S` would be shorter, but some terminals (codex names
    /// QTermWidget and xterm.js) drop the departing rows instead of
    /// keeping them in scrollback; a line feed never does. DECSTBM needs
    /// two rows, so `top` is at least 2.
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

    /// Draws whole rows onto blank screen rows starting at `y`. A wide
    /// glyph covers the cell after it, so that cell is not printed: it
    /// would land one column late and push the rest of the row right.
    fn draw_rows(&mut self, y: u16, rows: &[&[Cell]]) -> io::Result<()> {
        let mut cells = Vec::new();
        for (dy, row) in rows.iter().enumerate() {
            let mut covered = 0usize;
            for (x, cell) in row.iter().enumerate() {
                if covered > 0 {
                    covered -= 1;
                    continue;
                }
                covered = glyph_width(cell.symbol()).saturating_sub(1);
                cells.push((x as u16, y + dy as u16, cell));
            }
        }
        self.backend.draw(cells.into_iter())
    }
}

/// Columns a cell's glyph takes. A hyperlinked cell carries its OSC 8
/// wrapper in the symbol (`push_styled_lines_with_hyperlinks`), and only
/// the glyph between the escapes is visible.
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
