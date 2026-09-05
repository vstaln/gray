//! Question session state + resolution flow (split from `question`).

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Options,
    Notes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AutoResolutionTiming {
    Disabled,
    HiddenGrace { remaining: Duration },
    VisibleCountdown { remaining: Duration },
    Due,
}

fn format_remaining(remaining: Duration) -> String {
    let mut secs = remaining.as_secs();
    if remaining.subsec_nanos() > 0 {
        secs = secs.saturating_add(1);
    }
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

pub(crate) struct AnswerState {
    pub(crate) selected_idx: Option<usize>,
    pub(crate) draft: String,
    pub(crate) answer_committed: bool,
    pub(crate) notes_visible: bool,
}

pub(crate) struct QueuedRequest {
    pub questions: Vec<UserQuestion>,
    pub blocking: bool,
    pub tx: oneshot::Sender<Vec<UserAnswer>>,
    pub resolved: Arc<AtomicBool>,
}

pub(crate) struct QuestionSession {
    pub questions: Vec<UserQuestion>,
    pub blocking: bool,
    current_tx: Option<oneshot::Sender<Vec<UserAnswer>>>,
    current_resolved: Arc<AtomicBool>,
    pub queue: VecDeque<QueuedRequest>,
    pub(crate) answers: Vec<AnswerState>,
    pub(crate) current_idx: usize,
    pub(crate) focus: Focus,
    pub(crate) confirm_unanswered: Option<usize>,
    pub(crate) request_started_at: Instant,
    auto_snoozed: bool,
    pub(crate) last_countdown: Option<String>,
}

#[derive(Debug)]
pub(crate) enum QuestionOutcome {
    /// Key ignored / nothing changed.
    None,
    /// State changed; redraw and keep the session.
    Redraw,
    /// A request resolved. Caller pushes the transcript summary, routes
    /// non-blocking answers to the pending queue, and keeps the session
    /// only when `session_done` is false.
    Resolved {
        answers: Vec<UserAnswer>,
        questions: Vec<UserQuestion>,
        blocking: bool,
        session_done: bool,
    },
}

fn init_answers(questions: &[UserQuestion]) -> Vec<AnswerState> {
    questions
        .iter()
        .map(|_| AnswerState {
            // Option 1 preselected AND committed: cursor, accent, and answer
            // agree from the start, so Enter submits it with zero moves and
            // the highlight can never silently come back empty. Moving with
            // ↑/↓ un-commits (pick again with Enter/Space/digit);
            // Backspace skips explicitly.
            selected_idx: Some(0),
            draft: String::new(),
            answer_committed: true,
            notes_visible: false,
        })
        .collect()
}

pub(crate) enum TickOutcome {
    Idle,
    Redraw,
    AutoResolved {
        answers: Vec<UserAnswer>,
        questions: Vec<UserQuestion>,
        blocking: bool,
        session_done: bool,
    },
}

impl QuestionSession {
    pub(crate) fn new(
        questions: Vec<UserQuestion>,
        blocking: bool,
        tx: oneshot::Sender<Vec<UserAnswer>>,
        resolved: Arc<AtomicBool>,
    ) -> Self {
        Self {
            answers: init_answers(&questions),
            current_idx: 0,
            focus: Focus::Options,
            confirm_unanswered: None,
            request_started_at: Instant::now(),
            auto_snoozed: false,
            last_countdown: None,
            questions,
            blocking,
            current_tx: Some(tx),
            current_resolved: resolved,
            queue: VecDeque::new(),
        }
    }

    pub(crate) fn current_question(&self) -> &UserQuestion {
        &self.questions[self.current_idx]
    }

    /// Options plus the auto-added "Other" row (codex other_option_enabled).
    pub(crate) fn options_len(&self) -> usize {
        let q = self.current_question();
        q.options.len() + usize::from(q.is_other && !q.options.is_empty())
    }

    pub(crate) fn option_label_for_index(&self, idx: usize) -> Option<String> {
        let q = self.current_question();
        if idx < q.options.len() {
            Some(q.options[idx].label.clone())
        } else if idx == q.options.len() && q.is_other && !q.options.is_empty() {
            Some(OTHER_OPTION_LABEL.to_string())
        } else {
            None
        }
    }

    pub(crate) fn unanswered_count(&self) -> usize {
        self.answers.iter().filter(|a| !a.answer_committed).count()
    }

    pub(crate) fn progress_prefix(&self) -> String {
        let base = format!("Question {}/{}", self.current_idx + 1, self.questions.len());
        match self.unanswered_count() {
            0 => base,
            n => format!("{base} ({n} unanswered)"),
        }
    }

    pub(crate) fn auto_resolution_timing_at(&self, now: Instant) -> AutoResolutionTiming {
        if self.blocking || self.auto_snoozed {
            return AutoResolutionTiming::Disabled;
        }
        let elapsed = now.saturating_duration_since(self.request_started_at);
        if elapsed < AUTO_RESOLUTION_HIDDEN_GRACE {
            return AutoResolutionTiming::HiddenGrace {
                remaining: AUTO_RESOLUTION_HIDDEN_GRACE.saturating_sub(elapsed),
            };
        }
        let visible = elapsed
            .checked_sub(AUTO_RESOLUTION_HIDDEN_GRACE)
            .unwrap_or_default();
        if visible < AUTO_RESOLUTION_VISIBLE_COUNTDOWN {
            AutoResolutionTiming::VisibleCountdown {
                remaining: AUTO_RESOLUTION_VISIBLE_COUNTDOWN.saturating_sub(visible),
            }
        } else {
            AutoResolutionTiming::Due
        }
    }

    /// Ticker entry: fires empty auto-resolution when due; returns an
    /// outcome only when a resolution or a visible countdown change happened.
    pub(crate) fn tick(&mut self, now: Instant) -> TickOutcome {
        match self.auto_resolution_timing_at(now) {
            AutoResolutionTiming::Due => {
                let questions = self.questions.clone();
                let blocking = self.blocking;
                let session_done = self.resolve_with(Vec::new());
                TickOutcome::AutoResolved {
                    answers: Vec::new(),
                    questions,
                    blocking,
                    session_done,
                }
            }
            AutoResolutionTiming::VisibleCountdown { remaining } => {
                let text = format!("auto-resolves in {}", format_remaining(remaining));
                if self.last_countdown.as_deref() != Some(text.as_str()) {
                    self.last_countdown = Some(text);
                    TickOutcome::Redraw
                } else {
                    TickOutcome::Idle
                }
            }
            _ => TickOutcome::Idle,
        }
    }

    /// Sends answers for the current request, promotes the next queued
    /// request (codex advance_queue_or_complete) or ends the session.
    /// Returns `true` when the session is done.
    fn resolve_with(&mut self, answers: Vec<UserAnswer>) -> bool {
        if let Some(tx) = self.current_tx.take() {
            self.current_resolved.store(true, Ordering::Relaxed);
            let _ = tx.send(answers);
        }
        if let Some(next) = self.queue.pop_front() {
            self.questions = next.questions;
            self.blocking = next.blocking;
            self.current_tx = Some(next.tx);
            self.current_resolved = next.resolved;
            self.answers = init_answers(&self.questions);
            self.current_idx = 0;
            self.focus = Focus::Options;
            self.confirm_unanswered = None;
            self.request_started_at = Instant::now();
            self.auto_snoozed = false;
            self.last_countdown = None;
            false
        } else {
            true
        }
    }

    fn save_current_draft(&mut self, ta: &TextArea) {
        self.answers[self.current_idx].draft = ta.text().to_string();
    }

    fn restore_current_draft(&mut self, ta: &mut TextArea) {
        ta.set_text(&self.answers[self.current_idx].draft);
        ta.move_to_end();
    }

    fn move_question(&mut self, next: bool, ta: &mut TextArea) {
        let len = self.questions.len();
        self.save_current_draft(ta);
        let offset = if next { 1 } else { len.saturating_sub(1) };
        self.current_idx = (self.current_idx + offset) % len;
        self.restore_current_draft(ta);
        if self.focus == Focus::Notes {
            self.focus = Focus::Options;
        }
    }

    fn select_current_option(&mut self, committed: bool) {
        let len = self.options_len();
        let a = &mut self.answers[self.current_idx];
        // Selecting seeds the cursor at the highlighted row (defaults to first).
        a.selected_idx = Some(match a.selected_idx {
            Some(sel) => sel.min(len.saturating_sub(1)),
            None => 0,
        });
        a.answer_committed = committed;
    }

    /// Explicit skip (Backspace): resolve this question empty and move on.
    /// Committed-empty is distinct from untouched — it never triggers the
    /// end confirmation. Pick an option later to un-skip.
    fn skip_current(&mut self, ta: &mut TextArea) -> QuestionOutcome {
        let a = &mut self.answers[self.current_idx];
        a.selected_idx = None;
        a.draft = String::new();
        a.answer_committed = true;
        a.notes_visible = false;
        ta.set_text("");
        ta.move_to_end();
        if self.focus == Focus::Notes {
            self.focus = Focus::Options;
        }
        self.go_next_or_submit(ta)
    }

    fn clear_notes_and_focus_options(&mut self, ta: &mut TextArea) {
        let a = &mut self.answers[self.current_idx];
        a.draft = String::new();
        a.answer_committed = false;
        a.notes_visible = false;
        ta.set_text("");
        ta.move_to_end();
        self.focus = Focus::Options;
    }

    fn go_next_or_submit(&mut self, ta: &mut TextArea) -> QuestionOutcome {
        if self.current_idx + 1 >= self.questions.len() {
            self.save_current_draft(ta);
            if self.unanswered_count() > 0 {
                // Default to "Go back": Enter-hammering through questions must
                // return to the unanswered ones, never submit empties.
                self.confirm_unanswered = Some(1);
                QuestionOutcome::Redraw
            } else {
                self.submit_answers(ta)
            }
        } else {
            self.move_question(true, ta);
            QuestionOutcome::Redraw
        }
    }

    fn submit_answers(&mut self, ta: &TextArea) -> QuestionOutcome {
        self.confirm_unanswered = None;
        self.save_current_draft(ta);
        let mut out = Vec::new();
        for (idx, q) in self.questions.iter().enumerate() {
            let a = &self.answers[idx];
            let mut list = Vec::new();
            if a.answer_committed
                && let Some(sel) = a.selected_idx
                && let Some(label) = self.option_label_for_index_for(idx, sel)
            {
                list.push(label);
            }
            if a.answer_committed {
                let notes = a.draft.trim();
                if !notes.is_empty() {
                    list.push(format!("user_note: {notes}"));
                }
            }
            out.push(UserAnswer {
                id: q.id.clone(),
                answers: list,
            });
        }
        let session_done = self.resolve_with(out.clone());
        QuestionOutcome::Resolved {
            answers: out,
            questions: self.questions.clone(),
            blocking: self.blocking,
            session_done,
        }
    }

    fn option_label_for_index_for(&self, idx: usize, sel: usize) -> Option<String> {
        let q = &self.questions[idx];
        if sel < q.options.len() {
            Some(q.options[sel].label.clone())
        } else if sel == q.options.len() && q.is_other && !q.options.is_empty() {
            Some(OTHER_OPTION_LABEL.to_string())
        } else {
            None
        }
    }

    fn first_unanswered_index(&self) -> Option<usize> {
        self.answers.iter().position(|a| !a.answer_committed)
    }

    fn handle_confirm_key(&mut self, code: KeyCode, ta: &mut TextArea) -> QuestionOutcome {
        let Some(sel) = self.confirm_unanswered.as_mut() else {
            return QuestionOutcome::None;
        };
        match code {
            KeyCode::Esc | KeyCode::Backspace => {
                self.confirm_unanswered = None;
                if let Some(idx) = self.first_unanswered_index() {
                    self.current_idx = idx;
                    self.restore_current_draft(ta);
                }
                QuestionOutcome::Redraw
            }
            KeyCode::Up | KeyCode::Char('k') => {
                *sel = sel.saturating_sub(1);
                QuestionOutcome::Redraw
            }
            KeyCode::Down | KeyCode::Char('j') => {
                *sel = (*sel + 1).min(1);
                QuestionOutcome::Redraw
            }
            KeyCode::Enter => {
                let selected = *sel;
                self.confirm_unanswered = None;
                if selected == 0 {
                    self.submit_answers(ta)
                } else if let Some(idx) = self.first_unanswered_index() {
                    self.current_idx = idx;
                    self.restore_current_draft(ta);
                    QuestionOutcome::Redraw
                } else {
                    QuestionOutcome::Redraw
                }
            }
            KeyCode::Char('1') => {
                *sel = 0;
                QuestionOutcome::Redraw
            }
            KeyCode::Char('2') => {
                *sel = 1;
                QuestionOutcome::Redraw
            }
            _ => QuestionOutcome::None,
        }
    }

    pub(crate) fn on_key(
        &mut self,
        code: KeyCode,
        mods: KeyModifiers,
        ta: &mut TextArea,
    ) -> QuestionOutcome {
        if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
            return QuestionOutcome::None; // global Ctrl-C policy handles the turn
        }
        self.auto_snoozed = !self.blocking;

        if self.confirm_unanswered.is_some() {
            return self.handle_confirm_key(code, ta);
        }

        // Esc: clear notes, refocus options (codex prefer_esc semantics).
        if code == KeyCode::Esc && self.focus == Focus::Notes {
            self.clear_notes_and_focus_options(ta);
            return QuestionOutcome::Redraw;
        }

        // Question navigation is always available (codex).
        match (code, mods) {
            (KeyCode::Char('p'), KeyModifiers::CONTROL) | (KeyCode::PageUp, KeyModifiers::NONE) => {
                self.move_question(false, ta);
                return QuestionOutcome::Redraw;
            }
            (KeyCode::Char('n'), KeyModifiers::CONTROL)
            | (KeyCode::PageDown, KeyModifiers::NONE) => {
                self.move_question(true, ta);
                return QuestionOutcome::Redraw;
            }
            _ => {}
        }

        match self.focus {
            Focus::Options => match code {
                KeyCode::Left | KeyCode::Char('h') => {
                    self.move_question(false, ta);
                    QuestionOutcome::Redraw
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    self.move_question(true, ta);
                    QuestionOutcome::Redraw
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let len = self.options_len();
                    let a = &mut self.answers[self.current_idx];
                    let cur = a.selected_idx.take().unwrap_or(0);
                    a.selected_idx = Some(if cur == 0 { len - 1 } else { cur - 1 });
                    a.answer_committed = false;
                    QuestionOutcome::Redraw
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let len = self.options_len();
                    let a = &mut self.answers[self.current_idx];
                    let cur = a.selected_idx.take().unwrap_or(0);
                    a.selected_idx = Some(if cur + 1 >= len { 0 } else { cur + 1 });
                    a.answer_committed = false;
                    QuestionOutcome::Redraw
                }
                KeyCode::Char(' ') => {
                    self.select_current_option(true);
                    QuestionOutcome::Redraw
                }
                KeyCode::Backspace | KeyCode::Delete => self.skip_current(ta),
                KeyCode::Tab => {
                    // Seed the highlight so Tab works on a fresh question too.
                    self.select_current_option(false);
                    self.answers[self.current_idx].notes_visible = true;
                    self.focus = Focus::Notes;
                    QuestionOutcome::Redraw
                }
                KeyCode::Enter => {
                    // Highlight is the pick: Enter always commits what's shown, so a
                    // selection can never silently come back empty. Skip explicitly
                    // with Backspace; leaving via ←/→ still asks at the end.
                    self.select_current_option(true);
                    self.go_next_or_submit(ta)
                }
                KeyCode::Char(c) => {
                    if let Some(idx) = c
                        .to_digit(10)
                        .and_then(|d| (d >= 1).then(|| d as usize - 1))
                        && idx < self.options_len()
                    {
                        self.answers[self.current_idx].selected_idx = Some(idx);
                        self.select_current_option(true);
                        self.go_next_or_submit(ta)
                    } else if c.is_ascii_graphic() || c == ' ' {
                        // Typing while focused on options jumps into notes (codex).
                        self.answers[self.current_idx].notes_visible = true;
                        self.focus = Focus::Notes;
                        ta.insert_str(&c.to_string());
                        QuestionOutcome::Redraw
                    } else {
                        QuestionOutcome::None
                    }
                }
                _ => QuestionOutcome::None,
            },
            Focus::Notes => match code {
                KeyCode::Tab => {
                    self.clear_notes_and_focus_options(ta);
                    QuestionOutcome::Redraw
                }
                KeyCode::Backspace if ta.text().trim().is_empty() => {
                    let a = &mut self.answers[self.current_idx];
                    a.notes_visible = false;
                    self.focus = Focus::Options;
                    QuestionOutcome::Redraw
                }
                KeyCode::Up | KeyCode::Down => {
                    // Adjust selection without leaving notes (codex).
                    let len = self.options_len();
                    let a = &mut self.answers[self.current_idx];
                    let cur = a.selected_idx.take().unwrap_or(0);
                    a.selected_idx = Some(match code {
                        KeyCode::Up => {
                            if cur == 0 {
                                len - 1
                            } else {
                                cur - 1
                            }
                        }
                        _ => {
                            if cur + 1 >= len {
                                0
                            } else {
                                cur + 1
                            }
                        }
                    });
                    QuestionOutcome::Redraw
                }
                KeyCode::Enter => {
                    self.answers[self.current_idx].answer_committed = true;
                    self.go_next_or_submit(ta)
                }
                KeyCode::Backspace => {
                    ta.delete_backward(1);
                    QuestionOutcome::Redraw
                }
                KeyCode::Delete => {
                    ta.delete_forward(1);
                    QuestionOutcome::Redraw
                }
                KeyCode::Char(c) => {
                    ta.insert_str(&c.to_string());
                    QuestionOutcome::Redraw
                }
                _ => QuestionOutcome::None,
            },
        }
    }
}

/// Bridge entry (called from `ComposerQuestionAsker`): parks the request on
/// the TUI — queueing behind an active session like codex.
pub(crate) fn attach_request(
    t: &mut Tui,
    questions: Vec<UserQuestion>,
    blocking: bool,
    tx: oneshot::Sender<Vec<UserAnswer>>,
) {
    let resolved = Arc::new(AtomicBool::new(false));
    if let Some(q) = t.active_question.as_mut() {
        q.queue.push_back(QueuedRequest {
            questions,
            blocking,
            tx,
            resolved,
        });
        let _ = t.draw();
        return;
    }
    t.matches.clear();
    t.sel = 0;
    t.textarea.set_text("");
    t.active_question = Some(QuestionSession::new(questions, blocking, tx, resolved));
    let _ = t.draw();
}

/// Watcher entry: routes a key event into the active session.
pub(crate) fn handle_question_key(t: &mut Tui, code: KeyCode, mods: KeyModifiers) {
    let Some(mut q) = t.active_question.take() else {
        return;
    };
    match q.on_key(code, mods, &mut t.textarea) {
        QuestionOutcome::None => t.active_question = Some(q),
        QuestionOutcome::Redraw => {
            t.active_question = Some(q);
            let _ = t.draw();
        }
        QuestionOutcome::Resolved {
            answers,
            questions,
            blocking,
            session_done,
        } => {
            finish_resolution(t, q, answers, questions, blocking, session_done);
        }
    }
}

/// Ticker entry: drives auto-resolution for non-blocking requests.
pub(crate) fn tick_question(t: &mut Tui) {
    let Some(mut q) = t.active_question.take() else {
        return;
    };
    match q.tick(Instant::now()) {
        TickOutcome::Idle => t.active_question = Some(q),
        TickOutcome::Redraw => {
            t.active_question = Some(q);
            let _ = t.draw();
        }
        TickOutcome::AutoResolved {
            answers,
            questions,
            blocking,
            session_done,
        } => {
            finish_resolution(t, q, answers, questions, blocking, session_done);
        }
    }
}

fn finish_resolution(
    t: &mut Tui,
    q: QuestionSession,
    answers: Vec<UserAnswer>,
    questions: Vec<UserQuestion>,
    blocking: bool,
    session_done: bool,
) {
    push_result_summary(t, &questions, &answers, answers.is_empty() && !blocking);
    if !blocking && answers.iter().any(|a| !a.answers.is_empty()) {
        t.pending_question_answers
            .push(format_answers_text(&questions, &answers));
    }
    if session_done {
        // Restore the composer to a clean prompt state.
        t.textarea.set_text("");
        t.matches.clear();
        t.sel = 0;
        t.pending_pastes.clear();
    } else {
        t.active_question = Some(q);
    }
    let _ = t.draw();
}

fn format_answers_text(questions: &[UserQuestion], answers: &[UserAnswer]) -> String {
    let mut out = String::from("[user answered your earlier questions]");
    for q in questions {
        if let Some(a) = answers.iter().find(|a| a.id == q.id)
            && !a.answers.is_empty()
        {
            out.push_str(&format!("\n{} → {}", q.question, a.answers.join(" · ")));
        }
    }
    out
}

/// Resolved-question summary rows: the question on its own row, the outcome
/// (`→ answer` or `→ skipped`) stacked below it — never jammed on one line.
pub(crate) fn result_summary_lines(
    questions: &[UserQuestion],
    answers: &[UserAnswer],
) -> Vec<String> {
    let mut lines = Vec::new();
    for q in questions {
        let joined = answers
            .iter()
            .find(|a| a.id == q.id)
            .map(|a| a.answers.join(" · "))
            .unwrap_or_default();
        lines.push(format!("? {}", q.question));
        if joined.is_empty() {
            lines.push("  → skipped".to_string());
        } else {
            lines.push(format!("  → {joined}"));
        }
    }
    lines
}

fn push_result_summary(
    t: &mut Tui,
    questions: &[UserQuestion],
    answers: &[UserAnswer],
    _auto: bool,
) {
    let lines = result_summary_lines(questions, answers);
    if !lines.is_empty() {
        t.ensure_gap(1);
        // Stacked: one transcript row per line, not a joined blob.
        for line in lines {
            t.push_dim(line);
        }
        t.ensure_gap(1);
    }
}
