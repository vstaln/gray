//! `host/ask` — blocking user questions from sidecar plugins (`questions`,
//! `permissions`). One [`AskService`] per process, installed by [`install`]
//! at REPL boot; [`default_handler`] routes `host/ask` to it.
//!
//! Surfaces (same question, different I/O):
//! - interactive TTY: inline TUI modal (digits pick, Tab = notes, Enter =
//!   submit; Esc resolves empty = decline/skip, never a hang). Rendered
//!   through the composer's `ask_modal` slot above the input box — no
//!   alternate screen, no stdin fight (the turn key watcher is stopped
//!   while the modal owns the keys).
//! - piped stdin: number-or-free-text per question (the deleted
//!   `StdinQuestionAsker` semantics, verbatim).
//! - headless (no TTY, no stdin): empty immediately. Approvals fail closed
//!   downstream; `request_user_input` reports "no user reachable".
//!
//! Timeouts: the plugin enforces its own TTL (300s); the handler TTL here
//! is 300s too (mirrors `ASK_HANDLER_TTL` in `gray-plugin`). The service
//! never outlives the turn: [`shutdown`] (cancel token) resolves every
//! pending ask empty so a replaced agent can't strand a sidecar past 330s.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// One selectable option (mirrors the `gray-questions` contract).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    pub description: String,
}

/// One question (mirrors the `gray-questions` contract).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<AskOption>,
}

/// Answers for one question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AskAnswer {
    pub id: String,
    pub answers: Vec<String>,
}

/// Parse `host/ask` params. Strict on shape (a confused plugin must not
/// become a prompt), lenient on content (empty question list resolves
/// empty — the deleted zero-question guard, verbatim).
fn parse_params(v: &serde_json::Value) -> anyhow::Result<(Vec<AskQuestion>, bool)> {
    let questions: Vec<AskQuestion> =
        serde_json::from_value(v.get("questions").cloned().unwrap_or(serde_json::json!([])))
            .map_err(|e| anyhow::anyhow!("host/ask: bad questions: {e}"))?;
    if questions.len() > 3 {
        anyhow::bail!("host/ask: at most 3 questions");
    }
    let blocking = v.get("blocking").and_then(|b| b.as_bool()).unwrap_or(true);
    Ok((questions, blocking))
}

fn answers_json(answers: &[AskAnswer]) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = answers
        .iter()
        .map(|a| (a.id.clone(), serde_json::json!({ "answers": a.answers })))
        .collect();
    serde_json::json!({ "answers": map })
}

/// Pending ask state, shared between the handler future and the UI driver.
struct PendingAsk {
    questions: Vec<AskQuestion>,
    tx: tokio::sync::oneshot::Sender<Vec<AskAnswer>>,
}

struct Service {
    tui: Option<crate::composer::SharedTui>,
    interactive: bool,
    pending: Mutex<HashMap<u64, PendingAsk>>,
    next: Mutex<u64>,
    cancel: tokio_util::sync::CancellationToken,
}

static SERVICE: Mutex<Option<Arc<Service>>> = Mutex::new(None);

/// Install the process ask service. Called once at REPL boot (before the
/// first turn) and in `-p`/headless paths with `tui: None`.
pub fn install(tui: Option<crate::composer::SharedTui>, interactive: bool) {
    let svc = Arc::new(Service {
        tui,
        interactive,
        pending: Mutex::new(HashMap::new()),
        next: Mutex::new(0),
        cancel: tokio_util::sync::CancellationToken::new(),
    });
    *SERVICE.lock().expect("ask service") = Some(svc);
}

/// True while an ask modal owns the viewport keys (the turn key watcher
/// must not consume them — see `key_watcher.rs`).
pub(crate) fn is_ask_live() -> bool {
    SERVICE
        .lock()
        .expect("ask service")
        .as_ref()
        .is_some_and(|svc| {
            svc.tui
                .as_ref()
                .is_some_and(|t| t.lock().map(|t| t.ask_modal.is_some()).unwrap_or(false))
        })
}

/// Resolve every in-flight ask empty (turn replaced / process exiting).
pub fn shutdown() {
    let mut guard = SERVICE.lock().expect("ask service");
    if let Some(svc) = guard.take() {
        svc.cancel.cancel();
        let mut pending = svc.pending.lock().expect("ask pending");
        for (_, ask) in pending.drain() {
            let _ = ask.tx.send(Vec::new());
        }
    }
}

/// Route one `host/ask` call: parse, surface, await answers (300s TTL).
pub async fn handle_ask(params: serde_json::Value) -> serde_json::Value {
    // Service check first: a missing install is a host bug, always loud —
    // even for empty/nonblocking params (which otherwise short-circuit).
    let svc = SERVICE.lock().expect("ask service").clone();
    let Some(svc) = svc else {
        return serde_json::json!({"error": "host/ask: ask service not installed"});
    };
    let (questions, blocking) = match parse_params(&params) {
        Ok(p) => p,
        Err(e) => return serde_json::json!({"error": format!("{e:#}")}),
    };
    if questions.is_empty() {
        return serde_json::json!({ "answers": {} });
    }
    if !blocking {
        // v1: non-blocking resolves empty immediately (no follow-up
        // message injection — documented in docs/plugins.md).
        return serde_json::json!({ "answers": {} });
    }
    if svc.cancel.is_cancelled() {
        return serde_json::json!({ "answers": {} });
    }
    let id = {
        let mut n = svc.next.lock().expect("ask id");
        *n += 1;
        *n
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    svc.pending
        .lock()
        .expect("ask pending")
        .insert(id, PendingAsk { questions, tx });
    // Drive the UI: whichever surface answers first wins (oneshot send).
    if svc.interactive && svc.tui.is_some() {
        drive_tui(&svc, id).await;
    } else {
        drive_stdin(&svc, id).await;
    }
    svc.pending.lock().expect("ask pending").remove(&id);
    let answers = tokio::select! {
        Ok(a) = rx => a,
        _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => Vec::new(),
        _ = svc.cancel.cancelled() => Vec::new(),
    };
    answers_json(&answers)
}

/// Piped/headless surface: one stdin line per question (number picks an
/// option, free text becomes a note, EOF/blank → skip). Runs on a blocking
/// thread — never on the turn's async runtime. Headless (stdin without data
/// and not a TTY) resolves empty immediately. The pending entry stays in
/// the map until `handle_ask` removes it after the race, so the 300s
/// timeout still applies while stdin blocks.
async fn drive_stdin(svc: &Arc<Service>, id: u64) {
    let svc = svc.clone();
    tokio::task::spawn_blocking(move || {
        let questions = {
            let pending = svc.pending.lock().expect("ask pending");
            match pending.get(&id) {
                Some(p) => p.questions.clone(),
                None => return,
            }
        };
        let answers = ask_stdin(&questions);
        let mut pending = svc.pending.lock().expect("ask pending");
        if let Some(p) = pending.remove(&id) {
            let _ = p.tx.send(answers);
        }
    })
    .await
    .ok();
}

fn ask_stdin(questions: &[AskQuestion]) -> Vec<AskAnswer> {
    use std::io::{BufRead, Write};
    // Headless fast path: stdin is not a TTY and has no data ready — do
    // not block a daemon turn on a human that will never arrive. A
    // best-effort poll: readable stdin (pipe with data, TTY user) proceeds
    // to the per-question prompts below.
    if !stdin_has_data() {
        return Vec::new();
    }
    let stdin = std::io::stdin();
    let mut out = Vec::new();
    for q in questions {
        println!("Question: {}", q.question);
        for (i, opt) in q.options.iter().enumerate() {
            println!("  {}. {} — {}", i + 1, opt.label, opt.description);
        }
        println!(
            "  {}. None of the above — Optionally, add details in notes (tab).",
            q.options.len() + 1
        );
        print!("  answer (number, or free text; blank = skip): ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            out.push(AskAnswer {
                id: q.id.clone(),
                answers: vec!["no answer — use your best judgement".to_string()],
            });
            continue;
        }
        let t = line.trim();
        let answers = match t.parse::<usize>() {
            Ok(n) if n >= 1 && n <= q.options.len() => vec![q.options[n - 1].label.clone()],
            Ok(n) if n == q.options.len() + 1 => vec!["None of the above".to_string()],
            _ if t.is_empty() => Vec::new(),
            _ => vec![format!("user_note: {t}")],
        };
        out.push(AskAnswer {
            id: q.id.clone(),
            answers,
        });
    }
    out
}

/// True when stdin is worth blocking on: a TTY (a human may type), or a
/// pipe with bytes already available. Non-blocking poll only — never waits.
fn stdin_has_data() -> bool {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsFd;
        let stdin = std::io::stdin();
        let fd = stdin.as_fd();
        // poll(0): data (or HUP) ready → proceed; nothing → empty now.
        let mut pfd = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(&fd),
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        };
        // `libc` is already a workspace dependency (gray-tools).
        let r = unsafe { libc::poll(&mut pfd, 1, 0) };
        r > 0
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn t_lock(tui: &crate::composer::SharedTui) -> std::sync::MutexGuard<'_, crate::composer::Tui> {
    tui.lock().unwrap_or_else(|e| e.into_inner())
}

/// Interactive surface: inline TUI modal owning the viewport until submit.
/// Runs on a blocking thread with short TUI locks (same discipline as the
/// turn key watcher it replaces for the duration).
async fn drive_tui(svc: &Arc<Service>, id: u64) {
    let svc = svc.clone();
    tokio::task::spawn_blocking(move || {
        let Some(tui) = svc.tui.clone() else { return };
        let mut state = AskModalState::new(id);
        // Park the live turn status; restore it on exit.
        let saved_status = t_lock(&tui).take_status();
        t_lock(&tui).set_status(Some("Question"));
        let answers = run_ask_modal(&tui, &mut state, &svc);
        {
            let mut t = t_lock(&tui);
            // Transcript summary (the deleted stacked Q&A replay, verbatim shape).
            push_ask_summary(&mut t, &state.questions, &answers);
            t.restore_status(saved_status);
            let _ = t.draw();
        }
        // Resolve the handler future (timeout arm may have fired first —
        // second send is a silent no-op).
        let mut pending = svc.pending.lock().expect("ask pending");
        if let Some(p) = pending.remove(&id) {
            let _ = p.tx.send(answers);
        }
    })
    .await
    .ok();
}

/// Modal state: one cursor per question, sequential flow (answer q1,
/// advance; Backspace skips). Option 1 preselected AND committed (the
/// deleted overlay's semantics: Enter submits with zero moves, highlight
/// can never silently come back empty).
pub(crate) struct AskModalState {
    id: u64,
    pub(crate) questions: Vec<AskQuestion>,
    idx: usize,
    cursor: Vec<usize>,
    committed: Vec<bool>,
    notes: Vec<String>,
    notes_open: Vec<bool>,
}

impl AskModalState {
    fn new(id: u64) -> Self {
        Self {
            id,
            questions: Vec::new(),
            idx: 0,
            cursor: Vec::new(),
            committed: Vec::new(),
            notes: Vec::new(),
            notes_open: Vec::new(),
        }
    }

    fn load(&mut self, questions: Vec<AskQuestion>) {
        let n = questions.len();
        self.questions = questions;
        self.cursor = vec![0; n];
        self.committed = vec![true; n];
        self.notes = vec![String::new(); n];
        self.notes_open = vec![false; n];
    }

    fn options_len(&self) -> usize {
        self.questions[self.idx].options.len() + 1 // + Other
    }

    fn option_label(&self, q: usize, i: usize) -> &str {
        self.questions[q]
            .options
            .get(i)
            .map(|o| o.label.as_str())
            .unwrap_or("None of the above")
    }

    fn finish(&self) -> Vec<AskAnswer> {
        self.questions
            .iter()
            .enumerate()
            .map(|(qi, q)| {
                let mut answers = Vec::new();
                if self.committed[qi] {
                    answers.push(self.option_label(qi, self.cursor[qi]).to_string());
                }
                let note = self.notes[qi].trim();
                if !note.is_empty() {
                    answers.push(format!("user_note: {note}"));
                }
                AskAnswer {
                    id: q.id.clone(),
                    answers,
                }
            })
            .collect()
    }
}

/// One rendered modal row: (glyph, label, description).
pub(crate) fn ask_rows(state: &AskModalState) -> Vec<(String, String, String)> {
    let mut rows: Vec<(String, String, String)> = Vec::new();
    if state.questions.is_empty() {
        return rows;
    }
    let q = &state.questions[state.idx];
    rows.push((
        "❓".to_string(),
        format!("Q{}/{} {}", state.idx + 1, state.questions.len(), q.header),
        q.question.clone(),
    ));
    for i in 0..state.options_len() {
        let picked = if state.committed[state.idx] && i == state.cursor[state.idx] {
            "●"
        } else {
            "○"
        };
        let desc = q
            .options
            .get(i)
            .map(|o| o.description.as_str())
            .unwrap_or("add details in notes (tab)");
        rows.push((
            picked.to_string(),
            format!("{}. {}", i + 1, state.option_label(state.idx, i)),
            desc.to_string(),
        ));
    }
    let notes = if state.notes[state.idx].is_empty() {
        "Tab notes · Enter submit · Esc skip".to_string()
    } else {
        format!("notes: {}", state.notes[state.idx])
    };
    rows.push(("…".to_string(), "notes".to_string(), notes));
    rows
}

fn draw_ask_modal(tui: &crate::composer::SharedTui, state: &AskModalState) {
    let mut t = t_lock(tui);
    t.ask_modal = Some(crate::composer::AskModal {
        rows: ask_rows(state),
        cursor: state.cursor[state.idx] + 1, // +1 past the header row
    });
    let _ = t.draw();
}

fn clear_ask_modal(tui: &crate::composer::SharedTui) {
    let mut t = t_lock(tui);
    t.ask_modal = None;
    let _ = t.draw();
}

fn run_ask_modal(
    tui: &crate::composer::SharedTui,
    state: &mut AskModalState,
    svc: &Arc<Service>,
) -> Vec<AskAnswer> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read};
    // Load the questions under one lock, then loop unlocked on poll.
    {
        let pending = svc.pending.lock().expect("ask pending");
        if let Some(p) = pending.get(&state.id) {
            state.load(p.questions.clone());
        } else {
            return Vec::new();
        }
    }
    if state.questions.is_empty() {
        clear_ask_modal(tui);
        return Vec::new();
    }
    draw_ask_modal(tui, state);
    loop {
        if svc.cancel.is_cancelled() {
            clear_ask_modal(tui);
            return Vec::new();
        }
        match poll(std::time::Duration::from_millis(100)) {
            Ok(true) => {}
            _ => continue,
        }
        let Ok(ev) = read() else { continue };
        let Event::Key(key) = ev else { continue };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.code == KeyCode::Esc {
            clear_ask_modal(tui);
            return Vec::new(); // Esc always cancels (decline/skip downstream)
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            clear_ask_modal(tui);
            return Vec::new();
        }
        let mut submit = false;
        {
            let n = state.questions.len();
            let i = state.idx;
            match key.code {
                KeyCode::Up => {
                    state.cursor[i] = state.cursor[i].saturating_sub(1);
                    state.committed[i] = false;
                }
                KeyCode::Down => {
                    state.cursor[i] = (state.cursor[i] + 1).min(state.options_len() - 1);
                    state.committed[i] = false;
                }
                KeyCode::Char(' ') => {
                    state.committed[i] = true;
                }
                KeyCode::Tab => {
                    state.notes_open[i] = !state.notes_open[i];
                }
                KeyCode::Backspace => {
                    if state.notes_open[i] && !state.notes[i].is_empty() {
                        state.notes[i].pop();
                    } else {
                        // Skip explicitly (the deleted Backspace-skips semantic).
                        state.committed[i] = false;
                        state.notes[i].clear();
                        if i + 1 < n {
                            state.idx += 1;
                        } else {
                            submit = true;
                        }
                    }
                }
                KeyCode::Enter => {
                    if i + 1 < n {
                        state.idx += 1;
                    } else {
                        submit = true;
                    }
                }
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    // Digits pick directly; other text goes to notes
                    // (opening them implicitly, like the deleted overlay's
                    // "typing jumps into notes").
                    if let Some(d) = c.to_digit(10)
                        && (1..=9).contains(&d)
                        && (d as usize) <= state.options_len()
                    {
                        state.cursor[i] = d as usize - 1;
                        state.committed[i] = true;
                        if i + 1 < n {
                            state.idx += 1;
                        } else {
                            submit = true;
                        }
                    } else {
                        state.notes_open[i] = true;
                        state.notes[i].push(c);
                    }
                }
                _ => {}
            }
            let answers = if submit { Some(state.finish()) } else { None };
            draw_ask_modal(tui, state);
            if let Some(answers) = answers {
                clear_ask_modal(tui);
                return answers;
            }
        }
    }
}

/// Stacked Q&A replay (`? question` / `→ answer`, verbatim shape).
fn push_ask_summary(
    t: &mut crate::composer::Tui,
    questions: &[AskQuestion],
    answers: &[AskAnswer],
) {
    if questions.is_empty() {
        return;
    }
    t.ensure_gap(1);
    for q in questions {
        t.push_dim(format!("? {}", q.question));
        let joined = answers
            .iter()
            .find(|a| a.id == q.id)
            .map(|a| a.answers.join(" · "))
            .unwrap_or_default();
        if joined.is_empty() {
            t.push_dim("  → skipped".to_string());
        } else {
            t.push_dim(format!("  → {joined}"));
        }
    }
    t.ensure_gap(1);
}

/// Current working dir for `host/run` parity (handler pins it at install).
pub fn run_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[path = "ask_tests.rs"]
#[cfg(test)]
mod tests;
