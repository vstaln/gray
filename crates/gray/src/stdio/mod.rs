//! `gray --stdio`: a persistent newline-delimited-JSON session protocol.
//!
//! One long-lived `gray` process drives the same engine the interactive REPL
//! uses — `repl::prompt_turn::run_prompt_turn` for turns and
//! `repl::dispatch::dispatch_command` for slash commands — while embedding
//! surfaces (gray-gui) speak NDJSON on stdin/stdout instead of a terminal.
//!
//! Wire shape is documented in `docs/stdio-protocol.md`. In short: requests
//! arrive as `{"type": ..., "id": ...}`; every emitted row carries
//! `protocol`, `seq`, `session_id`, `req` (the request id that produced it)
//! and `turn_id` (when inside a turn).

pub(crate) mod events;
mod requests;

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::Config;
use crate::repl::{self, SessionState, SessionTotals};
use crate::session_store::{JsonlSessionStore, SessionId, SessionMeta, default_root};
use gray_core::agent::Agent;
use gray_core::event::Usage;
use gray_core::message::Message;

pub(crate) use events::{emit, emit_ask, emit_say, emit_say_lines};

static ACTIVE: AtomicBool = AtomicBool::new(false);
pub(crate) static SEQ: AtomicU64 = AtomicU64::new(0);
pub(crate) static OUT: Mutex<()> = Mutex::new(());

pub(crate) enum StdioTurnState {
    Idle,
    Running { turn_id: String, interrupted: bool },
}

pub(crate) static CURRENT_REQ: Mutex<Option<Value>> = Mutex::new(None);
pub(crate) static TURN_STATE_SLOT: Mutex<StdioTurnState> = Mutex::new(StdioTurnState::Idle);
pub(crate) static CURRENT_SESSION: Mutex<Option<String>> = Mutex::new(None);
pub(crate) static LAST_USAGE: Mutex<Option<Usage>> = Mutex::new(None);

pub(crate) fn is_turn_active() -> bool {
    matches!(*lock(&TURN_STATE_SLOT), StdioTurnState::Running { .. })
}

pub(crate) fn current_turn_id() -> Option<String> {
    match &*lock(&TURN_STATE_SLOT) {
        StdioTurnState::Running { turn_id, .. } => Some(turn_id.clone()),
        StdioTurnState::Idle => None,
    }
}

pub(crate) fn mark_turn_interrupted() {
    if let StdioTurnState::Running { interrupted, .. } = &mut *lock(&TURN_STATE_SLOT) {
        *interrupted = true;
    }
}

fn begin_turn_state(turn_id: String) {
    *lock(&TURN_STATE_SLOT) = StdioTurnState::Running {
        turn_id,
        interrupted: false,
    };
}

fn end_turn_state() -> bool {
    match std::mem::replace(&mut *lock(&TURN_STATE_SLOT), StdioTurnState::Idle) {
        StdioTurnState::Running { interrupted, .. } => interrupted,
        StdioTurnState::Idle => false,
    }
}

/// Inputs received while a turn runs. Mirrors the REPL's `queued_inputs`:
/// the agent's steer hook drains them mid-turn (`take_steer`), and whatever
/// is left after the turn becomes the next turn/command (`drain_queued`).
/// The trailing `Value` is the original request id for `req` correlation.
pub(crate) static STEER_QUEUE: Mutex<VecDeque<(String, Vec<PathBuf>, Value)>> =
    Mutex::new(VecDeque::new());

/// Guard so a poisoned lock never takes the protocol down.
pub(crate) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Agent steer hook (wired in `run_prompt_turn`): pop the next queued input
/// mid-turn. Like the REPL's `take_steerable`, inputs carrying attachments
/// are left for post-turn handling.
pub(crate) fn take_steer() -> Option<String> {
    let item = {
        let mut q = lock(&STEER_QUEUE);
        match q.front() {
            Some((_, images, _)) if images.is_empty() => q.pop_front(),
            _ => None,
        }
    };
    let (text, _, req) = item?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return None;
    }
    // The queued request's terminal result: it resolved by being injected
    // into the running turn as a steer (the `steered` event names the turn).
    emit_result(&req, true, json!({"steered": true}));
    Some(text)
}

fn push_queued(text: String, images: Vec<PathBuf>, req: Value) {
    lock(&STEER_QUEUE).push_back((text, images, req));
}

/// Mutable session state shared by the request handlers — the same set the
/// REPL main loop threads through `run_prompt_turn`/`dispatch_command`.
pub(crate) struct Session {
    pub agent: Option<Agent>,
    pub state: Option<SessionState>,
    pub totals: SessionTotals,
    pub pending_history: std::sync::Arc<Vec<Message>>,
    pub pending_command: Option<repl::ReplCommand>,
    pub unconfigured: bool,
    pub hide_thinking: bool,
    pub resumable: bool,
    pub quit: bool,
}

impl Session {
    fn new(unconfigured: bool, hide_thinking: bool) -> Self {
        Self {
            agent: None,
            state: None,
            totals: SessionTotals::default(),
            pending_history: std::sync::Arc::new(Vec::new()),
            pending_command: None,
            unconfigured,
            hide_thinking,
            resumable: false,
            quit: false,
        }
    }

    fn sid(&self) -> Option<String> {
        self.state
            .as_ref()
            .map(|s| s.session_id.as_str().to_string())
    }
}

fn sync_session(sess: &Session) {
    *lock(&CURRENT_SESSION) = sess.sid();
}

fn parse_attachments(v: &Value) -> Vec<PathBuf> {
    v.get("attachments")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Normalize a `command` line: REPL commands start with '/', so a bare
/// "help" becomes "/help". Anything else passes through verbatim.
fn command_line(v: &Value) -> String {
    let line = v.get("line").and_then(|l| l.as_str()).unwrap_or("").trim();
    if line.is_empty() {
        String::new()
    } else if line.starts_with('/') {
        line.to_string()
    } else {
        format!("/{line}")
    }
}

fn emit_result(id: &Value, ok: bool, extra: Value) {
    let mut row = json!({"type": "result", "id": id, "ok": ok});
    if let (Some(dst), Some(src)) = (row.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    emit(row);
}

/// Routing on the stdin-reader thread. Turn-control verbs are serviced
/// inline so they work *while* a turn is running; everything else goes to
/// the main loop. A prompt/command/steer arriving mid-turn is queued like
/// typed input in the REPL (the agent may consume it as a steer, else it
/// runs as the next turn).
fn route_incoming(tx: &tokio::sync::mpsc::UnboundedSender<Value>, v: Value) {
    let ty = v
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let id = v.get("id").cloned().unwrap_or(Value::Null);
    match ty.as_str() {
        "interrupt" => requests::interrupt(&id),
        "approve" => requests::approve(&v, &id),
        "steer" => {
            let text = v
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string();
            let images = parse_attachments(&v);
            if text.trim().is_empty() && images.is_empty() {
                emit_result(&id, false, json!({"error": "steer needs text"}));
                return;
            }
            push_queued(text, images, id.clone());
            emit_result(&id, true, json!({"queued": true}));
        }
        "prompt" | "command" if is_turn_active() => {
            let text = if ty == "prompt" {
                v.get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string()
            } else {
                command_line(&v)
            };
            let images = if ty == "prompt" {
                parse_attachments(&v)
            } else {
                Vec::new()
            };
            push_queued(text, images, id.clone());
            emit_result(&id, true, json!({"queued": true}));
        }
        _ => {
            let _ = tx.send(v);
        }
    }
}

/// One turn end-to-end: events stream via the `dispatch_agent_event` stdio
/// hook inside `run_prompt_turn`, then a `turn_end` + `result` close it out.
async fn run_turn(
    sess: &mut Session,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &repl::TuiOpt,
    req: Value,
    text: String,
    images: Vec<PathBuf>,
) -> anyhow::Result<()> {
    let turn_id = format!(
        "turn-{:x}-{:x}",
        gray_core::time::now_millis(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    *lock(&CURRENT_REQ) = Some(req.clone());
    begin_turn_state(turn_id.clone());

    emit(json!({"type": "turn_start", "turn_id": turn_id, "prompt": text}));

    if config.model.is_none() && sess.agent.is_none() {
        emit_say(
            "error",
            "no provider configured — connect one with set_model (list_models shows what's available)",
        );
        emit(json!({"type": "turn_end", "turn_id": turn_id, "ok": false, "error": "no_provider"}));
        emit_result(&req, false, json!({"error": "no_provider"}));
        let _ = end_turn_state();
        *lock(&CURRENT_REQ) = None;
        return Ok(());
    }

    let mut images = images;
    let r = repl::prompt_turn::run_prompt_turn(
        text,
        &mut images,
        &mut sess.agent,
        config,
        cwd,
        tui,
        false,
        &mut sess.state,
        &mut sess.totals,
        &mut sess.pending_command,
        &mut sess.pending_history,
        &mut sess.unconfigured,
        &mut sess.resumable,
    )
    .await;

    let interrupted = end_turn_state();
    if let Err(e) = &r {
        emit_say("error", &format!("{e:#}"));
    }
    let usage = *lock(&LAST_USAGE);
    emit(json!({
        "type": "turn_end",
        "turn_id": turn_id,
        "ok": r.is_ok(),
        "interrupted": interrupted,
        "usage": usage,
    }));
    emit_result(&req, r.is_ok(), json!({"turn_id": turn_id}));
    *lock(&CURRENT_REQ) = None;
    sync_session(sess);
    r?;
    Ok(())
}

/// Leftover queued inputs after a turn (or that arrived between requests)
/// run as their own turns/commands — the REPL's post-turn queued-input pop.
async fn drain_queued(
    sess: &mut Session,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &repl::TuiOpt,
) -> anyhow::Result<()> {
    for _ in 0..16 {
        let next = lock(&STEER_QUEUE).pop_front();
        let Some((text, images, req)) = next else {
            break;
        };
        let cmd = repl::expand_prompt_template(repl::parse_command(&text), cwd, false);
        match cmd {
            repl::ReplCommand::Empty => {}
            repl::ReplCommand::Prompt(text) => {
                run_turn(sess, config, cwd, tui, req, text, images).await?
            }
            other => {
                *lock(&CURRENT_REQ) = Some(req.clone());
                let flow = repl::dispatch::dispatch_command(
                    other,
                    &mut sess.agent,
                    config,
                    cwd,
                    tui,
                    &mut sess.state,
                    &mut sess.totals,
                    &mut sess.pending_command,
                    &mut sess.pending_history,
                    &mut sess.unconfigured,
                    &mut sess.hide_thinking,
                )
                .await?;
                *lock(&CURRENT_REQ) = None;
                if flow == repl::dispatch::Flow::Break {
                    sess.quit = true;
                }
                drain_pending(sess, config, cwd, tui).await?;
            }
        }
    }
    Ok(())
}

/// `pending_command` chains (e.g. `/new hi` queues a prompt, plugin commands
/// queue follow-ups). Runs until empty; each item inherits the current
/// request id for `req` correlation.
async fn drain_pending(
    sess: &mut Session,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &repl::TuiOpt,
) -> anyhow::Result<()> {
    for _ in 0..16 {
        let Some(cmd) = sess.pending_command.take() else {
            break;
        };
        match cmd {
            repl::ReplCommand::Prompt(text) => {
                let req = lock(&CURRENT_REQ).clone().unwrap_or(Value::Null);
                run_turn(sess, config, cwd, tui, req, text, Vec::new()).await?
            }
            other => {
                let flow = repl::dispatch::dispatch_command(
                    other,
                    &mut sess.agent,
                    config,
                    cwd,
                    tui,
                    &mut sess.state,
                    &mut sess.totals,
                    &mut sess.pending_command,
                    &mut sess.pending_history,
                    &mut sess.unconfigured,
                    &mut sess.hide_thinking,
                )
                .await?;
                if flow == repl::dispatch::Flow::Break {
                    sess.quit = true;
                    break;
                }
            }
        }
    }
    Ok(())
}

/// The REPL's between-turn maintenance, without the TUI: drain host `say`
/// output, surface background/cron/send wake cards, and run an injected
/// follow-up turn when one is waiting.
async fn idle_tick(
    sess: &mut Session,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &repl::TuiOpt,
) -> anyhow::Result<()> {
    for lines in crate::host::take_host_say() {
        emit_say_lines(&lines);
    }
    if !cfg!(windows)
        && let Ok(home) = crate::setup::gray_home()
        && let Some(sid) = sess.sid()
        && crate::session_mailbox::session_inbox_pending(&home, &sid)
    {
        crate::host::request_wake();
    }
    crate::host::clear_wake();

    let sid = sess.sid();
    crate::host::set_live_session(sid.as_deref());
    crate::host::set_background_source(sess.agent.as_ref().map(|a| {
        (
            a.executor_handle(),
            repl::jobs::session_ctx(cwd, sid.as_deref()),
        )
    }));

    let Some((cards, prompt, sends)) =
        repl::notice::idle_wake(sess.agent.as_ref(), sid.as_deref(), cwd)
    else {
        return Ok(());
    };
    for card in &cards {
        match card {
            repl::notice::WakeCard::Job(j) => emit_say(
                "info",
                &format!(
                    "background job {} finished ({}) after {} · log {}",
                    j.id, j.outcome, j.elapsed, j.log
                ),
            ),
            repl::notice::WakeCard::Cron(c) => emit_say(
                "info",
                &format!(
                    "cron: {}",
                    repl::notice::format_delivery_plain(&c.name, &c.body, c.reminder, c.failed)
                ),
            ),
            repl::notice::WakeCard::Text(t) => emit_say("info", t),
        }
    }
    if let Some(msg) = crate::turn_caps::check_caps(config, sess.totals.turns, sess.totals.cost) {
        emit_say("warn", &msg);
        crate::session_send::hold_back(&sends);
        return Ok(());
    }
    let mut images = Vec::new();
    for m in &sends {
        emit_say("info", &crate::session_send::prompt_for(&m.from, &m.text));
        images.extend(m.attachments.iter().cloned());
    }
    crate::session_send::mark_delivered(&sends);
    run_turn(sess, config, cwd, tui, Value::Null, prompt, images).await
}

async fn run_command(
    sess: &mut Session,
    config: &mut Config,
    cwd: &std::path::Path,
    tui: &repl::TuiOpt,
    req: Value,
    line: String,
) -> anyhow::Result<()> {
    *lock(&CURRENT_REQ) = Some(req.clone());
    let cmd = repl::expand_prompt_template(repl::parse_command(&line), cwd, false);
    match cmd {
        repl::ReplCommand::Empty => {
            *lock(&CURRENT_REQ) = None;
            emit_result(&req, true, Value::Object(Default::default()));
        }
        repl::ReplCommand::Prompt(text) => {
            *lock(&CURRENT_REQ) = None;
            run_turn(sess, config, cwd, tui, req, text, Vec::new()).await?;
        }
        other => {
            let flow = repl::dispatch::dispatch_command(
                other,
                &mut sess.agent,
                config,
                cwd,
                tui,
                &mut sess.state,
                &mut sess.totals,
                &mut sess.pending_command,
                &mut sess.pending_history,
                &mut sess.unconfigured,
                &mut sess.hide_thinking,
            )
            .await?;
            *lock(&CURRENT_REQ) = None;
            if flow == repl::dispatch::Flow::Break {
                sess.quit = true;
            }
            drain_pending(sess, config, cwd, tui).await?;
            emit_result(&req, true, Value::Object(Default::default()));
        }
    }
    sync_session(sess);
    Ok(())
}

/// Entry point for `gray --stdio` — see main.rs. Mirrors `run_repl_mode`'s
/// non-interactive setup, then serves requests until stdin closes.
pub async fn run_stdio_mode(
    config: &mut Config,
    resume_last: bool,
    session_id: Option<&str>,
) -> anyhow::Result<()> {
    ACTIVE.store(true, Ordering::Relaxed);
    let _ = crossterm::terminal::disable_raw_mode();
    let cwd = std::env::current_dir()?;

    crate::setup::set_user_context_window(config.context_window);
    crate::setup::set_user_reserve_tokens(config.context_reserve);
    crate::setup::set_user_keep_recent_tokens(config.context_keep);
    crate::setup::set_active_model_provider(&config.base_url);

    // Same background warm-up the REPL kicks off (minus TUI-only pieces).
    let offline = crate::config::offline();
    if !offline && !config.bare {
        tokio::spawn(crate::setup::fetch_models_dev_context());
        tokio::spawn(crate::setup::fetch_provider_metadata());
        tokio::spawn(crate::setup::fetch_recommended_models());
        if crate::setup::get_user_context_window().is_none() {
            tokio::spawn(crate::setup::fetch_litellm_context_windows());
            tokio::spawn(crate::setup::fetch_openrouter_rates());
        }
        if !config.uses_plugin_credentials() && config.context_window.is_none() {
            let base = config.base_url.clone();
            let key = config.api_key.clone();
            tokio::task::spawn_blocking(move || {
                crate::setup::fetch_live_provider_models(&base, key.as_deref());
            });
        }
        if config.uses_plugin_credentials() && config.context_window.is_none() {
            crate::setup::context::warm_plugin_model_context(config).await;
        }
    }
    std::thread::spawn(crate::shell_drain::sweep_old_shell_logs);

    // Ask/approval plumbing: demuxed JSON mode; answers arrive via the
    // `approve` request instead of a blocking stdin read.
    crate::ask::install_json(String::new());
    crate::ask::set_demux();

    if !config.bare {
        let _ = crate::plugin_status::ensure_boot(Some(crate::host::default_handler(cwd.clone())));
    }
    crate::host::set_repl_active(true);

    let mut sess = Session::new(config.model.is_none(), config.reasoning_hidden());
    let tui: repl::TuiOpt = None;

    // Session bootstrap: same load path as `run_repl_mode`.
    type Resumed = (
        SessionId,
        SessionMeta,
        Vec<crate::session_store::SessionEntry>,
        JsonlSessionStore,
    );
    let mut loaded: Option<Resumed> = None;
    if let Some(id) = session_id
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let sid = SessionId::new(id);
        match store.load(&sid).await {
            Ok((meta, entries)) => loaded = Some((sid, meta, entries, store)),
            Err(e) => emit_say("warn", &format!("could not resume session {id}: {e}")),
        }
    }
    if resume_last
        && loaded.is_none()
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let cwd_now = std::env::current_dir().ok();
        let mut best: Option<(
            SessionId,
            SessionMeta,
            Vec<crate::session_store::SessionEntry>,
        )> = None;
        if let Some(c) = cwd_now.as_deref()
            && let Some(rid) = store.recall_validated(c).await
            && let Ok((meta, entries)) = store.load(&rid).await
        {
            best = Some((rid, meta, entries));
        }
        if best.is_none() {
            let summaries = store.list().await;
            let latest = crate::resume::latest_summary(&summaries, cwd_now.as_deref())
                .or_else(|| crate::resume::latest_summary(&summaries, None));
            if let Some(l) = latest
                && let Ok((meta, entries)) = store.load(&l.id).await
            {
                best = Some((l.id.clone(), meta, entries));
            }
        }
        if let Some((sid, meta, entries)) = best {
            loaded = Some((sid, meta, entries, store));
        }
    }
    if let Some((sid, meta, entries, store)) = loaded {
        let open_guard = Some(store.acquire_open(&sid).await?);
        if config.model.is_none() && !meta.model.is_empty() {
            config.model = Some(meta.model.clone());
        }
        if let Some((old, new)) = repl::normalize_adopted_model(config) {
            emit_say(
                "warn",
                &format!(
                    "Thinking effort clamped from {old} to {new} (not supported by this model)"
                ),
            );
        }
        sess.state = Some(SessionState {
            full_save_pending: false,
            session_id: sid.clone(),
            store,
            _open_guard: open_guard,
        });
        repl::session::export_session_env(sid.as_str());
        sess.totals = SessionTotals::from_entries(&entries, config.model.as_deref().unwrap_or(""));
        sess.pending_history = std::sync::Arc::new(
            entries
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<Message>>(),
        );
    }

    // Default thinking effort, same heuristic the REPL applies.
    if config.thinking_effort.is_none()
        && crate::setup::model_supports_reasoning(&config.model.clone().unwrap_or_default())
            != Some(false)
    {
        config.thinking_effort = Some("high".to_string());
        if let Ok(path) = crate::setup::saved_config_path() {
            let _cfg_lock = crate::setup::lock_saved_config_at(&path)?;
            let mut saved = crate::setup::load_saved_config_at(&path);
            if saved.thinking_effort.is_none() {
                saved.thinking_effort = Some("high".to_string());
                crate::setup::save_saved_config_at(&path, &saved)?;
            }
        }
    }

    sync_session(&sess);
    emit(json!({
        "type": "hello",
        "gray_version": env!("CARGO_PKG_VERSION"),
        "cwd": cwd.display().to_string(),
        "model": config.model,
        "effort": config.thinking_effort,
        "provider_id": config.provider_id,
        "base_url": config.base_url,
        "unconfigured": sess.unconfigured,
    }));

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let ptx = tx.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(v) => route_incoming(&ptx, v),
                Err(e) => emit(json!({
                    "type": "error",
                    "code": "bad_json",
                    "message": format!("malformed request line: {e}"),
                })),
            }
        }
    });
    drop(tx);

    loop {
        if let Err(e) = idle_tick(&mut sess, config, &cwd, &tui).await {
            emit_say("error", &format!("{e:#}"));
        }
        if let Err(e) = drain_queued(&mut sess, config, &cwd, &tui).await {
            emit_say("error", &format!("{e:#}"));
        }
        if let Err(e) = drain_pending(&mut sess, config, &cwd, &tui).await {
            emit_say("error", &format!("{e:#}"));
        }
        if sess.quit {
            break;
        }
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(None) => break,
            Err(_) => continue,
            Ok(Some(v)) => {
                let id = v.get("id").cloned().unwrap_or(Value::Null);
                let ty = v
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                let r = match ty.as_str() {
                    "prompt" => {
                        let text = v
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string();
                        if text.trim().is_empty() && parse_attachments(&v).is_empty() {
                            emit_result(&id, false, json!({"error": "prompt needs text"}));
                            Ok(())
                        } else {
                            run_turn(
                                &mut sess,
                                config,
                                &cwd,
                                &tui,
                                id.clone(),
                                text,
                                parse_attachments(&v),
                            )
                            .await
                        }
                    }
                    "command" => {
                        let line = command_line(&v);
                        if line.is_empty() {
                            emit_result(&id, false, json!({"error": "command needs line"}));
                            Ok(())
                        } else {
                            run_command(&mut sess, config, &cwd, &tui, id.clone(), line).await
                        }
                    }
                    "steer" => {
                        let text = v
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_string();
                        push_queued(text, parse_attachments(&v), id.clone());
                        emit_result(&id, true, json!({"queued": true}));
                        Ok(())
                    }
                    "interrupt" => {
                        requests::interrupt(&id);
                        Ok(())
                    }
                    "approve" => {
                        requests::approve(&v, &id);
                        Ok(())
                    }
                    "status" => {
                        requests::status(&mut sess, config, &cwd, &id);
                        Ok(())
                    }
                    "list_commands" => {
                        requests::list_commands(&sess, &id);
                        Ok(())
                    }
                    "list_models" => {
                        requests::list_models(config, &v, &id).await;
                        Ok(())
                    }
                    "set_model" => {
                        requests::set_model(&mut sess, config, &cwd, &tui, &v, &id).await;
                        Ok(())
                    }
                    "list_sessions" => requests::list_sessions(&v, &id).await,
                    "resume" => {
                        requests::resume(&mut sess, config, &cwd, &tui, &v, &id).await;
                        Ok(())
                    }
                    "shutdown" | "quit" => {
                        sess.quit = true;
                        emit_result(&id, true, json!({"bye": true}));
                        Ok(())
                    }
                    "" => {
                        emit_result(&id, false, json!({"error": "missing request type"}));
                        Ok(())
                    }
                    other => {
                        emit_result(
                            &id,
                            false,
                            json!({"error": format!("unknown request type '{other}'")}),
                        );
                        Ok(())
                    }
                };
                if let Err(e) = r {
                    emit_result(&id, false, json!({"error": format!("{e:#}")}));
                }
            }
        }
    }

    // Shutdown path mirrors the REPL's: hooks (bounded), then shell tasks.
    crate::ask::shutdown();
    repl::shutdown_hooks(sess.agent.as_ref()).await;
    repl::shutdown_shell_tasks(sess.agent.as_ref(), &sess.state, &cwd).await;
    ACTIVE.store(false, Ordering::Relaxed);
    Ok(())
}
