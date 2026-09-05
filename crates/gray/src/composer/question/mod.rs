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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

mod panel;
mod session;

pub(crate) use panel::panel_lines;
pub(crate) use session::{QuestionSession, attach_request, handle_question_key, tick_question};

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
    ) -> futures::future::BoxFuture<'static, Result<Vec<UserAnswer>, gray_core::error::CoreError>>
    {
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

#[cfg(test)]
mod tests {
    use super::panel::{option_rows, wrap_plain};
    use super::session::{
        AutoResolutionTiming, QuestionOutcome, QueuedRequest, TickOutcome, result_summary_lines,
    };
    use super::*;
    use gray_core::questions::UserOption;

    fn mk(questions: usize) -> (QuestionSession, oneshot::Receiver<Vec<UserAnswer>>) {
        let qs: Vec<UserQuestion> = (0..questions)
            .map(|i| UserQuestion {
                id: format!("q{i}"),
                header: "H".into(),
                question: format!("question {i}?"),
                options: vec![
                    UserOption {
                        label: "Alpha".into(),
                        description: "first".into(),
                    },
                    UserOption {
                        label: "Beta (Recommended)".into(),
                        description: "second".into(),
                    },
                ],
                is_other: true,
            })
            .collect();
        let (tx, rx) = oneshot::channel();
        (
            QuestionSession::new(qs, true, tx, Arc::new(AtomicBool::new(false))),
            rx,
        )
    }

    #[test]
    fn digit_selects_and_advances_then_submits() {
        let (mut q, mut rx) = mk(1);
        let mut ta = TextArea::new();
        let out = q.on_key(KeyCode::Char('2'), KeyModifiers::NONE, &mut ta);
        match out {
            QuestionOutcome::Resolved {
                answers,
                session_done,
                ..
            } => {
                assert!(session_done);
                assert_eq!(answers[0].answers, vec!["Beta (Recommended)".to_string()]);
            }
            _ => panic!("expected resolution"),
        }
        assert_eq!(
            rx.try_recv().unwrap()[0].answers,
            vec!["Beta (Recommended)".to_string()]
        );
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
        assert_eq!(
            answers[0].answers,
            vec!["Alpha".to_string(), "user_note: hurry".to_string()]
        );
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
            let ends_mid_word = row.chars().last().is_some_and(|c| c != ' ')
                && rows
                    .iter()
                    .skip_while(|r| *r != row)
                    .nth(1)
                    .map(|next| next.chars().next().is_some_and(|c| c != ' '))
                    .unwrap_or(false);
            assert!(!ends_mid_word, "mid-word break: {rows:?}");
        }
        assert!(
            rows.iter().any(|r| r.contains("light")),
            "light must stay intact: {rows:?}"
        );
    }

    #[test]
    fn cursor_row_highlighted_on_entry() {
        let (q, _rx) = mk(1);
        let lines = panel_lines(&q, 80, 100);
        let head: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let idx = head
            .iter()
            .position(|t| t.contains("1.") && t.contains("Alpha"))
            .expect("option 1 row");
        let accent = lines[idx].spans.iter().any(|s| s.style.fg == Some(ACCENT));
        assert!(
            accent,
            "first option must be highlighted on entry: {:?}",
            head[idx]
        );
    }

    #[test]
    fn wrapped_option_rows_never_exceed_width() {
        let desc = "a very long description that would previously clip off the edge of the panel "
            .repeat(4);
        let w = 60;
        let rows = option_rows("›", 1, "Long", Some(&desc), false, w);
        assert!(
            rows.len() > 1,
            "long description should wrap onto continuations"
        );
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
        assert!(!matches!(
            qb.auto_resolution_timing_at(Instant::now()),
            AutoResolutionTiming::Due
        ));
        // blocking never auto-resolves
        let (tx, _rx2) = oneshot::channel::<Vec<UserAnswer>>();
        let mut nb = QuestionSession::new(
            qb.questions.clone(),
            false,
            tx,
            Arc::new(AtomicBool::new(false)),
        );
        nb.request_started_at = Instant::now() - Duration::from_secs(121);
        match nb.tick(Instant::now()) {
            TickOutcome::AutoResolved {
                answers,
                session_done,
                ..
            } => {
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
            QuestionOutcome::Resolved {
                session_done,
                blocking,
                ..
            } => {
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
        let answered = vec![UserAnswer {
            id: "q0".into(),
            answers: vec!["Alpha".into()],
        }];
        assert_eq!(
            result_summary_lines(&qs, &answered),
            vec!["? question 0?".to_string(), "  → Alpha".to_string()]
        );
    }
}
