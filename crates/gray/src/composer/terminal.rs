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
    /// The band was scrolled/erased by `insert_before` and the next frame has
    /// to repaint it from scratch. See `insert_before`.
    band_dirty: bool,
    /// Rows directly above the band that hold no transcript row: the ones it
    /// vacated sliding down to the bottom, minus any it has since grown back
    /// into. Growth spends these before scrolling, so opening the slash popup
    /// on a short transcript doesn't push the banner up — and, since closing
    /// it never scrolls back, off the screen one open/close at a time. The
    /// next scrollback insert draws into them, so a shrink never leaves a
    /// permanent blank gap (the doubled margin).
    blank_above: u16,
    /// Rows at the top of the band that are only the blank seam above the status
    /// dock (refreshed by `draw` every frame). A scrollback commit that ends in
    /// a blank row *is* that gap, so it overwrites the seam instead of stacking
    /// a second blank row on top of it (the doubled margin above `⬡ Working…`).
    top_slack: u16,
    /// Rows the band is already known to give up in the next frame: a live
    /// tool card whose result is being committed to scrollback. Rows inserted
    /// before that frame land on them rather than scrolling the screen and
    /// leaving blank rows between the committed card and the band afterwards.
    shrink_hint: u16,
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
            band_dirty: false,
            blank_above: 0,
            top_slack: 0,
            shrink_hint: 0,
        };
        term.set_viewport_area(viewport_area);
        Ok(term)
    }

    pub fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport_area = area;
    }

    /// Rows at the top of the band that are only the blank seam above the
    /// status dock. `draw` refreshes this every frame; see `top_slack`.
    pub fn set_top_slack(&mut self, rows: u16) {
        self.top_slack = rows;
    }

    /// Declares rows the band is about to give up (see `shrink_hint`). The next
    /// `set_viewport_height` clears whatever is left, so a hint never outlives
    /// its frame.
    pub fn hint_shrink(&mut self, rows: u16) {
        self.shrink_hint = self.shrink_hint.saturating_add(rows);
    }

    /// How many of the band's top rows an insert of `height` rows overwrites
    /// rather than pushes down: the declared shrink, plus the dock seam when
    /// the insert ends in a blank row (that row *is* the gap the seam held).
    fn take_band_rows(&mut self, height: u16, tail_blank: bool) -> u16 {
        let slack = if tail_blank { self.top_slack } else { 0 };
        let cap = self.viewport_area.height.saturating_sub(1);
        let eaten = self.shrink_hint.saturating_add(slack).min(height).min(cap);
        let from_hint = eaten.min(self.shrink_hint);
        self.shrink_hint -= from_hint;
        self.top_slack = self.top_slack.saturating_sub(eaten - from_hint);
        eaten
    }

    pub fn set_viewport_height(&mut self, height: u16, screen_size: Size) -> io::Result<()> {
        let mut area = self.viewport_area;
        area.height = height.min(screen_size.height);
        area.width = screen_size.width;
        // Whatever the inserts did not eat is settled by this frame.
        self.shrink_hint = 0;

        // Grow up into known-blank rows first: they hold no transcript, so
        // taking them moves nothing else on screen.
        if area.bottom() > screen_size.height {
            let rise = (area.bottom() - screen_size.height)
                .min(self.blank_above)
                .min(area.y);
            area.y -= rise;
            self.blank_above -= rise;
        }

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
            self.blank_above = self
                .blank_above
                .saturating_add(pinned_y.saturating_sub(vacated_top));
            area.y = pinned_y;
            self.set_viewport_area(area);
            // Clear from the vacated top (also resets the previous buffer,
            // so the next frame repaints the whole band at its new row).
            self.clear_after_position(Position::new(0, vacated_top))?;
            return Ok(());
        }

        if area != self.viewport_area {
            if !scrolled {
                // A band that grew into blank rows starts above its old top;
                // clear from whichever top is higher.
                let clear_pos = if self.viewport_area.is_empty() || area.y < self.viewport_area.y {
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
        // A clear that starts above the vacated rows wipes them too (resize
        // reflow, full-screen clear): they are no longer a gap to fill.
        if position.y < self.viewport_area.y.saturating_sub(self.blank_above) {
            self.blank_above = 0;
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
        // Erase + repaint the band inside the caller's synchronized-update
        // bracket: a scroll in `insert_before` moves the band's pixels with
        // the rest of the screen, so the rows it now covers are stale. Doing
        // the erase here (not in `insert_before`, which runs outside the
        // bracket) keeps "band gone" from ever reaching the screen on its own
        // — that was the input box flashing on every streamed row and tool
        // event.
        if self.band_dirty {
            self.band_dirty = false;
            self.clear()?;
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

        // Rows of the band this insert overwrites instead of pushing down: the
        // seam above the status dock when the commit ends blank (the commit IS
        // that gap), and the rows of a live card that is being committed. The
        // band then comes out the right height with nothing left blank above it.
        let tail_blank = buffer_last_row_is_blank(&buffer);
        let eaten = self.take_band_rows(height, tail_blank);
        let band_height = self.viewport_area.height.saturating_sub(eaten);

        // Start at the end of the transcript, not at the band's top: the
        // vacated rows above it are the first rows to fill.
        let top: i32 = self.viewport_area.top().into();
        let mut drawn_height: i32 = top - i32::from(self.blank_above).min(top);
        let mut buffer_height: i32 = height.into();
        let viewport_height: i32 = band_height.into();
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
            height: band_height,
            ..self.viewport_area
        });

        // The scroll moved the band with everything else; the next frame
        // erases and repaints it (see `band_dirty`).
        self.band_dirty = true;
        // The rows just drawn sit flush against the band, filling the
        // vacated rows: none are left blank above it.
        self.blank_above = 0;

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

/// True when the last row of an insert buffer is a bare blank: only spaces on
/// the terminal's default background, the same predicate the transcript uses
/// (`transcript_row_is_blank`) for "this block ends with a gap".
fn buffer_last_row_is_blank(buffer: &Buffer) -> bool {
    let area = buffer.area;
    if area.height == 0 {
        return false;
    }
    let y = area.bottom() - 1;
    (area.x..area.right()).all(|x| {
        let cell = &buffer[(x, y)];
        cell.symbol().trim().is_empty() && cell.bg == ratatui::style::Color::Reset
    })
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

    /// The reported flicker: `insert_before` scrolls the whole screen, so the
    /// band's pixels move with it. It used to erase the band right there —
    /// outside the frame's synchronized-update bracket — so a terminal could
    /// present one frame with no input box at all, on every streamed row and
    /// every tool event. The erase belongs to the next frame, which repaints
    /// the band inside the same bracket as the rest of the frame.
    #[test]
    fn the_band_erase_waits_for_the_next_frame() {
        let mut terminal = CustomTerminal::with_options(TestBackend::new(40, 20), 4).unwrap();
        let screen = Size::new(40, 20);
        terminal.set_viewport_height(4, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 16, 40, 4));
        // Stand in for a painted band: a real frame writes the whole band
        // through the terminal, so the backend ends up holding its rows.
        let paint = |terminal: &mut CustomTerminal<TestBackend>, mark: &str| {
            let band = terminal.viewport_area;
            terminal
                .draw(|frame| {
                    for y in band.y..band.bottom() {
                        for x in 0..40 {
                            frame.buffer[(x, y)].set_symbol(mark);
                        }
                    }
                })
                .unwrap();
        };
        paint(&mut terminal, "b");
        terminal
            .insert_before(1, |buf| {
                buf[(0, 0)].set_symbol("s");
            })
            .unwrap();
        // The insert moved the band, but nothing erased the old paint: the
        // rows are still painted, and the next frame knows it owes a full
        // repaint.
        let buffer = terminal.backend.buffer();
        assert!(
            (16..20).any(|y| buffer[(0, y)].symbol() != " "),
            "insert_before must not blank the band on its own"
        );
        assert!(terminal.band_dirty, "the frame must repaint the band");
        // The frame erases and repaints it: the vacated paint is gone, not
        // left stale where the band used to be.
        paint(&mut terminal, "x");
        assert!(!terminal.band_dirty);
        let band = terminal.viewport_area;
        let buffer = terminal.backend.buffer();
        assert!(
            (band.y..band.bottom()).all(|y| (0..40).all(|x| buffer[(x, y)].symbol() == "x")),
            "the band must be repainted from scratch, not left stale"
        );
        assert!(
            (band.bottom()..20).all(|y| (0..40).all(|x| buffer[(x, y)].symbol() == " ")),
            "the frame must erase the rows the band vacated"
        );
    }

    /// The reported bug: on a short transcript the slash popup grew the band
    /// by scrolling the whole screen, and closing it only slid the band back
    /// down — each open/close carried the banner further up until it left
    /// the screen. Growth now takes the blank rows above the band first.
    #[test]
    fn popup_open_close_leaves_the_banner_in_place() {
        let mut terminal = CustomTerminal::with_options(TestBackend::new(40, 20), 4).unwrap();
        let screen = Size::new(40, 20);
        terminal
            .insert_before(3, |buf| {
                buf[(0, 1)].set_symbol("B");
            })
            .unwrap();
        // First frame pins the band below the banner, leaving a blank gap.
        terminal.set_viewport_height(4, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 16, 40, 4));
        for _ in 0..3 {
            terminal.set_viewport_height(11, screen).unwrap();
            assert_eq!(terminal.viewport_area, Rect::new(0, 9, 40, 11));
            terminal.set_viewport_height(4, screen).unwrap();
            assert_eq!(terminal.viewport_area, Rect::new(0, 16, 40, 4));
            let buffer = terminal.backend.buffer();
            assert_eq!(buffer[(0, 1)].symbol(), "B", "the banner must not move");
        }
    }

    /// Growth beyond the blank rows still scrolls, by only the shortfall.
    #[test]
    fn growth_past_the_blank_rows_scrolls_only_the_rest() {
        let mut terminal = CustomTerminal::with_options(TestBackend::new(40, 20), 4).unwrap();
        let screen = Size::new(40, 20);
        terminal
            .insert_before(12, |buf| {
                buf[(0, 3)].set_symbol("B");
            })
            .unwrap();
        terminal.set_viewport_height(4, screen).unwrap();
        // 12 transcript rows + 4 blank + a 4-row band.
        assert_eq!(terminal.viewport_area, Rect::new(0, 16, 40, 4));
        terminal.set_viewport_height(10, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 10, 40, 10));
        // 4 blank rows spent, 2 scrolled: the banner rose by 2, not 6.
        let buffer = terminal.backend.buffer();
        assert_eq!(buffer[(0, 1)].symbol(), "B");
    }

    /// A shrink while pinned leaves blank rows above the band. The next
    /// scrollback rows must land in them, not below them.
    #[test]
    fn rows_inserted_after_a_shrink_fill_the_vacated_rows() {
        use ratatui::style::Style;

        let screen = Size::new(10, 12);
        let mut terminal = CustomTerminal::with_options(TestBackend::new(10, 12), 4).unwrap();
        terminal.set_viewport_height(6, screen).unwrap();
        terminal.set_viewport_height(4, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 8, 10, 4));

        for glyph in ["x", "y"] {
            terminal
                .insert_before(1, |buf| {
                    buf.set_string(0, 0, glyph, Style::default());
                })
                .unwrap();
        }

        // Back-to-back rows are adjacent: no blank row between them, and
        // the first one sits where the transcript ended (row 0), not at
        // the shrunken band's old top (row 8).
        let buffer = terminal.backend.buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "x");
        assert_eq!(buffer[(0, 1)].symbol(), "y");
    }

    /// The reported bug: a blank row committed while the status dock's seam
    /// row is on screen stacked on top of the seam, and the band's shrink
    /// then left a second vacated blank row above `⬡ Thinking…` until the
    /// next insert. The commit now overwrites the seam: one gap, no strays.
    #[test]
    fn a_blank_commit_overwrites_the_dock_seam_instead_of_doubling_it() {
        let screen = Size::new(10, 12);
        let mut terminal = CustomTerminal::with_options(TestBackend::new(10, 12), 6).unwrap();
        terminal.insert_before(6, |_| {}).unwrap();
        terminal.set_viewport_height(6, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 6, 10, 6));

        terminal.set_top_slack(1);
        terminal.insert_before(1, |_| {}).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 7, 10, 5));
        terminal.set_viewport_height(5, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 7, 10, 5));
        assert_eq!(terminal.blank_above, 0, "no vacated row above the band");
    }

    /// A commit that ends in content keeps the seam: only a blank tail *is*
    /// the gap.
    #[test]
    fn a_content_commit_does_not_eat_the_dock_seam() {
        use ratatui::style::Style;

        let screen = Size::new(10, 12);
        let mut terminal = CustomTerminal::with_options(TestBackend::new(10, 12), 6).unwrap();
        terminal.insert_before(6, |_| {}).unwrap();
        terminal.set_viewport_height(6, screen).unwrap();

        terminal.set_top_slack(1);
        terminal
            .insert_before(1, |buf| {
                buf.set_string(0, 0, "x", Style::default());
            })
            .unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 6, 10, 6));
    }

    /// Committing a live tool card: the card's rows leave the band in the
    /// same batch the committed rows arrive in. Hinted, the commit overwrites
    /// them; unhinted, the screen scrolls and the shrink strands blank rows
    /// between the committed card and the dock (extra margin under the card).
    #[test]
    fn a_hinted_card_commit_leaves_no_blank_rows_above_the_band() {
        use ratatui::style::Style;

        let screen = Size::new(10, 12);
        let commit = |terminal: &mut CustomTerminal<TestBackend>| {
            terminal
                .insert_before(3, |buf| {
                    buf.set_string(0, 2, "c", Style::default());
                })
                .unwrap();
            terminal.set_viewport_height(6, screen).unwrap();
        };
        let fresh = || {
            let mut terminal = CustomTerminal::with_options(TestBackend::new(10, 12), 8).unwrap();
            terminal.insert_before(4, |_| {}).unwrap();
            terminal.set_viewport_height(8, screen).unwrap();
            terminal
        };

        let mut stranded = fresh();
        commit(&mut stranded);
        assert_eq!(stranded.blank_above, 2, "unhinted: two stray blank rows");

        let mut hinted = fresh();
        hinted.hint_shrink(2);
        commit(&mut hinted);
        assert_eq!(hinted.viewport_area, Rect::new(0, 6, 10, 6));
        assert_eq!(hinted.blank_above, 0);
    }

    /// A hint is for one frame: whatever the inserts did not eat is dropped by
    /// the next `set_viewport_height`.
    #[test]
    fn a_shrink_hint_does_not_outlive_its_frame() {
        let screen = Size::new(10, 12);
        let mut terminal = CustomTerminal::with_options(TestBackend::new(10, 12), 8).unwrap();
        terminal.insert_before(4, |_| {}).unwrap();
        terminal.hint_shrink(2);
        terminal.set_viewport_height(8, screen).unwrap();
        assert_eq!(terminal.shrink_hint, 0);
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
