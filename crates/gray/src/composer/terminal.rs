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
    /// The height the band last asked for. On a full screen a shrink is
    /// held (see `held_height`): the band keeps its rows and the surplus
    /// is spent by the next scrollback inserts instead.
    wanted_height: u16,
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
            wanted_height: height,
        };
        term.set_viewport_area(viewport_area);
        Ok(term)
    }

    pub fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport_area = area;
    }

    /// The height to give the band when it asks for `height`. A shrink on a
    /// full screen (transcript flush against the band, band flush against
    /// the bottom) is held: sliding the band down would open the rows it
    /// gave up *between* the transcript and the dock, and they stay blank
    /// until something is inserted to fill them, which mid-turn can be a
    /// long wait (a queued multi-line prompt shrinking the input box under
    /// a paused Thinking dock). Held, the spare rows sit below the footer
    /// (the frame clears them) and `insert_before` spends them first. A
    /// short transcript keeps the band pinned to the bottom as before.
    fn held_height(&self, height: u16, screen_size: Size) -> u16 {
        let area = self.viewport_area;
        let full = self.blank_above == 0
            && area.y > 0
            && area.bottom() == screen_size.height
            && screen_size == self.last_known_screen_size;
        if full && height < area.height {
            area.height
        } else {
            height
        }
    }

    pub fn set_viewport_height(&mut self, height: u16, screen_size: Size) -> io::Result<()> {
        let height = height.min(screen_size.height);
        self.wanted_height = height;
        let mut area = self.viewport_area;
        area.height = self.held_height(height, screen_size);
        area.width = screen_size.width;

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
                // Move first: the clear judges "above the blank rows"
                // against the band's top, and the old top made a band that
                // rose into the blank rows forget the ones still left.
                self.set_viewport_area(area);
                self.clear_after_position(clear_pos)?;
            } else {
                self.set_viewport_area(area);
            }
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

        // A held shrink is spent here: the band gives up its surplus rows
        // (up to the rows being inserted) at its top, and the insert draws
        // into exactly those rows, so the gap above the dock stays one row.
        let slack = self
            .viewport_area
            .height
            .saturating_sub(self.wanted_height)
            .min(height);
        if slack > 0 {
            let area = Rect {
                y: self.viewport_area.y + slack,
                height: self.viewport_area.height - slack,
                ..self.viewport_area
            };
            self.set_viewport_area(area);
            self.blank_above = self.blank_above.saturating_add(slack);
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

        // The band keeps its height: rows it gives up were vacated before the
        // insert (`draw::settle_band`), and no commit ends in a blank row that
        // could double the seam (`transcript::margins`).
        let band_height = self.viewport_area.height;

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

    /// Committing a live tool card: the card's rows leave the band in the
    /// same batch the committed rows arrive in. Inserted before the band
    /// shrinks, the commit scrolls the screen; the late shrink used to strand
    /// blank rows between the committed card and the dock, and is now held
    /// on the full screen instead. Settled first (`draw::settle_band`), the
    /// commit fills the rows the band gave up, however many that was.
    #[test]
    fn a_settled_card_commit_leaves_no_blank_rows_above_the_band() {
        use ratatui::style::Style;

        let screen = Size::new(10, 12);
        let insert = |terminal: &mut CustomTerminal<TestBackend>| {
            terminal
                .insert_before(3, |buf| {
                    buf.set_string(0, 2, "c", Style::default());
                })
                .unwrap();
        };
        let fresh = || {
            let mut terminal = CustomTerminal::with_options(TestBackend::new(10, 12), 8).unwrap();
            terminal.insert_before(4, |_| {}).unwrap();
            terminal.set_viewport_height(8, screen).unwrap();
            terminal
        };

        // Shrunk late, the full screen holds the band's height: no stray
        // blank rows above it, the surplus waits below the footer.
        let mut late = fresh();
        insert(&mut late);
        late.set_viewport_height(6, screen).unwrap();
        assert_eq!(late.blank_above, 0, "shrunk late: no stray blank rows");
        assert_eq!(late.viewport_area.bottom(), 12);

        let mut settled = fresh();
        settled.set_viewport_height(6, screen).unwrap();
        insert(&mut settled);
        settled.set_viewport_height(6, screen).unwrap();
        assert_eq!(settled.viewport_area, Rect::new(0, 6, 10, 6));
        assert_eq!(settled.blank_above, 0);
    }

    /// End of turn: the status dock (2 rows) leaves the band and the footer
    /// plus its trailing gap (2 rows) are committed. Committed before the
    /// shrink, the footer scrolls in above the docked band, and the shrink
    /// used to strand a blank row under the gap (the doubled gap after an
    /// interrupt); it is held now. Shrunk first, the two rows land in the
    /// vacated rows.
    #[test]
    fn shrinking_before_the_turn_footer_leaves_no_stray_gap() {
        use ratatui::style::Style;

        let screen = Size::new(10, 12);
        let docked = || {
            let mut terminal = CustomTerminal::with_options(TestBackend::new(10, 12), 6).unwrap();
            terminal.insert_before(6, |_| {}).unwrap();
            terminal.set_viewport_height(6, screen).unwrap();
            terminal
        };
        // Every transcript insert is followed by a frame (`Tui::atomic`),
        // which re-pins the band to the bottom: `band` is that frame's height.
        let footer = |terminal: &mut CustomTerminal<TestBackend>, band: u16| {
            terminal
                .insert_before(1, |buf| {
                    buf.set_string(0, 0, "w", Style::default());
                })
                .unwrap();
            terminal.set_viewport_height(band, screen).unwrap();
        };
        let gap = |terminal: &mut CustomTerminal<TestBackend>| {
            terminal.insert_before(1, |_| {}).unwrap();
            terminal.set_viewport_height(4, screen).unwrap();
        };

        // Old order: the footer lands while the dock is still in the band,
        // and the first frame after it drops the dock.
        let mut late = docked();
        footer(&mut late, 4);
        gap(&mut late);
        assert_eq!(late.blank_above, 0, "footer before shrink: no stray row");

        // New order: the dock leaves first, then the footer and gap.
        let mut early = docked();
        early.set_viewport_height(4, screen).unwrap();
        footer(&mut early, 4);
        gap(&mut early);
        assert_eq!(early.blank_above, 0, "shrink first: no stray row");
        assert_eq!(early.viewport_area, Rect::new(0, 8, 10, 4));
    }

    /// The reported doubled margin: a multi-line follow-up typed and queued
    /// mid-turn collapses the input box while the dock idles on a paused
    /// Thinking run. Nothing is inserted, so slid down, the band left the
    /// rows it gave up blank between the transcript and the dock until the
    /// next row arrived. On a full screen the band keeps those rows (below
    /// its footer) and the next inserts spend them.
    #[test]
    fn a_shrink_with_nothing_to_insert_keeps_the_dock_under_the_transcript() {
        use ratatui::style::Style;

        let screen = Size::new(10, 12);
        let mut terminal = CustomTerminal::with_options(TestBackend::new(10, 12), 7).unwrap();
        terminal
            .insert_before(5, |buf| {
                buf.set_string(0, 4, "t", Style::default());
            })
            .unwrap();
        terminal.set_viewport_height(7, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 5, 10, 7));

        // The box collapses: 7 rows wanted, 4 kept. The band stays put,
        // flush under the transcript's last row.
        terminal.set_viewport_height(4, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 5, 10, 7));
        assert_eq!(terminal.blank_above, 0, "no blank rows above the dock");

        // One row arrives: it takes one held row, the band shrinks by one
        // and still sits flush under it and against the screen bottom.
        terminal
            .insert_before(1, |buf| {
                buf.set_string(0, 0, "u", Style::default());
            })
            .unwrap();
        terminal.set_viewport_height(4, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 6, 10, 6));
        assert_eq!(terminal.blank_above, 0);
        let buffer = terminal.backend.buffer();
        assert_eq!(buffer[(0, 4)].symbol(), "t", "nothing scrolled");
        assert_eq!(
            buffer[(0, 5)].symbol(),
            "u",
            "the row lands under the last one"
        );

        // Enough rows spend the rest: the band is back to its wanted height.
        terminal.insert_before(4, |_| {}).unwrap();
        terminal.set_viewport_height(4, screen).unwrap();
        assert_eq!(terminal.viewport_area, Rect::new(0, 8, 10, 4));
        assert_eq!(terminal.blank_above, 0);
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
