use super::*;

impl Tui {
    /// Skip (a key) or finish the intro. A skip lands on the steady home
    /// screen at once; a natural finish hands off to the band fade-in.
    pub(crate) fn end_intro(&mut self) {
        self.home_reveal = None;
        if self.intro_started.take().is_some() {
            self.reflow_on_resize_inner(self.last_width);
        }
    }

    pub(super) fn finish_intro(&mut self) {
        if self.intro_started.take().is_some() {
            self.home_reveal = Some(Instant::now());
            self.reflow_on_resize_inner(self.last_width);
        }
    }

    /// The home band's fade-in alpha while it runs; clears itself at 1.
    pub(crate) fn home_reveal_alpha(&mut self) -> Option<f32> {
        let started = self.home_reveal?;
        let t = started.elapsed().as_secs_f32() * 1000.0 / HOME_REVEAL_MS as f32;
        if t >= 1.0 {
            self.home_reveal = None;
            return None;
        }
        Some(ease_out_cubic(t))
    }

    pub(super) fn draw_intro(&mut self) {
        if self.modal_open {
            return;
        }
        let Some(started) = self.intro_started else {
            return;
        };
        let elapsed = started.elapsed();
        if elapsed >= Duration::from_millis(INTRO_TOTAL_MS) {
            self.finish_intro();
            return;
        }
        let w = self.last_width as usize;
        let progress = (elapsed.as_secs_f64() * 1000.0 / INTRO_TOTAL_MS as f64) as f32;
        let hint_alpha = ease_out_cubic(elapsed.as_secs_f32() * 1000.0 / MARK_FADE_MS);
        let dim = Style::default().fg(faded(crate::theme::theme().text_dim, hint_alpha));
        let _ = self.terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(
                Paragraph::new(welcome_lines_at(w, progress, Some(elapsed))),
                area,
            );
            let row = ratatui::layout::Rect::new(
                area.x,
                area.y + area.height.saturating_sub(1),
                area.width,
                1,
            );
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "enter to skip · /intro disable to disable",
                    dim,
                ))
                .alignment(ratatui::layout::Alignment::Center),
                row,
            );
        });
    }
}
