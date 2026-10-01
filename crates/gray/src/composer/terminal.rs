//! Custom inline terminal implementation for composer.
//!
//! Replaces `ratatui::Terminal` with in-place viewport mutation (`set_viewport_area` /
//! `set_viewport_height`) matching OpenAI Codex (`codex-rs/tui/src/custom_terminal.rs`).
//!
//! Unlike `ratatui::Terminal`, dynamic resizing does NOT destroy and recreate the terminal,
//! and does NOT re-probe the cursor via CPR (`\x1b[6n`) on height changes. This eliminates
//! prompt doubling and cursor drift down the terminal screen.

use std::io;

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

pub struct CustomTerminal<B>
where
    B: Backend,
{
    backend: B,
    buffers: [Buffer; 2],
    current: usize,
    pub hidden_cursor: bool,
    pub viewport_area: Rect,
    pub last_known_screen_size: Size,
}

impl<B> Drop for CustomTerminal<B>
where
    B: Backend,
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
    B: Backend,
{
    pub fn with_options(mut backend: B, height: u16) -> io::Result<Self> {
        let screen_size = backend.size()?;
        let cursor_pos = backend
            .get_cursor_position()
            .unwrap_or(Position { x: 0, y: 0 });
        let height = height.min(screen_size.height);

        let mut y = cursor_pos.y;
        if y + height > screen_size.height {
            let overflow = (y + height) - screen_size.height;
            backend.set_cursor_position(Position::new(0, screen_size.height.saturating_sub(1)))?;
            backend.append_lines(overflow)?;
            y = screen_size.height.saturating_sub(height);
        }

        let viewport_area = Rect::new(0, y, screen_size.width, height);
        let mut term = Self {
            backend,
            buffers: [Buffer::empty(Rect::ZERO), Buffer::empty(Rect::ZERO)],
            current: 0,
            hidden_cursor: false,
            viewport_area,
            last_known_screen_size: screen_size,
        };
        term.set_viewport_area(viewport_area);
        Ok(term)
    }

    pub fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport_area = area;
    }

    pub fn set_viewport_height(&mut self, height: u16, screen_size: Size) -> io::Result<()> {
        let mut area = self.viewport_area;
        area.height = height.min(screen_size.height);
        area.width = screen_size.width;

        let mut scrolled = false;
        // If the viewport has expanded beyond the screen height, scroll everything else up to make room.
        if area.bottom() > screen_size.height {
            let scroll_by = area.bottom() - screen_size.height;
            // Clear the stale composer before scrolling
            self.clear_after_position(Position::new(0, area.top()))?;
            self.backend
                .set_cursor_position(Position::new(0, screen_size.height.saturating_sub(1)))?;
            self.backend.append_lines(scroll_by)?;
            Backend::flush(&mut self.backend)?;
            area.y = anchored_viewport_y(area.height, screen_size.height);
            scrolled = true;
        }
        // The composer band always rides the screen's last rows, from the
        // first frame (a fresh or cleared session included): a viewport that
        // is not flush with the bottom slides down to it. The rows it
        // vacates are cleared back to plain terminal rows (the next inserted
        // transcript row lands there), so nothing stale stays painted and no
        // dead band is left below the footer.
        let pinned_y = anchored_viewport_y(area.height, screen_size.height);
        if pinned_y != area.y {
            let vacated_top = self.viewport_area.y;
            area.y = pinned_y;
            self.set_viewport_area(area);
            // Clear from the vacated top (also resets the previous buffer,
            // so the next frame repaints the whole band at its new row).
            self.clear_after_position(Position::new(0, vacated_top))?;
            return Ok(());
        }

        if area != self.viewport_area {
            if !scrolled {
                let clear_pos = if self.viewport_area.is_empty() {
                    area.as_position()
                } else {
                    self.viewport_area.as_position()
                };
                self.clear_after_position(clear_pos)?;
            }
            self.set_viewport_area(area);
        }

        Ok(())
    }

    pub fn clear_after_position(&mut self, position: Position) -> io::Result<()> {
        self.backend.set_cursor_position(position)?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        self.buffers[1 - self.current].reset();
        Ok(())
    }

    pub fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.clear_after_position(self.viewport_area.as_position())
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
        let screen_size = self.size()?;
        if screen_size != self.last_known_screen_size {
            self.last_known_screen_size = screen_size;
        }
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

    pub fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }

    pub fn insert_before<F>(&mut self, height: u16, draw_fn: F) -> io::Result<()>
    where
        F: FnOnce(&mut Buffer),
    {
        if height == 0 {
            return Ok(());
        }

        let width = self.viewport_area.width.max(1);
        let area = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        let mut buffer = Buffer::empty(area);
        draw_fn(&mut buffer);
        let mut buffer_content = buffer.content.as_slice();

        let mut drawn_height: i32 = self.viewport_area.top().into();
        let mut buffer_height: i32 = height.into();
        let viewport_height: i32 = self.viewport_area.height.into();
        let screen_height: i32 = self.last_known_screen_size.height.into();

        while buffer_height + viewport_height > screen_height {
            let to_draw = buffer_height.min(screen_height);
            let scroll_up = 0.max(drawn_height + to_draw - screen_height);
            self.scroll_up(scroll_up as u16)?;
            buffer_content = self.draw_lines(
                (drawn_height - scroll_up) as u16,
                to_draw as u16,
                buffer_content,
            )?;
            drawn_height += to_draw - scroll_up;
            buffer_height -= to_draw;
        }

        let scroll_up = 0.max(drawn_height + buffer_height + viewport_height - screen_height);
        self.scroll_up(scroll_up as u16)?;
        self.draw_lines(
            (drawn_height - scroll_up) as u16,
            buffer_height as u16,
            buffer_content,
        )?;
        drawn_height += buffer_height - scroll_up;

        self.set_viewport_area(Rect {
            y: drawn_height as u16,
            ..self.viewport_area
        });

        self.clear()?;

        Ok(())
    }

    fn scroll_up(&mut self, lines_to_scroll: u16) -> io::Result<()> {
        if lines_to_scroll > 0 {
            self.backend.set_cursor_position(Position::new(
                0,
                self.last_known_screen_size.height.saturating_sub(1),
            ))?;
            self.backend.append_lines(lines_to_scroll)?;
        }
        Ok(())
    }

    fn draw_lines<'a>(
        &mut self,
        y_offset: u16,
        lines_to_draw: u16,
        cells: &'a [Cell],
    ) -> io::Result<&'a [Cell]> {
        let width: usize = self.viewport_area.width.max(1).into();
        let (to_draw, remainder) = cells.split_at(width * lines_to_draw as usize);
        if lines_to_draw > 0 {
            let iter = to_draw
                .iter()
                .enumerate()
                .map(|(i, c)| ((i % width) as u16, y_offset + (i / width) as u16, c));
            self.backend.draw(iter)?;
            Backend::flush(&mut self.backend)?;
        }
        Ok(remainder)
    }
}

/// Top row of a `height`-row composer band pinned to the bottom of a
/// `screen_h`-row screen: its last row is the screen's last row. A band
/// taller than the screen sits at row 0. Pure for testability.
fn anchored_viewport_y(height: u16, screen_h: u16) -> u16 {
    screen_h.saturating_sub(height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    /// A turn's dock and live cards fill the screen; when they clear, the
    /// input box and the footer must come back down to the screen's last row
    /// instead of parking above a dead band of cleared rows.
    #[test]
    fn shrinking_a_bottom_anchored_viewport_returns_to_the_last_row() {
        let mut terminal = CustomTerminal::with_options(TestBackend::new(40, 20), 6).unwrap();
        let screen = Size::new(40, 20);

        // Turn in flight: the dock + live cards fill the screen, so the
        // composer's last row is the screen's last row.
        terminal.set_viewport_height(20, screen).unwrap();
        assert_eq!(terminal.viewport_area.bottom(), 20);

        // Tool result: the dock and live cards clear (20 rows -> 6).
        terminal.set_viewport_height(6, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 14, 40, 6));
        // The rows it gave up are blank rows again, not stale composer text.
        // The test module is a child of this one, so the backend field
        // is directly readable.
        let buffer = terminal.backend.buffer();
        assert!((0..14).all(|y| (0..40).all(|x| buffer[(x, y)].symbol() == " ")));
    }

    /// Fresh session on a tall screen: the banner leaves the cursor high, and
    /// the first frame must already put the band on the screen's last rows.
    #[test]
    fn a_fresh_session_pins_to_the_last_rows_from_the_first_frame() {
        let mut backend = TestBackend::new(40, 20);
        backend.set_cursor_position(Position::new(0, 7)).unwrap();
        let mut terminal = CustomTerminal::with_options(backend, 4).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 7, 40, 4));
        let screen = Size::new(40, 20);
        terminal.set_viewport_height(4, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 16, 40, 4));
        assert_eq!(terminal.viewport_area.bottom(), 20);
    }

    /// Growth and shrink both keep the band's last row on the screen's last
    /// row, on a session that never filled the screen.
    #[test]
    fn growth_and_shrink_keep_the_band_on_the_bottom() {
        let mut terminal = CustomTerminal::with_options(TestBackend::new(40, 20), 4).unwrap();
        let screen = Size::new(40, 20);
        terminal.set_viewport_height(9, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 11, 40, 9));
        terminal.set_viewport_height(4, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 16, 40, 4));
    }

    /// A taller screen after a resize re-pins the band to the new last row.
    #[test]
    fn a_taller_screen_repins_the_band() {
        let mut terminal = CustomTerminal::with_options(TestBackend::new(40, 20), 4).unwrap();
        terminal.set_viewport_height(4, Size::new(40, 20)).unwrap();
        terminal.set_viewport_height(4, Size::new(40, 30)).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 26, 40, 4));
    }

    #[test]
    fn anchored_y_covers_fresh_growth_shrink_cap_and_tiny_screens() {
        assert_eq!(anchored_viewport_y(5, 50), 45); // fresh session
        assert_eq!(anchored_viewport_y(7, 50), 43); // growth
        assert_eq!(anchored_viewport_y(5, 50), 45); // shrink
        assert_eq!(anchored_viewport_y(50, 50), 0); // full screen
        assert_eq!(anchored_viewport_y(60, 50), 0); // taller than the screen
        assert_eq!(anchored_viewport_y(4, 1), 0); // tiny screen
        assert_eq!(anchored_viewport_y(4, 0), 0); // no screen
    }
}
