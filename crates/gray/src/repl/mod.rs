//! Interactive REPL mode for Gray.
// 2 turn loops (~400 lines) + 3 provider blocks duplicated; extract ensure_provider + run_turn when adding streaming resume.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use crate::session_store::{JsonlSessionStore, SessionId, SessionMeta, default_root};
use gray_core::agent::{Agent, CommandOutcome, PluginHooks, ToolContext};
use gray_core::error::CoreError;
use gray_core::event::AgentEvent;
use gray_core::message::Message;

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

/// Static slash-command table driving both `/help` and the autocomplete panel.
/// True while an agent turn is in flight: `Some(token)` cancels on first
/// Ctrl-C (token consumed, turn handler reports it, REPL stays alive); a
/// second press with no token in flight exits. At the prompt (no token)
/// the first press clears the draft (or arms exit when already empty) and
/// only a second press within 5 s exits — otherwise exit via /quit (or
/// Ctrl-D on an empty line). Single mutex = no TOCTOU between flag and token.
static TURN_STATE: StdMutex<Option<tokio_util::sync::CancellationToken>> = StdMutex::new(None);

/// Window for a second Ctrl-C/SIGINT to confirm exit (both the global
/// signal policy below and the prompt `read_line` key handler agree on 5 s).
pub(crate) const CTRL_C_EXIT_WINDOW_MS: u64 = 5_000;

/// Pure repeat check shared by the signal policy and (via the same window)
/// the prompt handler: true only for a second press inside the window.
pub(crate) fn sigint_should_exit(last_ms: u64, now_ms: u64) -> bool {
    now_ms.wrapping_sub(last_ms) <= CTRL_C_EXIT_WINDOW_MS
}

/// Last at-prompt SIGINT (millis since epoch) for the two-press exit.
/// Mid-turn presses consume the turn token instead and never touch this.
static LAST_PROMPT_SIGINT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// Installs the single global Ctrl-C policy:
/// - during a turn: cancel the turn (first press), the turn handler reports
///   it and the REPL stays alive; a second press with no token exits.
/// - at the prompt: first press only arms exit (the `read_line` key handler
///   clears the draft instead of quitting); a second press within 5 s exits
///   cleanly, otherwise exit via /quit (or Ctrl-D on an empty line).
async fn spawn_ctrl_c_policy() {
    loop {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        let token = TURN_STATE.lock().ok().and_then(|mut g| g.take());
        if let Some(t) = token {
            t.cancel(); // first press mid-turn: cancel, stay alive
        } else {
            let now = now_ms();
            let last = LAST_PROMPT_SIGINT_MS.load(std::sync::atomic::Ordering::Relaxed);
            if sigint_should_exit(last, now) && last != 0 {
                // Second press within the window: exit cleanly.
                // Say something — a bare exit(0) mid-turn looks like a crash.
                let _ = crossterm::terminal::disable_raw_mode();
                let _ = write!(
                    std::io::stdout(),
                    "\x1b[?25h\r\n\x1b[2m(interrupted — bye)\x1b[0m\r\n"
                );
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            }
            // First press at the prompt: arm exit, stay alive (the prompt
            // key handler clears the draft; exit via second press or /quit).
            LAST_PROMPT_SIGINT_MS.store(now, std::sync::atomic::Ordering::Relaxed);
        }
    }
}
use crate::config::Config;
use crate::{DEFAULT_SYS_PROMPT, build_agent, load_or_create_system_prompt_at};

pub mod attachments;
pub mod commands;
mod cron;
mod dispatch;
pub mod format;
mod handlers;
mod key_watcher;
mod plugin_cmds;
mod prompt_turn;
mod session;
mod status;
mod user_cmds;

pub(crate) use commands::{REGISTRY, completion_fill, completion_matches_dyn};
pub use commands::{ReplCommand, ResumeArgs, SysAction, parse_command};
pub(crate) use format::build_user_message_with_attachments;
pub use format::{THINKING_STYLE, fmt_usage, format_core_error};
pub(crate) use handlers::{
    expand_skill_command, handle_model, handle_sys, handle_thinking, reload_agent,
};
pub(crate) use plugin_cmds::handle_plugin_command;
pub(crate) use session::{
    dispatch_agent_event, handle_resume, maybe_overflow_compact, maybe_threshold_compact,
    persist_turn_messages, print_exit_hint,
};
pub(crate) use status::{
    SessionTotals, handle_compact, handle_context_window, handle_copy, handle_usage, turn_footer,
    turn_tokens_per_second,
};
pub(crate) use user_cmds::handle_feedback;

/// Shared TUI handle: the composer plus its shutdown flag.
pub(crate) type TuiOpt = Option<(
    crate::composer::SharedTui,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
)>;

pub(crate) struct SessionState {
    /// A failed write requires a full replacement before any suffix append.
    pub(crate) full_save_pending: bool,
    pub(crate) store: crate::session_store::JsonlSessionStore,
    pub(crate) session_id: crate::session_store::SessionId,
}

/// Command feedback: through the composer when it owns the terminal, else stdout.
/// Raw println! while the composer viewport is live collides with the next draw (ghost input).
pub(crate) fn say(tui: Option<&crate::composer::SharedTui>, msg: &str) {
    if let Some(t) = tui {
        let mut t = t.lock().expect("tui lock");
        // No gap above: command cards skip their trailing gap so this hugs them.
        for line in msg.split('\n') {
            t.push_dim(format!("└ {line}"));
        }
        // Breathing room below command output before the next prompt.
        t.ensure_gap(1);
    } else {
        println!("{msg}");
    }
}

/// Clamp `config.thinking_effort` to the levels `model` accepts
/// (Prime-Agent `clampThinkingLevel` parity). `None`/empty effort stays
/// as-is; unknown family keeps the current level. Persists to saved config.
/// Returns `(old, new)` when a clamp happened.
pub(crate) fn clamp_thinking_to_model_name(
    config: &mut Config,
    model: &str,
) -> Option<(String, String)> {
    if model.is_empty() {
        return None;
    }
    let current = config.thinking_effort.clone()?;
    if current.is_empty() {
        return None;
    }
    let clamped = crate::setup::clamp_thinking_level(model, &current);
    if clamped == current {
        return None;
    }
    let (old, new) = (current, clamped.to_string());
    config.thinking_effort = Some(new.clone());
    if let Ok(path) = crate::setup::saved_config_path() {
        let mut saved = crate::setup::load_saved_config_at(&path);
        saved.thinking_effort = Some(new.clone());
        let _ = crate::setup::save_saved_config_at(&path, &saved);
    }
    Some((old, new))
}

/// Clamp `config.thinking_effort` to what `config.model` accepts.
/// See [`clamp_thinking_to_model_name`].
pub(crate) fn clamp_thinking_to_model(config: &mut Config) -> Option<(String, String)> {
    let model = config.model.clone().unwrap_or_default();
    clamp_thinking_to_model_name(config, &model)
}

/// Post-`run_connect_modal` feedback shared by `/connect` and the
/// first-turn setup path: sync the chosen model into the composer and echo
/// the provider name. Clamps a stale effort (e.g. deepseek `max` → Spark
/// `xhigh`) before painting so the footer never shows an unsupported level.
pub(crate) fn push_provider_connected(
    config: &mut Config,
    tui: &TuiOpt,
    hide_thinking: Option<&mut bool>,
) {
    crate::setup::set_active_model_provider(&config.base_url);
    let clamped = clamp_thinking_to_model(config);
    if clamped.is_some()
        && let Some(h) = hide_thinking
    {
        *h = config.reasoning_hidden();
    }
    let Some((shared, _)) = tui else {
        if let Some((old, new)) = clamped {
            println!("Thinking effort clamped from {old} to {new} (not supported by this model)");
        }
        return;
    };
    let mut t = shared.lock().expect("tui lock");
    if let Some(m) = &config.model {
        t.set_model(m.clone());
    }
    if let Some((_, ref new)) = clamped {
        t.set_thinking_effort(new.clone());
        t.set_hide_thinking(config.reasoning_hidden());
    }
    let model_str = config.model.as_deref().unwrap_or("default");
    let prov_name = crate::setup::load_catalog()
        .ok()
        .and_then(|c| {
            c.values()
                .find(|p| p.base_url == config.base_url)
                .map(|p| p.name.clone())
        })
        .unwrap_or_else(|| "provider".to_string());
    t.push_dim(format!("└ connected to {prov_name} · {model_str}"));
    // Close the loop: what you got, and where to change it.
    let effort = config.thinking_effort.as_deref().unwrap_or("high");
    t.push_dim(format!(
        "└ thinking {effort} · /model to switch, /thinking for effort"
    ));
    if let Some((old, new)) = clamped {
        t.push_dim(format!(
            "└ thinking effort clamped from {old} to {new} (not supported by this model)"
        ));
    }
    t.ensure_gap(1);
    let _ = t.draw();
}

/// Split a `/name argv…` line into (`/name`, argv words) for plugin
/// slash-command routing. `None` when the line isn't a slash command.
fn split_plugin_command(line: &str) -> Option<(String, Vec<String>)> {
    let words = shlex::split(line.trim().strip_prefix('/')?)?;
    let (first, rest) = words.split_first()?;
    if first.is_empty() {
        return None;
    }
    Some((format!("/{first}"), rest.to_vec()))
}

/// Claimed plugin slash commands for `/help`, in hook order. Names drop
/// the leading slash for display (`echo`, not `/echo`).
fn plugin_help_entries(hooks: &[Arc<dyn PluginHooks>]) -> Vec<(String, String)> {
    hooks
        .iter()
        .flat_map(|h| h.commands())
        .map(|c| {
            (
                c.name.strip_prefix('/').unwrap_or(&c.name).to_string(),
                c.description,
            )
        })
        .collect()
}

/// Protocol v1 `command/run`: the first hook claiming `/name` owns it.
/// `None` when no hook claims the command (the caller keeps the
/// unknown-command message) or the owner declines to handle it.
/// The outcome decides the caller's path: `Say` prints via `say()`,
/// `Prompt` is submitted as a `ReplCommand::Prompt` turn.
/// Headless agent behind the cron `AsyncRunner` seam for the REPL tick:
/// fresh agent per fire (no resume/history), events collected without
/// streaming. Mirrors `HeadlessRunner` in cron_serve.rs; the runner is the
/// host's business (cron_serve stays agent-agnostic).
struct ReplRunner {
    config: Config,
}

#[async_trait::async_trait(?Send)]
impl crate::cron_serve::AsyncRunner for ReplRunner {
    async fn run(&self, prompt: String, cwd: std::path::PathBuf) -> anyhow::Result<String> {
        let mut agent = crate::build_agent(&self.config, &cwd, None).await?;
        let ctx = gray_core::agent::ToolContext {
            cwd,
            cancel: tokio_util::sync::CancellationToken::new(),
            session_id: None,
        };
        let events = agent
            .run(gray_core::message::Message::user(prompt), ctx)
            .await
            .map_err(|e| {
                anyhow::anyhow!(crate::repl::format_core_error(&e, &self.config.base_url))
            })?;
        Ok(crate::cron_fire::transcript_text(&events))
    }
}

async fn run_plugin_command(
    hooks: &[Arc<dyn PluginHooks>],
    name: &str,
    argv: Vec<String>,
) -> Option<CommandOutcome> {
    let owner = hooks
        .iter()
        .find(|h| h.commands().iter().any(|c| c.name == name))?;
    owner.run_command(name, argv).await
}

// No background shell tasks exist (blocking-only bash): nothing to stop on quit.
async fn shutdown_shell_tasks(_session_state: &Option<SessionState>, _tui: &TuiOpt) {}

/// Graceful sidecar teardown (`plugin/shutdown`); best-effort, never fails.
async fn shutdown_hooks(agent: Option<&gray_core::agent::Agent>) {
    let hooks: Vec<Arc<dyn PluginHooks>> = agent.map(|a| a.hooks().to_vec()).unwrap_or_default();
    for h in &hooks {
        h.shutdown().await;
    }
}

/// Restores the inline viewport after an alternate-screen modal (model/provider/etc).
/// EnterAlternateScreen/LeaveAlternateScreen breaks ratatui's Inline(10) viewport anchor;
/// without this the next Tui::draw renders off-screen and the input box vanishes.
/// Width unchanged → just re-anchor (LeaveAlternateScreen already restored the
/// scrollback; clearing/re-emitting here destroyed it). Width changed → full reflow.
fn restore_viewport(tui: Option<&crate::composer::SharedTui>) {
    if let Some(shared) = tui {
        let mut t = shared.lock().expect("tui lock");
        let (cols, rows) = crossterm::terminal::size().unwrap_or((t.last_width, t.last_height));
        if cols == t.last_width && rows == t.last_height {
            t.reanchor_viewport(cols);
        } else {
            t.pending_resize = None;
            t.reflow_on_resize(cols);
        }
    }
}

async fn with_modal<T>(
    tui: Option<&crate::composer::SharedTui>,
    fut: impl std::future::Future<Output = T>,
) -> T {
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").set_modal_open(true);
    }
    let r = fut.await;
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").set_modal_open(false);
    }
    restore_viewport(tui);
    r
}

pub(crate) fn with_modal_sync<T>(
    tui: Option<&crate::composer::SharedTui>,
    f: impl FnOnce() -> T,
) -> T {
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").set_modal_open(true);
    }
    let r = f();
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").set_modal_open(false);
    }
    restore_viewport(tui);
    r
}

/// Runs Gray in interactive REPL mode.
pub async fn run_repl_mode(
    config: &mut Config,
    resume_last: bool,
    session_id: Option<&str>,
) -> anyhow::Result<()> {
    let _ = crossterm::terminal::disable_raw_mode();
    crate::tui::clear_screen();
    let cwd = std::env::current_dir()?;

    // Interactive terminals get the ratatui composer; piped input falls back
    // to plain cooked reads (scripts, tests).
    use std::io::IsTerminal;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();

    // context window: user override > provider live > disk > litellm/models.dev > guess fallback
    crate::setup::set_user_context_window(config.context_window);
    crate::setup::set_user_reserve_tokens(config.context_reserve);
    crate::setup::set_user_keep_recent_tokens(config.context_keep);
    crate::setup::set_active_model_provider(&config.base_url);
    // Discovery also feeds completion, even with a cached/overridden context window.
    let base = config.base_url.clone();
    let key = config.api_key.clone();
    tokio::task::spawn_blocking(move || {
        crate::setup::fetch_live_provider_models(&base, key.as_deref());
    });
    tokio::spawn(crate::setup::fetch_models_dev_context());
    if crate::setup::get_user_context_window().is_none() {
        tokio::spawn(crate::setup::fetch_litellm_context_windows());
        tokio::spawn(crate::setup::fetch_openrouter_rates());
    }

    // boot: no forced wizard. A dim hint appears when unconfigured,
    // and the provider picker fires the moment credentials are needed.
    tokio::spawn(spawn_ctrl_c_policy());
    // Shell logs: 7-day + 10MiB startup sweep (blocking-only bash keeps
    // per-call logs on disk for the header's grep hint).
    crate::shell_drain::sweep_old_shell_logs();

    let mut unconfigured = config.model.is_none();
    // Piped first-run skips onboarding like `-p` (never blocks on a picker).
    if unconfigured && interactive {
        let ready = crate::setup::run_onboarding(config).await?;
        if !ready {
            print!(
                "\r\x1b[2mrunning without a provider — send a message to set one up (or /provider)\x1b[0m\r\n"
            );
        }
        print!("\r\n");
        crate::setup::set_active_model_provider(&config.base_url);
        // onboarding may have set model/base_url — re-sync context window override and prime cache
        crate::setup::set_user_context_window(config.context_window);
        crate::setup::set_user_reserve_tokens(config.context_reserve);
        crate::setup::set_user_keep_recent_tokens(config.context_keep);
        // reasoning efforts already fetched unconditionally at boot (see above).
        if crate::setup::get_user_context_window().is_none() {
            tokio::spawn(crate::setup::fetch_litellm_context_windows());
            if let Some(m) = config.model.clone()
                && crate::setup::get_cached_model_context(&m).is_none()
            {
                let base = config.base_url.clone();
                let key = config.api_key.clone();
                tokio::spawn(async move {
                    crate::setup::fetch_live_provider_models(&base, key.as_deref());
                });
            }
        }
    }

    // The agent is built lazily so the REPL opens even with no model/key configured;
    // we surface a friendly hint on first use instead of refusing to start.
    let mut agent: Option<Agent> = None;
    let mut session_state: Option<SessionState> = None;
    let mut session_totals = SessionTotals::default();
    let mut pending_history: Vec<Message> = Vec::new();
    let mut resumed_session_info: Option<(SessionId, Vec<crate::session_store::SessionEntry>)> =
        None;

    // `--session <id>` reopens that exact session; `-c`/`--last` reopens the
    // most recent. Both resolve into `loaded` and share one apply block.
    type Resumed = (
        SessionId,
        crate::session_store::SessionMeta,
        Vec<crate::session_store::SessionEntry>,
        JsonlSessionStore,
    );
    let mut loaded: Option<Resumed> = None;

    // `--session <id>`: reopen that exact session.
    if let Some(id) = session_id
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let sid = SessionId::new(id);
        match store.load(&sid).await {
            Ok((meta, entries)) => loaded = Some((sid, meta, entries, store)),
            Err(e) => {
                println!("could not resume session {id}: {e}");
            }
        }
    }

    // `-c`: reopen the most recent session instead of starting blank.
    // Recall-first: the remembered pointer answers in one file read; the
    // list scan below is the fallback (cold start, pruned pointer, corrupt
    // recalled file — any failure degrades, never errors).
    if resume_last
        && loaded.is_none()
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let cwd_now = std::env::current_dir().ok();
        // (session id, meta, entries) — one load per path, never two.
        type Best = (
            SessionId,
            crate::session_store::SessionMeta,
            Vec<crate::session_store::SessionEntry>,
        );
        let mut recalled: Option<Best> = None;
        if let Some(c) = cwd_now.as_deref()
            && let Some(rid) = store.recall_validated(c).await
            && let Ok((meta, entries)) = store.load(&rid).await
        {
            recalled = Some((rid, meta, entries));
        }
        let best: Option<Best> = match recalled {
            Some(hit) => Some(hit),
            None => {
                let summaries = store.list().await;
                let latest = crate::resume::latest_summary(&summaries, cwd_now.as_deref())
                    .or_else(|| crate::resume::latest_summary(&summaries, None));
                match latest {
                    Some(l) => match store.load(&l.id).await {
                        Ok((meta, entries)) => Some((l.id.clone(), meta, entries)),
                        Err(e) => {
                            println!("could not resume: {e}");
                            None
                        }
                    },
                    None => None,
                }
            }
        };
        if let Some((sid, meta, entries)) = best {
            loaded = Some((sid, meta, entries, store));
        }
    }

    if let Some((sid, meta, entries, store)) = loaded {
        if config.model.is_none() && !meta.model.is_empty() {
            config.model = Some(meta.model.clone());
        }
        // Startup resume lands on the session's model: clamp a stale effort
        // (e.g. saved `max` under a Spark session) before the first build
        // and before the TUI init below paints the footer.
        if let Some((old, new)) = clamp_thinking_to_model(config) {
            println!("Thinking effort clamped from {old} to {new} (not supported by this model)");
        }
        let history: Vec<Message> = entries.iter().map(|e| e.message.clone()).collect();
        pending_history = history.clone();
        if let Ok(built) = build_agent(config, &cwd, Some(sid.as_str())).await {
            agent = Some(built.with_messages(history));
        }
        // T3.4 lifecycle: resumed sessions start with no ledger state
        // (fresh builds start empty; clear anyway — see dispatch /new).
        if let Some(ledger) = gray_plugin::builder::current_file_ledger() {
            ledger.clear();
        }
        session_state = Some(SessionState {
            full_save_pending: false,
            session_id: sid.clone(),
            store,
        });
        session_totals =
            SessionTotals::from_entries(&entries, config.model.as_deref().unwrap_or(""));
        resumed_session_info = Some((sid, entries));
    }

    // Fresh sessions need the same normalization as model switches and resumes.
    if let Some((old, new)) = clamp_thinking_to_model(config) {
        println!("Thinking effort clamped from {old} to {new} (not supported by this model)");
    }

    // Interactive terminals get the ratatui composer; piped input falls back
    // to plain cooked reads (scripts, tests).
    // The composer owns the bottom pane for the whole session. A tiny ticker
    // task refreshes the elapsed-seconds status while turns run.
    // NOTE: `is_terminal()` alone still passes on headless ptys where the
    // cursor-position query inside `Tui::new` (ratatui Inline viewport)
    // fails. Probe upfront and map init failure to a clean error — a panic
    // here exits 101 via the panic hook in main.rs (kept as-is for real
    // bugs); a clean error exits non-zero with a readable message instead.
    let tui: TuiOpt = if interactive {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        if crossterm::terminal::size().is_err() {
            anyhow::bail!(
                "not a terminal — the interactive composer needs a real terminal \
                 (could not query terminal size); pipe input or run under a TTY"
            );
        }
        let shared = std::sync::Arc::new(std::sync::Mutex::new({
            let mut t = crate::composer::Tui::new().map_err(|e| {
                anyhow::anyhow!(
                    "not a terminal — the interactive composer needs a real terminal \
                     (cursor position could not be read: {e:#}); pipe input or run under a TTY"
                )
            })?;
            if let Some(m) = &config.model {
                t.set_model(m.clone());
            }
            if let Some(eff) = &config.thinking_effort {
                t.set_thinking_effort(eff.clone());
            }
            t.set_hide_thinking(config.reasoning_hidden());
            t.set_cwd(cwd.display().to_string());
            if let Some((ref sid, ref entries)) = resumed_session_info {
                t.replay_session_history(entries, &cwd);
                t.ensure_gap(1);
                t.push_dim(format!(
                    "\u{2b22} Resumed session {} ({} messages)",
                    sid.as_str(),
                    entries.len()
                ));
                t.ensure_gap(1);
            }
            t
        }));
        crate::host::register_tui(&shared);
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let ticker_stop = stop.clone();
        let ticker_tui = shared.clone();
        tokio::spawn(async move {
            loop {
                if ticker_stop.load(AtomicOrdering::Relaxed) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if ticker_stop.load(AtomicOrdering::Relaxed) {
                    break;
                }
                if let Ok(mut t) = ticker_tui.try_lock() {
                    // Stop may have been set while we were waiting to
                    // acquire the lock (a keystroke handler holds it
                    // briefly). Don't repaint the viewport after the main
                    // thread has already started shutdown.
                    if ticker_stop.load(AtomicOrdering::Relaxed) {
                        break;
                    }
                    t.tick_status();
                }
            }
        });
        Some((shared, stop))
    } else {
        None
    };

    // Cron: background tick in live sessions (workstream C). A dedicated OS
    // thread + current-thread runtime — the agent future is `!Send`, so it
    // can never `tokio::spawn`. Fires claim through the store's at-most-once
    // contract, so a concurrent `tick`/`serve` simply sees nothing due.
    // Results surface via the host/say queue drained at the top of the loop
    // (new chat lines in the session's own right, never transcript).
    // No join on exit: process return terminates the thread, and an
    // in-flight fire's claim TTL (300s) lets the next ticker reclaim it.
    // File-only cron management remains usable on Windows; automatic firing
    // must not bypass the CLI's explicit unsupported-execution boundary.
    if interactive && !cfg!(windows) {
        let cfg = config.clone();
        if let Ok(home) = crate::setup::gray_home()
            && let Ok(store) = crate::cron::CronStore::open(home.join("cron"))
        {
            std::thread::spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                rt.block_on(async {
                    let runner = ReplRunner { config: cfg };
                    let deliver = crate::cron_serve::SaveLocalDeliver { home };
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                    loop {
                        interval.tick().await;
                        match crate::cron_serve::tick_once(&store, &runner, &deliver, "repl").await
                        {
                            Ok(rep) if rep.fired > 0 => crate::host::queue_say(format!(
                                "⏰ cron tick: fired={} errors={}",
                                rep.fired, rep.errors
                            )),
                            Ok(_) => {}
                            Err(e) => log::warn!("repl cron tick failed: {e:#}"),
                        }
                    }
                });
            });
        }
    }

    // pi's hideThinkingBlock — toggled with /thinking, session-only.
    // Reasoning is ON by default — user wants to see thinking (high effort).
    // Bare /thinking toggles visibility; picker sets level persisted to config.
    let mut hide_thinking = config.reasoning_hidden();
    // Wire default: if no effort saved yet, enable reasoning so ThinkingDelta
    // actually streams on openrouter/zen etc. Persist once so future sessions
    // keep it without relying on this default branch. Skipped when the
    // provider says the model doesn't reason (opencode parity).
    if config.thinking_effort.is_none()
        && crate::setup::model_supports_reasoning(&config.model.clone().unwrap_or_default())
            != Some(false)
    {
        config.thinking_effort = Some("high".to_string());
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            if saved.thinking_effort.is_none() {
                saved.thinking_effort = Some("high".to_string());
                let _ = crate::setup::save_saved_config_at(&path, &saved);
            }
        }
    }
    // The messaging gateway was deleted from gray core (chat returns as a
    // plugin): no autostart,
    // no boot card — the TUI starts clean.
    let mut pending_command: Option<ReplCommand> = None;
    let mut pending_images: Vec<std::path::PathBuf> = Vec::new();

    loop {
        // Plugin-initiated `host/say` lines queued while a turn ran (cron
        // reports) surface here, through the composer when it owns the screen.
        for line in crate::host::take_host_say() {
            say(tui.as_ref().map(|(s, _)| s), &line);
        }
        let cmd = if let Some(c) = pending_command.take() {
            c
        } else {
            let (line_text, images) = if interactive {
                let (shared, stop) = tui.as_ref().expect("interactive implies tui");
                let (txt, imgs) = {
                    // The input lock is per-event inside read_line, so background
                    // painters keep drawing while we wait; re-lock only to shut down.
                    let pair = match crate::composer::input::read_line(shared)? {
                        Some(v) => v,
                        None => {
                            stop.store(true, std::sync::atomic::Ordering::Relaxed);
                            shared.lock().expect("tui lock").shutdown();
                            shutdown_hooks(agent.as_ref()).await;
                            shutdown_shell_tasks(&session_state, &tui).await;
                            print_exit_hint(&session_state);
                            break;
                        }
                    };
                    // Flag stop for background tickers; with per-event locking a
                    // final ticker draw may still slip in before shutdown clears.
                    if matches!(parse_command(&pair.0), ReplCommand::Quit) {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    pair
                };
                (txt, imgs)
            } else {
                if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
                    print!("\u{203a} ");
                    std::io::stdout().flush()?;
                }
                let mut buf = String::new();
                if std::io::stdin().read_line(&mut buf)? == 0 {
                    shutdown_hooks(agent.as_ref()).await;
                    shutdown_shell_tasks(&session_state, &tui).await;
                    break;
                }
                (buf.trim().to_string(), Vec::new())
            };
            pending_images = images;
            expand_skill_command(
                parse_command(&line_text),
                cwd.as_path(),
                tui.as_ref().map(|(s, _)| s),
                false,
            )
        };
        // Clear pending images for non-prompt commands (keep for Prompt/Empty+images)
        if !matches!(&cmd, ReplCommand::Prompt(_) | ReplCommand::Empty) {
            pending_images.clear();
        }
        match cmd {
            ReplCommand::Empty => {
                // Bare Enter (no images) is a no-op. An image-only submit runs
                // the normal prompt turn with empty text.
                if !pending_images.is_empty() {
                    prompt_turn::run_prompt_turn(
                        String::new(),
                        &mut pending_images,
                        &mut agent,
                        config,
                        &cwd,
                        &tui,
                        interactive,
                        &mut session_state,
                        &mut session_totals,
                        &mut pending_command,
                        &mut pending_history,
                        &mut unconfigured,
                    )
                    .await?;
                }
            }
            ReplCommand::Prompt(prompt_text) => {
                if let Some(msg) =
                    crate::turn_caps::check_caps(config, session_totals.turns, session_totals.cost)
                {
                    say(tui.as_ref().map(|(s, _)| s), &msg);
                    shutdown_shell_tasks(&session_state, &tui).await;
                    break;
                }
                prompt_turn::run_prompt_turn(
                    prompt_text,
                    &mut pending_images,
                    &mut agent,
                    config,
                    &cwd,
                    &tui,
                    interactive,
                    &mut session_state,
                    &mut session_totals,
                    &mut pending_command,
                    &mut pending_history,
                    &mut unconfigured,
                )
                .await?;
            }
            other => {
                if dispatch::dispatch_command(
                    other,
                    &mut agent,
                    config,
                    &cwd,
                    &tui,
                    &mut session_state,
                    &mut session_totals,
                    &mut pending_command,
                    &mut pending_history,
                    &mut unconfigured,
                    &mut hide_thinking,
                )
                .await?
                    == dispatch::Flow::Break
                {
                    shutdown_shell_tasks(&session_state, &tui).await;
                    break;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;

#[path = "mod_ctrl_c_policy_tests.rs"]
#[cfg(test)]
mod ctrl_c_policy_tests;
