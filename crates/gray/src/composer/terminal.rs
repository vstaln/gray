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
use ratatui::style::Style;
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
    /// Latched once the composer's last row has reached the screen's last
    /// row (the transcript overflowed the screen). While latched the
    /// composer's fixed rows — input box, footer — stay glued to the
    /// screen's bottom: a later resize slides the viewport instead of
    /// parking the footer above dead terminal rows.
    bottom_anchored: bool,
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
            bottom_anchored: viewport_area.bottom() >= screen_size.height,
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
            area.y = screen_size.height - area.height;
            scrolled = true;
        }
        if area.bottom() >= screen_size.height {
            // Reached (or overran) the screen's last row: the inline
            // composer now lives on the screen's bottom rows.
            self.bottom_anchored = true;
        } else if area.bottom() > self.viewport_area.bottom() {
            // Growing without reaching the last row: the transcript no
            // longer fills the screen (a new session, a cleared one), so the
            // composer hugs the conversation again. Without this a stale
            // latch would later jump the composer to the screen's bottom.
            self.bottom_anchored = false;
        } else if self.bottom_anchored {
            // Shrinking a bottom-anchored composer — the dock and live cards
            // clearing at a tool result, turn end — must not slide the input
            // box up the screen and park the footer above dead terminal
            // rows. Slide the viewport down to the screen's last row
            // instead, and repaint the rows it vacates (the cleared
            // dock/card rows) as scrollback rows so no stale text and no
            // stripe of the terminal's default background survives there.
            // The transcript above keeps its rows: nothing scrolls.
            let vacated = self.viewport_area.bottom() - area.bottom();
            let vacated_top = self.viewport_area.y;
            area.y = screen_size.height - area.height;
            self.set_viewport_area(area);
            self.paint_blank_rows(vacated_top, vacated, screen_size.width)?;
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

    /// Paints `height` blank rows at `y`, sized to the viewport width, with
    /// the composer's surface background — the scrollback look, so rows a
    /// shrunken viewport vacated read as transcript space rather than a
    /// stripe of the terminal's default background.
    fn paint_blank_rows(&mut self, y: u16, height: u16, width: u16) -> io::Result<()> {
        if height == 0 || width == 0 {
            return Ok(());
        }
        let count = (width as usize) * (height as usize);
        let mut cell = Cell::default();
        cell.set_style(Style::default().bg(crate::theme::theme().surface_bg));
        let rows: Vec<Cell> = vec![cell; count];
        self.backend.draw(rows.iter().enumerate().map(|(i, c)| {
            (
                (i % width as usize) as u16,
                y + (i / width as usize) as u16,
                c,
            )
        }))?;
        Backend::flush(&mut self.backend)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn backend(&self) -> &B {
        &self.backend
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    /// A tool call grows the composer (status dock + live cards) until the
    /// viewport's last row is the screen's last row; the tool result then
    /// clears those rows. The composer's fixed rows — input box, footer —
    /// have to stay on the screen's last rows: sliding them up parked the
    /// footer above dead terminal rows (an empty stripe at the bottom of
    /// the screen) and made the text area jump on every tool call.
    #[test]
    fn shrinking_a_bottom_anchored_viewport_keeps_the_composer_on_the_last_row() {
        let mut terminal = CustomTerminal::with_options(TestBackend::new(40, 20), 6).unwrap();
        let screen = Size::new(40, 20);

        // Turn in flight: the dock + live cards fill the screen, so the
        // composer's last row is the screen's last row.
        terminal.set_viewport_height(20, screen).unwrap();
        assert_eq!(terminal.viewport_area.bottom(), 20);

        // Tool result: the dock and live cards clear (20 rows -> 6).
        terminal.set_viewport_height(6, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 14, 40, 6));

        // The vacated 14 rows carry the composer's band background rather
        // than the terminal's default (which showed as a stripe).
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].bg, crate::theme::theme().surface_bg);
        assert_eq!(buffer[(39, 13)].bg, crate::theme::theme().surface_bg);
    }

    /// A composer that never reached the screen's last row (a transcript
    /// shorter than the screen) keeps hugging the conversation instead of
    /// jumping to the screen's bottom.
    #[test]
    fn an_unanchored_viewport_never_slides_to_the_bottom() {
        let mut terminal = CustomTerminal::with_options(TestBackend::new(40, 20), 4).unwrap();
        let screen = Size::new(40, 20);
        terminal.set_viewport_height(9, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 0, 40, 9));
        terminal.set_viewport_height(4, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 0, 40, 4));
    }
}
