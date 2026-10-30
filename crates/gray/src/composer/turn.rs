use super::*;

impl Tui {
    pub fn handle_paste(&mut self, pasted: String) -> bool {
        input::handle_paste(self, pasted)
    }

    pub fn begin_turn(&mut self, label: &str) {
        self.left_home = true;
        let now = Instant::now();

        self.allow_empty_submit = false;

        self.turn_show_effort = Some(footer_badge_visible(&self.model_name, None));
        self.cache.pause(now);
        self.stream_round_boundary = false;
        self.stream_round_had_text = false;
        self.stream_round_target = None;

        self.discard_held_punctuation();

        if let Some(active) = self.active_compaction.clone() {
            self.is_task_running = true;
            self.status = Some((active.started_at, COMPACTION_HEADER.to_string()));
            let _ = self.draw();
            return;
        }
        if self.turn_started.is_none() {
            self.turn_started = Some(now);
            self.turn_had_thinking = false;
        }
        self.turn_billed_output = None;
        self.streamed_bytes = 0;
        self.turn_output_accum = 0;
        self.turn_stream_ms = 0;
        self.is_task_running = true;
        self.status = Some((now, label.to_string()));
        let _ = self.draw();
    }

    pub(crate) fn take_status(&mut self) -> Option<(Instant, String)> {
        self.status.take()
    }

    pub(crate) fn restore_status(&mut self, status: Option<(Instant, String)>) {
        self.status = status;
    }

    pub(crate) fn set_background_work(&mut self, label: Option<String>) {
        if self.background_work != label {
            self.background_work = label;
            let _ = self.draw();
        }
    }

    pub fn set_status(&mut self, label: Option<&str>) {
        if let Some(active) = self.active_compaction.clone() {
            self.status = Some((active.started_at, COMPACTION_HEADER.to_string()));
            let _ = self.draw();
            return;
        }
        self.status = label.map(|l| (Instant::now(), l.to_string()));
        let _ = self.draw();
    }

    pub(crate) fn mark_stream_round_boundary(&mut self) {
        if self.is_task_running {
            if self.stream_round_had_text {
                let was_boundary = self.stream_round_boundary;

                let target = self.history_entries.iter().rposition(|entry| {
                    matches!(
                        entry,
                        TranscriptEntry::StyledLines { lines, .. }
                            if lines.iter().any(|line| {
                                !crate::composer::transcript::transcript_row_is_blank(line)
                            })
                    )
                });
                if target.is_some() {
                    self.stream_round_target = target;
                    self.stream_round_boundary = true;
                } else if !was_boundary {
                    self.stream_round_target = None;
                    self.stream_round_boundary = false;
                }
            }

            self.stream_round_had_text = false;
        }
    }

    pub fn begin_compaction(&mut self, id: String) {
        if self
            .active_compaction
            .as_ref()
            .is_some_and(|active| active.id == id)
        {
            return;
        }
        self.flush_markdown();
        self.end_thinking_run(true);
        self.is_task_running = true;

        self.allow_empty_submit = false;
        let started_at = Instant::now();
        self.active_compaction = Some(ActiveCompaction { id, started_at });
        self.status = Some((started_at, COMPACTION_HEADER.to_string()));
        let _ = self.draw();
    }

    pub fn finish_compaction(&mut self, id: &str, restore: Option<&str>) -> Option<Duration> {
        let active = self.active_compaction.take()?;
        if active.id != id {
            self.active_compaction = Some(active);
            return None;
        }
        let elapsed = active.started_at.elapsed();
        match restore {
            Some(label) => {
                self.status = Some((Instant::now(), label.to_string()));
            }
            None => {
                self.is_task_running = false;
                self.status = None;
            }
        }
        let _ = self.draw();
        Some(elapsed)
    }

    pub fn compaction_elapsed(&self) -> Option<Duration> {
        self.active_compaction
            .as_ref()
            .map(|a| a.started_at.elapsed())
    }

    pub fn flush_markdown(&mut self) {
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            let style = if self.thinking {
                crate::composer::transcript::thinking_style()
            } else {
                Style::default()
            };
            for line in rest.split('\n') {
                if !line.is_empty() {
                    self.push_line_styled(line.to_string(), style);
                }
            }
        }
        self.commit_markdown_tail();
    }

    pub fn end_turn(&mut self, stream_ms: u64) {
        self.rearm_cache();

        self.active_compaction = None;

        self.live_tools.clear();

        let elapsed = self.turn_started.take().map(|s| s.elapsed());
        let had_thinking = self.turn_had_thinking;
        self.last_retry_detail = None;
        self.turn_had_thinking = false;
        if self.thinking {
            self.end_thinking_run(true);
        }

        self.flush_markdown();
        self.stream_round_boundary = false;
        self.stream_round_had_text = false;
        self.stream_round_target = None;

        self.discard_held_punctuation();
        self.is_task_running = false;
        self.status = None;
        self.turn_show_effort = None;

        let _ = self.draw();

        let turn_toks = self.turn_billed_output;
        self.turn_billed_output = None;
        self.turn_output_accum = 0;
        self.turn_stream_ms = 0;

        let warnings = crate::take_profile_warnings();
        if !warnings.is_empty() {
            self.ensure_gap();
            for w in warnings {
                self.push_dim(format!("warning: {w}"));
            }
        }

        if let Some(elapsed) = elapsed {
            let line = turn_footer_line(had_thinking, elapsed, turn_toks, stream_ms);
            self.ensure_gap();
            self.push_dim(line);
            self.ensure_gap();
        }
        let _ = std::io::stdout().flush();
        let _ = self.draw();
    }

    pub fn snapshot(&self) -> crate::setup::BackgroundSnapshot {
        let (used_tokens, cache_hit_rate) =
            if let Some(u) = self.latest_usage.or(self.cumulative_usage) {
                (u.total(), u.cache_hit_rate())
            } else {
                (0, 0.0)
            };
        let snap = crate::setup::BackgroundSnapshot {
            transcript: self.transcript.clone(),
            history_entries: self.history_entries.clone(),
            cwd: self.cwd.clone(),
            model_name: self.model_name.clone(),
            thinking_effort: self.thinking_effort.clone(),
            prompt_text: self.textarea.text().to_string(),
            used_tokens,
            cache_hit_rate,
            home: self.is_home(),
            home_notice: self.home_notice.clone(),
            plugin_stats: self.plugin_stats.clone(),
            home_update: self.home_update.clone(),
            home_quality_of_life: self.home_quality_of_life.clone(),
            home_session: self.home_session.clone(),
            attachments: self.attachments.clone(),
            transcript_cache: Default::default(),
            backdrop_cache: Default::default(),
        };
        let (w, _) = crossterm::terminal::size().unwrap_or((80, 24));
        snap.rebuild_transcript(w as usize);
        snap
    }

    pub fn tick_status(&mut self) {
        if self.modal_open {
            return;
        }

        if let Some((cols, deadline)) = self.pending_resize {
            if Instant::now() >= deadline {
                self.pending_resize = None;
                let (live_cols, live_rows) =
                    crossterm::terminal::size().unwrap_or((cols, self.last_height));
                if live_cols != self.last_width || live_rows != self.last_height {
                    self.reflow_on_resize(live_cols);
                    return;
                }
            }
        } else if let Ok((cols, rows)) = crossterm::terminal::size()
            && (cols != self.last_width || rows != self.last_height)
        {
            self.pending_resize = Some((cols, Instant::now() + Duration::from_millis(75)));
            if self.status.is_none() {
                return;
            }
        }
        let widget_changed = self.plugin_widget.refresh();

        let hint_cleared = if self.hint_expired() {
            self.hint = None;
            true
        } else {
            false
        };
        let cache_ticking = self.cache_remaining().is_some();
        let footer_changed = crate::statusline::version() != self.status_line_seen;
        if self.intro_started.is_none() {
            self.tick_replay();
        }
        if self.intro_started.is_none()
            && self.status.is_none()
            && self.live_tools.is_empty()
            && !self.plugin_widget.active()
            && !widget_changed
            && !cache_ticking
            && !footer_changed
            && !hint_cleared
        {
            return;
        }
        let _ = self.draw();
    }

    pub fn shutdown(&mut self) {
        self.background_closed = true;
        let _ = self.hide_background();
        self.background = None;
        crate::composer::kitty::flush_transmits();

        self.gap_owed = true;
        self.pay_gap();
        let _ = self.terminal.clear();
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::cursor::Show,
            crossterm::cursor::MoveToColumn(0),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown),
        );
        let _ = std::io::stdout().flush();
    }
}
