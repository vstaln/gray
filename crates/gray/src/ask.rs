use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    pub description: String,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opt_in: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub options: Vec<AskOption>,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multi: bool,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default_all: bool,
}

impl AskQuestion {
    fn is_multi(&self) -> bool {
        self.multi && !self.options.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AskAnswer {
    pub id: String,
    pub answers: Vec<String>,
}

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

    json_turn: Option<String>,

    demux: std::sync::atomic::AtomicBool,
}

static SERVICE: Mutex<Option<Arc<Service>>> = Mutex::new(None);

pub fn install(tui: Option<crate::composer::SharedTui>, interactive: bool) {
    install_service(tui, interactive, None);
}

pub fn install_json(turn_id: String) {
    install_service(None, false, Some(turn_id));
}

pub fn json_enabled() -> bool {
    std::env::var("GRAY_JSON_ASK").is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn install_service(
    tui: Option<crate::composer::SharedTui>,
    interactive: bool,
    json_turn: Option<String>,
) {
    let svc = Arc::new(Service {
        tui,
        interactive,
        pending: Mutex::new(HashMap::new()),
        next: Mutex::new(0),
        cancel: tokio_util::sync::CancellationToken::new(),
        json_turn,
        demux: std::sync::atomic::AtomicBool::new(false),
    });
    *SERVICE.lock().expect("ask service") = Some(svc);
}

pub fn set_demux() {
    if let Some(svc) = SERVICE.lock().expect("ask service").as_ref() {
        svc.demux.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn deliver_json_answer(ask_id: u64, line: &str) {
    let excerpt = crate::repl::format::truncate_chars(line.trim(), 120);
    let svc = SERVICE.lock().expect("ask service").clone();
    let Some(svc) = svc else {
        log::warn!("ask answer dropped: no ask service (ask_id {ask_id}): {excerpt}");
        return;
    };
    let mut pending = svc.pending.lock().expect("ask pending");
    let Some(p) = pending.get(&ask_id) else {
        log::warn!("ask answer dropped: unknown or closed ask_id {ask_id}: {excerpt}");
        return;
    };
    match parse_json_answer(line, ask_id, &p.questions) {
        Some(answers) => {
            if let Some(p) = pending.remove(&ask_id) {
                let _ = p.tx.send(answers);
            }
        }
        None => log::warn!("ask answer dropped: unparsable line for ask_id {ask_id}: {excerpt}"),
    }
}

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

pub async fn handle_ask(params: serde_json::Value) -> serde_json::Value {
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
        return serde_json::json!({ "answers": {} });
    }
    if svc.cancel.is_cancelled() {
        return serde_json::json!({ "answers": {} });
    }
    let multi_ids: Vec<String> = questions
        .iter()
        .filter(|q| q.is_multi())
        .map(|q| q.id.clone())
        .collect();
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

    let drive_svc = svc.clone();
    let drive = tokio::spawn(async move {
        if drive_svc.demux.load(std::sync::atomic::Ordering::Relaxed)
            && drive_svc.json_turn.is_some()
        {
            let turn = drive_svc.json_turn.clone().expect("json_turn checked");
            let questions = {
                let pending = drive_svc.pending.lock().expect("ask pending");
                match pending.get(&id) {
                    Some(p) => p.questions.clone(),
                    None => return,
                }
            };
            {
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                let _ = writeln!(out, "{}", json_ask_row(&turn, id, &questions));
                let _ = out.flush();
            }
            std::future::pending::<()>().await;
        } else if let Some(turn) = drive_svc.json_turn.clone() {
            drive_json(&drive_svc, id, turn).await;
        } else if drive_svc.interactive && drive_svc.tui.is_some() {
            drive_tui(&drive_svc, id).await;
        } else {
            drive_stdin(&drive_svc, id).await;
        }
    });
    let answers = tokio::select! {
        biased;
        Ok(a) = rx => a,
        _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => Vec::new(),
        _ = svc.cancel.cancelled() => Vec::new(),
        _ = drive => Vec::new(),
    };

    svc.pending.lock().expect("ask pending").remove(&id);
    let mut out = answers_json(&answers);
    if let Some(map) = out.get_mut("answers").and_then(|m| m.as_object_mut()) {
        for qid in &multi_ids {
            if let Some(entry) = map.get_mut(qid) {
                let selected: Vec<serde_json::Value> = entry
                    .get("answers")
                    .and_then(|a| a.as_array())
                    .map(|a| {
                        a.iter()
                            .filter(|v| v.as_str().is_some_and(|s| !s.starts_with("user_note: ")))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                entry["selected"] = serde_json::Value::Array(selected);
            }
        }
    }
    out
}

async fn drive_json(svc: &Arc<Service>, id: u64, turn: String) {
    let svc = svc.clone();
    tokio::task::spawn_blocking(move || {
        use std::io::{BufRead, Write};
        let questions = {
            let pending = svc.pending.lock().expect("ask pending");
            match pending.get(&id) {
                Some(p) => p.questions.clone(),
                None => return,
            }
        };
        {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{}", json_ask_row(&turn, id, &questions));
            let _ = out.flush();
        }
        let mut answers = Vec::new();
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            if let Some(parsed) = parse_json_answer(&line, id, &questions) {
                answers = parsed;
                break;
            }
        }
        let mut pending = svc.pending.lock().expect("ask pending");
        if let Some(p) = pending.remove(&id) {
            let _ = p.tx.send(answers);
        }
    })
    .await
    .ok();
}

fn json_ask_row(turn: &str, id: u64, questions: &[AskQuestion]) -> serde_json::Value {
    serde_json::json!({
        "protocol": 1, "turn_id": turn, "type": "progress", "phase": "ask",
        "ask_id": id, "questions": questions,
    })
}

fn parse_json_answer(line: &str, id: u64, questions: &[AskQuestion]) -> Option<Vec<AskAnswer>> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("ask_id").and_then(|a| a.as_u64()) != Some(id) {
        return None;
    }
    let map = v.get("answers").and_then(|a| a.as_object())?;
    Some(
        questions
            .iter()
            .filter_map(|q| {
                let picked: Vec<String> = map
                    .get(&q.id)?
                    .as_array()?
                    .iter()
                    .filter_map(|a| a.as_str().map(str::to_string))
                    .collect();
                Some(AskAnswer {
                    id: q.id.clone(),
                    answers: picked,
                })
            })
            .collect(),
    )
}

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

    if !stdin_has_data() {
        return Vec::new();
    }
    let stdin = std::io::stdin();
    let mut out = Vec::new();
    for q in questions {
        if q.is_multi() {
            out.push(ask_stdin_multi(&stdin, q));
            continue;
        }
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

fn ask_stdin_multi(stdin: &std::io::Stdin, q: &AskQuestion) -> AskAnswer {
    use std::io::{BufRead, Write};
    println!("Question: {}", q.question);
    for (i, opt) in q.options.iter().enumerate() {
        let mark = if q.default_all && !opt.opt_in {
            "x"
        } else {
            " "
        };
        println!("  [{mark}] {}. {} — {}", i + 1, opt.label, opt.description);
    }
    print!("  pick (numbers, e.g. 1,3; blank = as shown; 0 = none): ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line).unwrap_or(0);
    let t = line.trim();
    let answers: Vec<String> = if read == 0 || t.is_empty() {
        if q.default_all {
            q.options
                .iter()
                .filter(|o| !o.opt_in)
                .map(|o| o.label.clone())
                .collect()
        } else {
            Vec::new()
        }
    } else {
        let mut picked = vec![false; q.options.len()];
        for part in t.split(|c: char| c == ',' || c.is_whitespace()) {
            if let Ok(n) = part.parse::<usize>()
                && n >= 1
                && n <= q.options.len()
            {
                picked[n - 1] = true;
            }
        }
        q.options
            .iter()
            .zip(picked)
            .filter(|(_, p)| *p)
            .map(|(o, _)| o.label.clone())
            .collect()
    };
    AskAnswer {
        id: q.id.clone(),
        answers,
    }
}

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

        let mut pfd = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(&fd),
            events: libc::POLLIN | libc::POLLHUP,
            revents: 0,
        };

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

async fn drive_tui(svc: &Arc<Service>, id: u64) {
    let svc = svc.clone();
    tokio::task::spawn_blocking(move || {
        let Some(tui) = svc.tui.clone() else { return };
        let mut state = AskModalState::new(id);
        {
            let pending = svc.pending.lock().expect("ask pending");
            if let Some(p) = pending.get(&state.id) {
                state.load(p.questions.clone());
            } else {
                return;
            }
        }

        let saved_status = t_lock(&tui).take_status();
        t_lock(&tui).set_status(Some("Question"));
        let answers = run_ask_modal(&tui, &mut state, &svc.cancel).unwrap_or_default();
        {
            let mut t = t_lock(&tui);

            push_ask_summary(&mut t, &state.questions, &answers);
            t.restore_status(saved_status);
            let _ = t.draw();
        }

        let mut pending = svc.pending.lock().expect("ask pending");
        if let Some(p) = pending.remove(&id) {
            let _ = p.tx.send(answers);
        }
    })
    .await
    .ok();
}

pub(crate) struct AskModalState {
    id: u64,
    pub(crate) questions: Vec<AskQuestion>,
    idx: usize,
    cursor: Vec<usize>,
    committed: Vec<bool>,
    notes: Vec<String>,
    notes_open: Vec<bool>,
    checked: Vec<Vec<bool>>,
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
            checked: Vec::new(),
        }
    }

    fn load(&mut self, questions: Vec<AskQuestion>) {
        let n = questions.len();
        self.questions = questions;
        self.cursor = vec![0; n];
        self.committed = vec![true; n];
        self.notes = vec![String::new(); n];
        self.notes_open = vec![false; n];
        self.checked = self
            .questions
            .iter()
            .map(|q| {
                q.options
                    .iter()
                    .map(|o| q.is_multi() && q.default_all && !o.opt_in)
                    .collect()
            })
            .collect();
    }

    fn multi(&self) -> bool {
        self.questions[self.idx].is_multi()
    }

    fn options_len(&self) -> usize {
        if self.multi() {
            return self.questions[self.idx].options.len();
        }
        self.questions[self.idx].options.len() + 1
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
                if q.is_multi() {
                    answers.extend(
                        q.options
                            .iter()
                            .zip(&self.checked[qi])
                            .filter(|(_, c)| **c)
                            .map(|(o, _)| o.label.clone()),
                    );
                } else if self.committed[qi] {
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
    let multi = state.multi();
    for i in 0..state.options_len() {
        let picked = if multi {
            if state.checked[state.idx][i] {
                "[x]"
            } else {
                "[ ]"
            }
        } else if state.committed[state.idx] && i == state.cursor[state.idx] {
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
        if multi {
            "Space toggle · Tab notes · Enter confirm · Esc skip".to_string()
        } else {
            "Tab notes · Enter submit · Esc skip".to_string()
        }
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
        cursor: state.cursor[state.idx] + 1,
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
    cancel: &tokio_util::sync::CancellationToken,
) -> Option<Vec<AskAnswer>> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read};
    if state.questions.is_empty() {
        clear_ask_modal(tui);
        return Some(Vec::new());
    }
    draw_ask_modal(tui, state);
    loop {
        if cancel.is_cancelled() {
            clear_ask_modal(tui);
            return None;
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
            return None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            clear_ask_modal(tui);
            return None;
        }
        let mut submit = false;
        {
            let n = state.questions.len();
            let i = state.idx;
            let multi = state.multi();
            if let Some(up) = crate::keymap::select_move(&key) {
                state.cursor[i] = if up {
                    state.cursor[i].saturating_sub(1)
                } else {
                    (state.cursor[i] + 1).min(state.options_len() - 1)
                };
                if !multi {
                    state.committed[i] = false;
                }
                draw_ask_modal(tui, state);
                continue;
            }
            match key.code {
                KeyCode::Char(' ') if multi && !state.notes_open[i] => {
                    let c = state.cursor[i];
                    state.checked[i][c] = !state.checked[i][c];
                }
                KeyCode::Char(c)
                    if multi
                        && !state.notes_open[i]
                        && c.to_digit(10).is_some_and(|d| {
                            (1..=9).contains(&d) && (d as usize) <= state.options_len()
                        }) =>
                {
                    let d = c.to_digit(10).unwrap_or(1) as usize - 1;
                    state.cursor[i] = d;
                    state.checked[i][d] = !state.checked[i][d];
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
                        state.committed[i] = false;
                        state.notes[i].clear();
                        state.checked[i].iter_mut().for_each(|c| *c = false);
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
                    if let Some(d) = c.to_digit(10)
                        && !state.notes_open[i]
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
                return Some(answers);
            }
        }
    }
}

pub(crate) fn ask_questions_modal(
    tui: &crate::composer::SharedTui,
    questions: Vec<AskQuestion>,
) -> Option<Vec<AskAnswer>> {
    let mut state = AskModalState::new(0);
    state.load(questions);
    let cancel = tokio_util::sync::CancellationToken::new();
    let answers = run_ask_modal(tui, &mut state, &cancel);
    {
        let mut t = t_lock(tui);
        if let Some(answers) = &answers {
            push_ask_summary(&mut t, &state.questions, answers);
        }
        let _ = t.draw();
    }
    answers
}

fn push_ask_summary(
    t: &mut crate::composer::Tui,
    questions: &[AskQuestion],
    answers: &[AskAnswer],
) {
    if questions.is_empty() {
        return;
    }
    t.ensure_gap();
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
    t.ensure_gap();
}

#[path = "ask_tests.rs"]
#[cfg(test)]
mod tests;
