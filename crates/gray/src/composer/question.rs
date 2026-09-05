//! `request_user_input` overlay: codex `bottom_pane/request_user_input`
//! state machine ported onto gray's inline viewport.
//!
//! Behaviors kept from codex: per-question option selection (↑/↓/digits),
//! typing jumps into notes, tab/esc clear notes, ←/→ + ctrl-p/n question
//! navigation, unanswered-submit confirmation, 60s+60s auto-resolution for
//! non-blocking requests (snoozed on keypress), request queueing.
//! Adaptations: option 1 preselected and committed (cursor, accent, and
//! answer agree from the start — Enter submits it with zero moves, and a
//! highlight can never silently come back empty), Backspace skips the current
//! question, notes live in the shared textarea (no second composer), and the
//! panel renders in the fixed inline viewport instead of a dynamic bottom pane.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tokio::sync::oneshot;

use super::Tui;
use crate::composer::text_area::TextArea;
use crate::setup::icons::icon;
use gray_core::questions::{UserAnswer, UserQuestion};

pub(crate) const AUTO_RESOLUTION_HIDDEN_GRACE: Duration = Duration::from_secs(60);
pub(crate) const AUTO_RESOLUTION_VISIBLE_COUNTDOWN: Duration = Duration::from_secs(60);
const OTHER_OPTION_LABEL: &str = "None of the above";
const OTHER_OPTION_DESCRIPTION: &str = "Optionally, add details in notes (tab).";
const UNANSWERED_CONFIRM_TITLE: &str = "Submit with unanswered questions?";
const TIP_SEPARATOR: &str = " | ";

const ACCENT: Color = Color::Rgb(246, 173, 126);
const TEXT: Color = Color::Rgb(225, 225, 225);
const DIM: Color = Color::Rgb(140, 140, 140);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Focus {
    Options,
    Notes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AutoResolutionTiming {
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

struct AnswerState {
    selected_idx: Option<usize>,
    draft: String,
    answer_committed: bool,
    notes_visible: bool,
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
    answers: Vec<AnswerState>,
        current_idx: usize,
    pub(crate) focus: Focus,
    confirm_unanswered: Option<usize>,
    request_started_at: Instant,
    auto_snoozed: bool,
    last_countdown: Option<String>,
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

    fn current_question(&self) -> &UserQuestion {
        &self.questions[self.current_idx]
    }

    /// Options plus the auto-added "Other" row (codex other_option_enabled).
    fn options_len(&self) -> usize {
        let q = self.current_question();
        q.options.len() + usize::from(q.is_other && !q.options.is_empty())
    }

    fn option_label_for_index(&self, idx: usize) -> Option<String> {
        let q = self.current_question();
        if idx < q.options.len() {
            Some(q.options[idx].label.clone())
        } else if idx == q.options.len() && q.is_other && !q.options.is_empty() {
            Some(OTHER_OPTION_LABEL.to_string())
        } else {
            None
        }
    }

    fn unanswered_count(&self) -> usize {
        self.answers.iter().filter(|a| !a.answer_committed).count()
    }

    fn progress_prefix(&self) -> String {
        let base = format!("Question {}/{}", self.current_idx + 1, self.questions.len());
        match self.unanswered_count() {
            0 => base,
            n => format!("{base} ({n} unanswered)"),
        }
    }

    fn auto_resolution_timing_at(&self, now: Instant) -> AutoResolutionTiming {
        if self.blocking || self.auto_snoozed {
            return AutoResolutionTiming::Disabled;
        }
        let elapsed = now.saturating_duration_since(self.request_started_at);
        if elapsed < AUTO_RESOLUTION_HIDDEN_GRACE {
            return AutoResolutionTiming::HiddenGrace {
                remaining: AUTO_RESOLUTION_HIDDEN_GRACE.saturating_sub(elapsed),
            };
        }
        let visible = elapsed.checked_sub(AUTO_RESOLUTION_HIDDEN_GRACE).unwrap_or_default();
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
                TickOutcome::AutoResolved { answers: Vec::new(), questions, blocking, session_done }
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
            if a.answer_committed && let Some(sel) = a.selected_idx && let Some(label) = self.option_label_for_index_for(idx, sel) {
                list.push(label);
            }
            if a.answer_committed {
                let notes = a.draft.trim();
                if !notes.is_empty() {
                    list.push(format!("user_note: {notes}"));
                }
            }
            out.push(UserAnswer { id: q.id.clone(), answers: list });
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

    pub(crate) fn on_key(&mut self, code: KeyCode, mods: KeyModifiers, ta: &mut TextArea) -> QuestionOutcome {
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
            (KeyCode::Char('n'), KeyModifiers::CONTROL) | (KeyCode::PageDown, KeyModifiers::NONE) => {
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
                KeyCode::Backspace | KeyCode::Delete => {
                    self.skip_current(ta)
                }
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
                    if let Some(idx) = c.to_digit(10).and_then(|d| (d >= 1).then(|| d as usize - 1))
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
                        KeyCode::Up => if cur == 0 { len - 1 } else { cur - 1 },
                        _ => if cur + 1 >= len { 0 } else { cur + 1 },
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
        q.queue.push_back(QueuedRequest { questions, blocking, tx, resolved });
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
    let Some(mut q) = t.active_question.take() else { return };
    match q.on_key(code, mods, &mut t.textarea) {
        QuestionOutcome::None => t.active_question = Some(q),
        QuestionOutcome::Redraw => {
            t.active_question = Some(q);
            let _ = t.draw();
        }
        QuestionOutcome::Resolved { answers, questions, blocking, session_done } => {
            finish_resolution(t, q, answers, questions, blocking, session_done);
        }
    }
}

/// Ticker entry: drives auto-resolution for non-blocking requests.
pub(crate) fn tick_question(t: &mut Tui) {
    let Some(mut q) = t.active_question.take() else { return };
    match q.tick(Instant::now()) {
        TickOutcome::Idle => t.active_question = Some(q),
        TickOutcome::Redraw => {
            t.active_question = Some(q);
            let _ = t.draw();
        }
        TickOutcome::AutoResolved { answers, questions, blocking, session_done } => {
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
        t.pending_question_answers.push(format_answers_text(&questions, &answers));
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
        if let Some(a) = answers.iter().find(|a| a.id == q.id) && !a.answers.is_empty() {
            out.push_str(&format!("\n{} → {}", q.question, a.answers.join(" · ")));
        }
    }
    out
}

/// Resolved-question summary rows: the question on its own row, the outcome
/// (`→ answer` or `→ skipped`) stacked below it — never jammed on one line.
fn result_summary_lines(questions: &[UserQuestion], answers: &[UserAnswer]) -> Vec<String> {
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

fn push_result_summary(t: &mut Tui, questions: &[UserQuestion], answers: &[UserAnswer], _auto: bool) {
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

/// Interactive-mode bridge: parks the questions on the composer overlay and
/// awaits the user's answers. Late answers for non-blocking asks are
/// delivered by the REPL as a follow-up user message.
pub struct ComposerQuestionAsker {
    pub tui: super::SharedTui,
}

impl gray_core::questions::QuestionAsker for ComposerQuestionAsker {
    fn ask(
        &self,
        questions: Vec<UserQuestion>,
        blocking: bool,
    ) -> futures::future::BoxFuture<'static, Result<Vec<UserAnswer>, gray_core::error::CoreError>> {
        let tui = self.tui.clone();
        Box::pin(async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            {
                let mut t = tui.lock().expect("tui lock");
                attach_request(&mut t, questions, blocking, tx);
            }
            match rx.await {
                Ok(answers) => Ok(answers),
                Err(_) => Err(gray_core::error::CoreError::Cancelled),
            }
        })
    }
}

/// Builds the panel lines for the inline viewport (draw side). Rows are
/// capped at `max_rows`; the option window scrolls around the selection.
pub(crate) fn panel_lines(q: &QuestionSession, w: usize, max_rows: usize) -> Vec<Line<'static>> {
    let bg_style = Style::default().bg(Color::Rgb(22, 22, 22));
    let mut lines: Vec<Line<'static>> = Vec::new();
    // top margin — like 4f8cc65 [WORKING WORKING FINAL ULTRA MEGA SUPREME...] padded card box
    lines.push(Line::from("").style(bg_style));
    if q.confirm_unanswered.is_some() {
        let sel = q.confirm_unanswered.unwrap_or(0);
        lines.push(Line::from(Span::styled(
            format!(" {UNANSWERED_CONFIRM_TITLE}"),
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        )));
        let n = q.unanswered_count();
        let rows = [
            ("Submit anyway", format!("Submit with {n} unanswered question{}.", if n == 1 { "" } else { "s" })),
            ("Go back", "Return to the first unanswered question.".to_string()),
        ];
        for (i, (label, desc)) in rows.iter().enumerate() {
            let prefix = if i == sel { icon("arrow") } else { " " };
            lines.push(option_row(prefix, i + 1, label, Some(desc), i == sel));
        }
        lines.push(Line::from("").style(bg_style));
        return lines;
    }

    let mut countdown = String::new();
    if let Some(text) = &q.last_countdown {
        countdown = format!(" · {text}");
    }
    lines.push(Line::from(Span::styled(
        format!(" {}{countdown}", q.progress_prefix()),
        Style::default().fg(DIM),
    )));

    let question_text = format!(" {}", q.current_question().question);
    let wrapped = wrap_plain(&question_text, w);
    let q_lines = wrapped.len();
    lines.extend(wrapped.into_iter().map(|l| {
        Line::from(Span::styled(l, Style::default().fg(TEXT).add_modifier(Modifier::BOLD)))
    }));

    // Budget: top(1) + progress(1) + question + tips(1) + bottom margin(1);
    // rest goes to options.
    // Min 3 options so long questions don't squeeze to 1.
    let budget = max_rows.saturating_sub(4 + q_lines.min(max_rows.saturating_sub(4)));
    let len = q.options_len();
    // Cursor position vs committed pick are different things: the cursor row
    // always shows the arrow and gets accent styling from the start, so the
    // preselected first option reads as highlighted with zero moves — a
    // highlight is the cursor, never a submitted answer (preselect stays
    // unanswered until Enter, Space, a digit, or notes confirm it).
    let picked = q.answers[q.current_idx].selected_idx.is_some();
    let cursor = q.answers[q.current_idx].selected_idx.unwrap_or(0);
    let visible = budget.min(len).max(3.min(len));
    let start = cursor.saturating_sub(visible.saturating_sub(1)).min(cursor);
    for i in start..len.min(start + visible) {
        let is_cursor = i == cursor;
        let prefix = if is_cursor { icon("arrow") } else { " " };
        let label = q.option_label_for_index(i).unwrap_or_default();
        let desc = if i < q.current_question().options.len() {
            Some(q.current_question().options[i].description.clone())
        } else {
            Some(OTHER_OPTION_DESCRIPTION.to_string())
        };
        for row in option_rows(prefix, i + 1, &label, desc.as_deref(), is_cursor && picked, w) {
            lines.push(row);
        }
    }

    lines.push(tips_line(q));
    // bottom margin mirrors the top one — without it the footer jams
    // against "enter to submit".
    lines.push(Line::from("").style(bg_style));
    lines
}

/// One option as wrapped rows: the head row keeps the picked styling, long
/// descriptions wrap onto dim continuation rows instead of clipping off-screen.
fn option_rows(prefix: &str, num: usize, label: &str, desc: Option<&str>, selected: bool, w: usize) -> Vec<Line<'static>> {
    let accent = Style::default().fg(if selected { ACCENT } else { DIM }).add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(if selected { ACCENT } else { TEXT }).add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(DIM);
    let head = format!(" {prefix} {num}. {label}");
    let Some(d) = desc.filter(|d| !d.is_empty()) else {
        return vec![Line::from(vec![
            Span::styled(format!(" {prefix} {num}. "), accent),
            Span::styled(label.to_string(), label_style),
        ])];
    };
    // wrap_plain reserves 4 for padding; desc starts after "head — ".
    let desc_w = w.saturating_sub(head.chars().count() + 3 + 4).max(10);
    let chunks = wrap_plain(d, desc_w + 4);
    let mut rows = vec![Line::from(vec![
        Span::styled(format!(" {prefix} {num}. "), accent),
        Span::styled(label.to_string(), label_style),
        Span::styled(format!(" — {}", chunks.first().cloned().unwrap_or_default()), dim_style),
    ])];
    let indent = " ".repeat(head.chars().count() + 3);
    for c in chunks.iter().skip(1) {
        rows.push(Line::from(Span::styled(format!("{indent}{c}"), dim_style)));
    }
    rows
}

fn option_row(prefix: &str, num: usize, label: &str, desc: Option<&str>, selected: bool) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            format!(" {prefix} {num}. "),
            Style::default().fg(if selected { ACCENT } else { DIM }).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            label.to_string(),
            Style::default().fg(if selected { ACCENT } else { TEXT }).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(d) = desc {
        spans.push(Span::styled(
            format!(" — {d}"),
            Style::default().fg(DIM),
        ));
    }
    Line::from(spans)
}

fn tips_line(q: &QuestionSession) -> Line<'static> {
    let notes_visible = q.answers[q.current_idx].notes_visible || !q.answers[q.current_idx].draft.trim().is_empty();
    let mut tips: Vec<(String, bool)> = Vec::new();
    let sel = q.answers[q.current_idx].selected_idx.is_some();
    if sel && !notes_visible {
        tips.push(("tab to add notes".into(), true));
    } else if sel && notes_visible {
        tips.push(("tab or esc to clear notes".into(), false));
    }
    let is_last = q.current_idx + 1 >= q.questions.len();
    let submit = if q.questions.len() == 1 || is_last {
        "enter picks + submits"
    } else {
        "enter picks + next"
    };
    tips.push(("backspace skips question".into(), false));
    tips.push((submit.into(), true));
    if q.questions.len() > 1 {
        tips.push(("←/→ to change question".into(), false));
    }
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    for (i, (text, highlight)) in tips.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(TIP_SEPARATOR, Style::default().fg(Color::Rgb(80, 80, 80))));
        }
        spans.push(Span::styled(
            text.clone(),
            Style::default().fg(if *highlight { ACCENT } else { DIM }),
        ));
    }
    Line::from(spans)
}

/// Word-aware wrap matching the transcript's `word_flush_cut`: break at the
/// last space in the window, hard-cut only a single overlong word.
fn wrap_plain(s: &str, w: usize) -> Vec<String> {
    let content_w = w.saturating_sub(4).max(1);
    s.split('\n')
        .flat_map(|line| {
            if line.is_empty() {
                return vec![String::new()];
            }
            let chars: Vec<char> = line.chars().collect();
            let mut rows = Vec::new();
            let mut start = 0usize;
            while start < chars.len() {
                let mut end = (start + content_w).min(chars.len());
                if end < chars.len()
                    && let Some(sp) = chars[start..end].iter().rposition(|c| *c == ' ')
                    && sp > 0
                {
                    end = start + sp + 1;
                }
                rows.push(chars[start..end].iter().collect());
                start = end;
            }
            rows
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::questions::UserOption;

    fn mk(questions: usize) -> (QuestionSession, oneshot::Receiver<Vec<UserAnswer>>) {
        let qs: Vec<UserQuestion> = (0..questions)
            .map(|i| UserQuestion {
                id: format!("q{i}"),
                header: "H".into(),
                question: format!("question {i}?"),
                options: vec![
                    UserOption { label: "Alpha".into(), description: "first".into() },
                    UserOption { label: "Beta (Recommended)".into(), description: "second".into() },
                ],
                is_other: true,
            })
            .collect();
        let (tx, rx) = oneshot::channel();
        (QuestionSession::new(qs, true, tx, Arc::new(AtomicBool::new(false))), rx)
    }

    #[test]
    fn digit_selects_and_advances_then_submits() {
        let (mut q, mut rx) = mk(1);
        let mut ta = TextArea::new();
        let out = q.on_key(KeyCode::Char('2'), KeyModifiers::NONE, &mut ta);
        match out {
            QuestionOutcome::Resolved { answers, session_done, .. } => {
                assert!(session_done);
                assert_eq!(answers[0].answers, vec!["Beta (Recommended)".to_string()]);
            }
            _ => panic!("expected resolution"),
        }
        assert_eq!(rx.try_recv().unwrap()[0].answers, vec!["Beta (Recommended)".to_string()]);
    }

    #[test]
    fn notes_flow_appends_user_note() {
        let (mut q, mut rx) = mk(1);
        let mut ta = TextArea::new();
        // select option 1 via space, add notes, submit
        q.on_key(KeyCode::Char(' '), KeyModifiers::NONE, &mut ta);
        q.on_key(KeyCode::Tab, KeyModifiers::NONE, &mut ta);
        ta.insert_str("hurry");
        q.on_key(KeyCode::Enter, KeyModifiers::NONE, &mut ta);
        let answers = rx.try_recv().unwrap();
        assert_eq!(answers[0].answers, vec!["Alpha".to_string(), "user_note: hurry".to_string()]);
    }

    #[test]
    fn fresh_session_preselects_first_option() {
        let (q, _rx) = mk(1);
        assert_eq!(q.answers[0].selected_idx, Some(0));
        assert!(q.answers[0].answer_committed); // preselect is a real commit
    }

    #[test]
    fn enter_commits_highlighted_option() {
        let (mut q, mut rx) = mk(1);
        let mut ta = TextArea::new();
        // Untouched highlight sits on row 0: Enter picks it (never empty).
        let out = q.on_key(KeyCode::Enter, KeyModifiers::NONE, &mut ta);
        assert!(matches!(out, QuestionOutcome::Resolved { .. }));
        assert_eq!(rx.try_recv().unwrap()[0].answers, vec!["Alpha".to_string()]);
    }

    #[test]
    fn backspace_skips_question_without_confirmation() {
        let (mut q, mut rx) = mk(1);
        let mut ta = TextArea::new();
        let out = q.on_key(KeyCode::Backspace, KeyModifiers::NONE, &mut ta);
        assert!(matches!(out, QuestionOutcome::Resolved { .. }));
        assert!(rx.try_recv().unwrap()[0].answers.is_empty());
    }

    #[test]
    fn unanswered_submit_defaults_to_go_back() {
        let (mut q, _rx) = mk(2);
        let mut ta = TextArea::new();
        // Un-commit q1 by moving (arrows un-commit), leave it via keyboard
        // navigation, answer q2: the last question then submits with q1 still
        // open → confirmation.
        q.on_key(KeyCode::Down, KeyModifiers::NONE, &mut ta);
        q.on_key(KeyCode::Char('n'), KeyModifiers::CONTROL, &mut ta);
        let out = q.on_key(KeyCode::Enter, KeyModifiers::NONE, &mut ta);
        assert!(matches!(out, QuestionOutcome::Redraw));
        assert_eq!(q.confirm_unanswered, Some(1)); // Go back is the default.
        // Enter on the default returns to the questions instead of submitting empties.
        let out = q.on_key(KeyCode::Enter, KeyModifiers::NONE, &mut ta);
        assert!(matches!(out, QuestionOutcome::Redraw));
        assert!(q.confirm_unanswered.is_none());
    }

    #[test]
    fn confirm_submit_anyway_needs_explicit_up() {
        let (mut q, mut rx) = mk(2);
        let mut ta = TextArea::new();
        q.on_key(KeyCode::Down, KeyModifiers::NONE, &mut ta);
        q.on_key(KeyCode::Char('n'), KeyModifiers::CONTROL, &mut ta);
        let out = q.on_key(KeyCode::Enter, KeyModifiers::NONE, &mut ta);
        assert!(matches!(out, QuestionOutcome::Redraw));
        // Move off the Go-back default up to Submit anyway, then submit.
        q.on_key(KeyCode::Up, KeyModifiers::NONE, &mut ta);
        let out = q.on_key(KeyCode::Enter, KeyModifiers::NONE, &mut ta);
        match out {
            QuestionOutcome::Resolved { answers, .. } => {
                assert!(answers[0].answers.is_empty());
                assert_eq!(answers[1].answers, vec!["Alpha".to_string()]);
            }
            _ => panic!("expected resolution"),
        }
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn wrap_plain_breaks_at_word_boundaries() {
        // Screenshot case: "warm interior light glowing" was split as "l" / "ight".
        let text = "warm interior light glowing from windows, porch.";
        let rows = wrap_plain(text, 30);
        assert!(rows.len() > 1);
        assert_eq!(rows.concat(), text);
        for row in &rows[..rows.len() - 1] {
            // Mid-word break only allowed for a single overlong word.
            let ends_mid_word = row
                .chars()
                .last()
                .is_some_and(|c| c != ' ')
                && rows
                    .iter()
                    .skip_while(|r| *r != row)
                    .nth(1)
                    .map(|next| next.chars().next().is_some_and(|c| c != ' '))
                    .unwrap_or(false);
            assert!(!ends_mid_word, "mid-word break: {rows:?}");
        }
        assert!(rows.iter().any(|r| r.contains("light")), "light must stay intact: {rows:?}");
    }

    #[test]
    fn cursor_row_highlighted_on_entry() {
        let (q, _rx) = mk(1);
        let lines = panel_lines(&q, 80, 100);
        let head: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let idx = head.iter().position(|t| t.contains("1.") && t.contains("Alpha")).expect("option 1 row");
        let accent = lines[idx]
            .spans
            .iter()
            .any(|s| s.style.fg == Some(ACCENT));
        assert!(accent, "first option must be highlighted on entry: {:?}", head[idx]);
    }

    #[test]
    fn wrapped_option_rows_never_exceed_width() {
        let desc = "a very long description that would previously clip off the edge of the panel ".repeat(4);
        let w = 60;
        let rows = option_rows("›", 1, "Long", Some(&desc), false, w);
        assert!(rows.len() > 1, "long description should wrap onto continuations");
        for row in &rows {
            let text: String = row.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.chars().count() <= w, "option row overflows: {text:?}");
        }
        // Head row keeps label + start of description; continuations are plain.
        let head: String = rows[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(head.contains("Long") && head.contains('—'), "got: {head:?}");
    }

    #[test]
    fn enter_flow_commits_each_question() {
        let (mut q, mut rx) = mk(2);
        let mut ta = TextArea::new();
        // q1: Enter commits the preselected first option and advances.
        let out = q.on_key(KeyCode::Enter, KeyModifiers::NONE, &mut ta);
        assert!(matches!(out, QuestionOutcome::Redraw));
        // q2 (last): Enter commits and submits everything.
        let out = q.on_key(KeyCode::Enter, KeyModifiers::NONE, &mut ta);
        assert!(matches!(out, QuestionOutcome::Resolved { .. }));
        let answers = rx.try_recv().unwrap();
        assert_eq!(answers[0].answers, vec!["Alpha".to_string()]);
        assert_eq!(answers[1].answers, vec!["Alpha".to_string()]);
    }

    #[test]
    fn auto_resolution_times_out_non_blocking_only() {
        let (qb, _rx) = mk(1);
        assert!(!matches!(qb.auto_resolution_timing_at(Instant::now()), AutoResolutionTiming::Due));
        // blocking never auto-resolves
        let (tx, _rx2) = oneshot::channel::<Vec<UserAnswer>>();
        let mut nb = QuestionSession::new(qb.questions.clone(), false, tx, Arc::new(AtomicBool::new(false)));
        nb.request_started_at = Instant::now() - Duration::from_secs(121);
        match nb.tick(Instant::now()) {
            TickOutcome::AutoResolved { answers, session_done, .. } => {
                assert!(answers.is_empty());
                assert!(session_done);
            }
            _ => panic!("auto-resolve should fire"),
        }
    }

    #[test]
    fn queue_promotes_next_request() {
        let (mut q, mut rx1) = mk(1);
        let (tx2, mut rx2) = oneshot::channel();
        q.queue.push_back(QueuedRequest {
            questions: q.questions.clone(),
            blocking: false,
            tx: tx2,
            resolved: Arc::new(AtomicBool::new(false)),
        });
        let mut ta = TextArea::new();
        let out = q.on_key(KeyCode::Char('1'), KeyModifiers::NONE, &mut ta);
        match out {
            QuestionOutcome::Resolved { session_done, blocking, .. } => {
                assert!(!session_done);
                assert!(!blocking);
            }
            _ => panic!("expected resolution"),
        }
        assert!(rx1.try_recv().is_ok());
        // second question is now current; answer it
        q.on_key(KeyCode::Char('2'), KeyModifiers::NONE, &mut ta);
        assert!(rx2.try_recv().is_ok());
    }

    #[test]
    fn panel_lines_end_with_bottom_margin() {
        let (q, _rx) = mk(1);
        let lines = panel_lines(&q, 80, 100);
        assert!(lines.len() > 3, "panel should render, got {}", lines.len());
        let last = lines.last().unwrap();
        assert!(
            last.spans.iter().all(|s| s.content.trim().is_empty()),
            "last panel row must be blank (bottom margin)"
        );
    }

    #[test]
    fn result_summary_stacks_question_and_outcome() {
        let qs = vec![UserQuestion {
            id: "q0".into(),
            header: "H".into(),
            question: "question 0?".into(),
            options: vec![],
            is_other: false,
        }];
        // Skipped stacks like answered: question row, then outcome row.
        assert_eq!(
            result_summary_lines(&qs, &[]),
            vec!["? question 0?".to_string(), "  → skipped".to_string()]
        );
        let answered = vec![UserAnswer { id: "q0".into(), answers: vec!["Alpha".into()] }];
        assert_eq!(
            result_summary_lines(&qs, &answered),
            vec!["? question 0?".to_string(), "  → Alpha".to_string()]
        );
    }
}
