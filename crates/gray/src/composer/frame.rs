use super::*;

impl Tui {
    pub(crate) fn width(&self) -> usize {
        self.last_width.max(20) as usize
    }

    pub(crate) fn set_background(&mut self, path: Option<&std::path::Path>) -> anyhow::Result<()> {
        anyhow::ensure!(!self.background_closed, "Gray TUI has closed");
        anyhow::ensure!(
            !self.modal_open,
            "close the modal before changing background"
        );
        let next = if let Some(path) = path {
            anyhow::ensure!(
                std::env::var_os("TMUX").is_none() && std::env::var_os("STY").is_none(),
                "background through tmux/screen is not supported"
            );
            anyhow::ensure!(
                matches!(
                    std::env::var("TERM_PROGRAM")
                        .unwrap_or_default()
                        .to_lowercase()
                        .as_str(),
                    "ghostty" | "kitty"
                ),
                "background requires Ghostty or Kitty"
            );
            Some(background::Background::load(path)?)
        } else {
            None
        };
        self.hide_background()?;
        self.background = next;
        self.draw()
    }

    pub(crate) fn hide_background(&mut self) -> std::io::Result<()> {
        if let Some(bg) = &mut self.background {
            bg.hide(&mut std::io::stdout().lock())?;
        }
        Ok(())
    }

    /// Hand the real terminal to a scrolling child process: reset the
    /// viewport's DECSTBM margins and park the cursor on a fresh last row so
    /// the child appends ordinary scroll text instead of writing trapped
    /// inside the viewport's scroll strip. The band scrolls up into history;
    /// `reanchor_viewport` repaints it when the child exits.
    pub(crate) fn release_for_child(&mut self) {
        use std::io::Write as _;
        let mut out = std::io::stdout();
        let rows = crossterm::terminal::size().map(|(_, r)| r).unwrap_or(24);
        let _ = out.write_all(b"\x1b[r");
        let _ = crossterm::execute!(out, crossterm::cursor::MoveTo(0, rows.saturating_sub(1)));
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }

    pub(crate) fn set_modal_open(&mut self, open: bool) {
        if open {
            let _ = self.hide_background();
        }
        self.modal_open = open;
        if !open {
            self.flush_modal_pending();
        }
    }

    pub(crate) fn flush_modal_pending(&mut self) {
        if self.modal_pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.modal_pending);
        for item in pending {
            match item {
                PendingInsert::Rows(lines, bg) => self.insert_paragraph(&lines, bg),
                PendingInsert::Styled(rows) => self.insert_styled_rows(rows),
            }
        }
        let _ = self.draw();
    }

    pub(crate) fn begin_insert_batch(&mut self) {
        self.insert_batch.get_or_insert_with(Vec::new);
    }

    pub(crate) fn end_insert_batch(&mut self) {
        let Some(rows) = self.insert_batch.take() else {
            return;
        };
        if rows.is_empty() {
            return;
        }
        self.atomic(|t| {
            let _ = draw::settle_band(t);
            let total_h = rows.len() as u16;
            let _ = t.terminal.insert_before(total_h, |buf| {
                transcript::paint_insert_rows(buf, &rows);
            });
        });
    }

    pub(crate) fn draw(&mut self) -> anyhow::Result<()> {
        if self.intro_started.is_some() {
            self.draw_intro();
            return Ok(());
        }
        draw::draw(self)
    }
}
