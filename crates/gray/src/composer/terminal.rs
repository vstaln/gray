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

    #[allow(dead_code)]
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
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
    pub last_known_cursor_pos: Position,
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
            last_known_cursor_pos: cursor_pos,
        };
        term.set_viewport_area(viewport_area);
        Ok(term)
    }

    #[allow(dead_code)]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    #[allow(dead_code)]
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    #[allow(dead_code)]
    pub fn current_buffer(&self) -> &Buffer {
        &self.buffers[self.current]
    }

    #[allow(dead_code)]
    pub fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    #[allow(dead_code)]
    pub fn previous_buffer(&self) -> &Buffer {
        &self.buffers[1 - self.current]
    }

    #[allow(dead_code)]
    pub fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
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
        self.previous_buffer_mut().reset();
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
        self.last_known_cursor_pos = position;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.backend.get_cursor_position()
    }

    pub fn swap_buffers(&mut self) {
        self.buffers[1 - self.current].reset();
        self.current = 1 - self.current;
    }

    pub fn flush(&mut self) -> io::Result<()> {
        let previous_buffer = &self.buffers[1 - self.current];
        let current_buffer = &self.buffers[self.current];
        let updates = previous_buffer.diff(current_buffer);
        if let Some((col, row, _)) = updates.last() {
            self.last_known_cursor_pos = Position { x: *col, y: *row };
        }
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
