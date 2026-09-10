//! Interactive REPL mode for Gray.
// 2 turn loops (~400 lines) + 3 provider blocks duplicated; extract ensure_provider + run_turn when adding streaming resume.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gray_core::agent::{Agent, CommandOutcome, PermissionMode, PluginHooks, ToolContext};
use gray_core::error::CoreError;
use gray_core::event::AgentEvent;
use gray_core::message::Message;
use gray_session::{JsonlSessionStore, SessionId, SessionMeta, default_root};

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

mod acp_cmds;
pub mod attachments;
pub mod commands;
mod dispatch;
mod empty_turn;
pub mod format;
mod handlers;
mod key_watcher;
mod plugin_cmds;
mod prompt_turn;
mod session;
mod status;
mod user_cmds;

pub(crate) use acp_cmds::{handle_acp_command, run_acp_turn};
pub(crate) use commands::{REGISTRY, completion_fill, completion_matches_dyn};
pub use commands::{ReplCommand, ResumeArgs, SysAction, parse_command};
pub(crate) use format::build_user_message_with_attachments;
pub use format::{THINKING_STYLE, fmt_event, fmt_usage, format_core_error};
pub(crate) use handlers::{
    expand_skill_command, handle_model, handle_sys, handle_thinking, reload_agent,
};
pub(crate) use plugin_cmds::{handle_marketplace_command, handle_plugin_command};
pub(crate) use session::{
    dispatch_agent_event, handle_resume, maybe_overflow_compact, maybe_threshold_compact,
    persist_turn_messages, print_exit_hint,
};
pub(crate) use status::{
    SessionTotals, handle_compact, handle_context_window, handle_usage, turn_footer,
};
pub(crate) use user_cmds::{handle_feedback, handle_permissions};

/// Shared TUI handle: the composer plus its shutdown flag.
pub(crate) type TuiOpt = Option<(
    crate::composer::SharedTui,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
)>;

pub(crate) struct SessionState {
    pub(crate) store: gray_session::JsonlSessionStore,
    pub(crate) session_id: gray_session::SessionId,
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

/// Split a `/name argv…` line into (`/name`, argv words) for plugin
/// slash-command routing. `None` when the line isn't a slash command.
fn split_plugin_command(line: &str) -> Option<(String, Vec<String>)> {
    let mut words = line.trim().strip_prefix('/')?.split_whitespace();
    let first = words.next()?;
    if first.is_empty() {
        return None;
    }
    Some((format!("/{first}"), words.map(|w| w.to_string()).collect()))
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

/// 2E exit sweep: stop the session's background shell tasks (3 s deadline).
/// Covers the registry key in use plus `"nosession"` (pre-session turns).
async fn shutdown_shell_tasks(session_state: &Option<SessionState>, tui: &TuiOpt) {
    let mut keys = vec!["nosession".to_string()];
    if let Some(s) = session_state {
        keys.push(s.session_id.as_str().to_string());
    }
    keys.dedup();
    let mut stopped = 0;
    for k in &keys {
        stopped += crate::shell_drain::shutdown_shell_session(k).await;
    }
    if stopped > 0 {
        say(
            tui.as_ref().map(|(s, _)| s),
            &format!(
                "stopped {stopped} background task{}",
                if stopped == 1 { "" } else { "s" }
            ),
        );
    }
}

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
        shared.lock().expect("tui lock").modal_open = true;
    }
    let r = fut.await;
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").modal_open = false;
    }
    restore_viewport(tui);
    r
}

pub(crate) fn with_modal_sync<T>(
    tui: Option<&crate::composer::SharedTui>,
    f: impl FnOnce() -> T,
) -> T {
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").modal_open = true;
    }
    let r = f();
    if let Some(shared) = tui {
        shared.lock().expect("tui lock").modal_open = false;
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
    // auto-fetch provider context window in background if not yet cached and no user override
    // models.dev doubles as the reasoning-effort source for the thinking
    // picker, so it fetches unconditionally — gating it on the context
    // override starved the picker (unknown families fell back to the full
    // catalog, offering efforts the model rejects).
    tokio::spawn(crate::setup::fetch_models_dev_context());
    if crate::setup::get_user_context_window().is_none() {
        tokio::spawn(crate::setup::fetch_litellm_context_windows());
        tokio::spawn(crate::setup::fetch_openrouter_rates());
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

    // boot: no forced wizard. A dim hint appears when unconfigured,
    // and the provider picker fires the moment credentials are needed.
    tokio::spawn(spawn_ctrl_c_policy());
    // Shell drain (briefs 3A/2E): one process wake subscription for the
    // session filter below, plus the 7-day log sweep.
    crate::shell_drain::sweep_old_shell_logs();
    let _shell_drain = crate::shell_drain::spawn_shell_drain();

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
    // Sticky ACP session: `/acp <agent>` parks one here; prompts route
    // through it until `/acp off`. The native `agent` above sits idle meanwhile.
    let mut acp: Option<gray_acp::AcpSession> = None;
    let mut session_state: Option<SessionState> = None;
    let mut session_totals = SessionTotals::default();
    let mut pending_history: Vec<Message> = Vec::new();
    let mut resumed_session_info: Option<(SessionId, Vec<gray_session::SessionEntry>)> = None;

    // `--session <id>`: reopen that exact session.
    if let Some(id) = session_id
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let sid = SessionId::new(id);
        match store.load(&sid).await {
            Ok((meta, entries)) => {
                if config.model.is_none() && !meta.model.is_empty() {
                    config.model = Some(meta.model.clone());
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
                    session_id: sid.clone(),
                    store,
                });
                session_totals =
                    SessionTotals::from_entries(&entries, config.model.as_deref().unwrap_or(""));
                resumed_session_info = Some((sid, entries));
            }
            Err(e) => {
                println!("could not resume session {id}: {e}");
            }
        }
    }

    // `-c`: reopen the most recent session instead of starting blank.
    if resume_last
        && session_state.is_none()
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let summaries = store.list().await;
        let cwd_now = std::env::current_dir().ok();
        if let Some(latest) = crate::resume::latest_summary(&summaries, cwd_now.as_deref())
            .or_else(|| crate::resume::latest_summary(&summaries, None))
        {
            match store.load(&latest.id).await {
                Ok((meta, entries)) => {
                    if config.model.is_none() && !meta.model.is_empty() {
                        config.model = Some(meta.model.clone());
                    }
                    let history: Vec<Message> = entries.iter().map(|e| e.message.clone()).collect();
                    pending_history = history.clone();
                    if let Ok(built) = build_agent(config, &cwd, Some(latest.id.as_str())).await {
                        agent = Some(built.with_messages(history));
                    }
                    // T3.4 lifecycle: see the --session resume above.
                    if let Some(ledger) = gray_plugin::builder::current_file_ledger() {
                        ledger.clear();
                    }
                    session_state = Some(SessionState {
                        session_id: latest.id.clone(),
                        store,
                    });
                    session_totals = SessionTotals::from_entries(
                        &entries,
                        config.model.as_deref().unwrap_or(""),
                    );
                    resumed_session_info = Some((latest.id.clone(), entries));
                }
                Err(e) => println!("could not resume: {e}"),
            }
        }
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

    // request_user_input bridge: TUI overlay when interactive,
    // stdin prompts when piped.
    let question_bridge: gray_core::questions::QuestionBridge = if interactive {
        let shared = tui
            .as_ref()
            .map(|(s, _)| s.clone())
            .expect("interactive implies tui");
        gray_core::questions::QuestionBridge(std::sync::Arc::new(
            crate::composer::ComposerQuestionAsker { tui: shared },
        ))
    } else {
        gray_core::questions::QuestionBridge(std::sync::Arc::new(gray_tools::StdinQuestionAsker))
    };

    let approval_gate = gray_core::approvals::ApprovalGate::new(
        config
            .permissions
            .as_deref()
            .unwrap_or(gray_core::approvals::MODE_AUTO),
    );
    if let Some((shared, _)) = tui.as_ref() {
        shared
            .lock()
            .expect("tui lock")
            .set_permission_mode(approval_gate.mode());
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
    // The messaging gateway left gray core (the gray-gateway crate is
    // preserved and still runs via the `gray gateway` CLI): no autostart,
    // no boot card — the TUI starts clean.
    let mut pending_command: Option<ReplCommand> = None;
    let mut pending_images: Vec<std::path::PathBuf> = Vec::new();

    loop {
        if pending_command.is_none()
            && let Some((shared, _)) = tui.as_ref()
            && let Ok(mut t) = shared.try_lock()
            && !t.pending_question_answers.is_empty()
        {
            let texts = std::mem::take(&mut t.pending_question_answers);
            pending_command = Some(ReplCommand::Prompt(texts.join("\n\n")));
        }
        // Plugin-initiated `host/say` lines queued while a turn ran (cron
        // reports) surface here, through the composer when it owns the screen.
        for line in crate::host::take_host_say() {
            say(tui.as_ref().map(|(s, _)| s), &line);
        }
        // Shell wake drain (brief 3A): point the background subscription at
        // this session, then route queued exit/pattern notes.
        crate::shell_drain::set_drain_session(&crate::shell_drain::shell_session_key(
            session_state.as_ref().map(|s| s.session_id.as_str()),
        ));
        let mut cmd = if let Some(c) = pending_command.take() {
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
                            if let Some(s) = acp.take() {
                                s.shutdown().await;
                            }
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
                    if let Some(s) = acp.take() {
                        s.shutdown().await;
                    }
                    shutdown_shell_tasks(&session_state, &tui).await;
                    break;
                }
                (buf.trim().to_string(), Vec::new())
            };
            pending_images = images;
            if let Some((shared, _)) = tui.as_ref() {
                let pending = shared
                    .lock()
                    .expect("tui lock")
                    .pending_permission_mode
                    .take();
                if let Some(mode) = pending {
                    config.permissions = Some(mode.clone());
                    approval_gate.set_mode(&mode);
                    if let Ok(path) = crate::setup::saved_config_path() {
                        let mut saved = crate::setup::load_saved_config_at(&path);
                        saved.permissions = config.permissions.clone();
                        let _ = crate::setup::save_saved_config_at(&path, &saved);
                    }
                } else {
                    let gate_mode = approval_gate.mode();
                    let mut t = shared.lock().expect("tui lock");
                    if t.permission_mode() != gate_mode {
                        t.set_permission_mode(gate_mode.clone());
                        config.permissions = Some(gate_mode.clone());
                        if let Ok(path) = crate::setup::saved_config_path() {
                            let mut saved = crate::setup::load_saved_config_at(&path);
                            saved.permissions = config.permissions.clone();
                            let _ = crate::setup::save_saved_config_at(&path, &saved);
                        }
                    }
                }
            }
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
        // Route queued shell wakes: a turn about to run absorbs them via
        // steer (newest tool result at the next request); idle at the prompt
        // starts a synthetic follow-up turn as a system notice when
        // shell.wake_on_exit, else steers for the next turn.
        let wakes = crate::shell_drain::take_shell_wake();
        if !wakes.is_empty() {
            let joined = wakes.join("\n");
            match &cmd {
                ReplCommand::Prompt(_) => {
                    if let Some(a) = agent.as_mut() {
                        for w in wakes {
                            a.steer(w);
                        }
                    } else {
                        crate::shell_drain::queue_shell_wake(joined);
                    }
                }
                ReplCommand::Empty if pending_images.is_empty() => {
                    if crate::shell_drain::wake_on_exit() {
                        say(tui.as_ref().map(|(s, _)| s), &joined);
                        cmd = ReplCommand::Prompt(joined);
                    } else if let Some(a) = agent.as_mut() {
                        for w in wakes {
                            a.steer(w);
                        }
                        say(tui.as_ref().map(|(s, _)| s), &joined);
                    } else {
                        say(tui.as_ref().map(|(s, _)| s), &joined);
                        crate::shell_drain::queue_shell_wake(joined);
                    }
                }
                _ => {
                    if let Some(a) = agent.as_mut() {
                        for w in wakes {
                            a.steer(w);
                        }
                    } else {
                        crate::shell_drain::queue_shell_wake(joined);
                    }
                }
            }
        }

        match cmd {
            ReplCommand::Empty => {
                empty_turn::run_empty_turn(
                    &mut pending_images,
                    &mut agent,
                    config,
                    &cwd,
                    &tui,
                    interactive,
                    &mut session_state,
                    &mut session_totals,
                    &mut pending_history,
                    &mut unconfigured,
                    &question_bridge,
                    &approval_gate,
                )
                .await?;
            }
            ReplCommand::Prompt(prompt_text) => {
                if acp.is_some() {
                    run_acp_turn(
                        prompt_text,
                        &mut pending_images,
                        &mut acp,
                        config,
                        &cwd,
                        &tui,
                        interactive,
                        &mut session_state,
                        &mut session_totals,
                        &mut pending_command,
                        config.model.as_deref(),
                    )
                    .await?;
                } else {
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
                        &question_bridge,
                        &approval_gate,
                    )
                    .await?;
                }
            }
            other => {
                if dispatch::dispatch_command(
                    other,
                    &mut agent,
                    &mut acp,
                    config,
                    &cwd,
                    &tui,
                    &mut session_state,
                    &mut session_totals,
                    &mut pending_command,
                    &mut pending_history,
                    &mut unconfigured,
                    &mut hide_thinking,
                    &approval_gate,
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

#[cfg(test)]
mod ctrl_c_policy_tests {
    use super::*;

    #[test]
    fn sigint_second_press_within_window_exits() {
        // First press (no prior) never exits — verified by last==0 guard at
        // the call site; pure helper: far apart → false, close → true.
        assert!(!sigint_should_exit(
            1_000,
            1_000 + CTRL_C_EXIT_WINDOW_MS + 1
        ));
        assert!(sigint_should_exit(1_000, 1_000 + 1_000));
        assert!(sigint_should_exit(1_000, 1_000 + CTRL_C_EXIT_WINDOW_MS));
        // Clock skew backwards → wrapping_sub is huge → false.
        assert!(!sigint_should_exit(2_000, 1_000));
    }

    #[test]
    fn totals_sum_durations_and_skip_untimed() {
        let entry = |id: u64, duration_ms: Option<u64>| gray_session::SessionEntry {
            compaction_boundary: false,
            entry_id: id,
            parent_id: None,
            timestamp: 0,
            message: gray_core::message::Message::user("hi"),
            usage: Some(gray_core::event::Usage::new(10, 5)),
            duration_ms,
        };
        let entries = vec![entry(0, Some(6000)), entry(1, Some(4000)), entry(2, None)];
        let t = super::SessionTotals::from_entries(&entries, "test-persist-model");
        assert_eq!(t.turns, 3);
        assert_eq!(t.total_duration_ms, 10_000);
        assert_eq!(t.timed_turns, 2);
    }

    #[test]
    fn turn_footer_includes_duration_when_known() {
        let usage = gray_core::event::Usage::new(1000, 500);
        let totals = super::SessionTotals::default();
        let line = super::turn_footer(&usage, "test-persist-model", &totals, Some(6500));
        assert!(line.contains("6.5s"), "footer should show time: {line}");
        assert!(line.contains("tok"), "footer should keep tokens: {line}");
    }
}
