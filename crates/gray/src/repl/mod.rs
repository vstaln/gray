//! Interactive REPL mode for Gray.
// 2 turn loops (~400 lines) + 3 provider blocks duplicated; extract ensure_provider + run_turn when adding streaming resume.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gray_core::agent::{Agent, CommandOutcome, PluginHooks, ToolContext};
use gray_core::error::CoreError;
use gray_core::event::AgentEvent;
use gray_core::message::Message;
use gray_session::{JsonlSessionStore, SessionId, SessionMeta, default_root};

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

/// Static slash-command table driving both `/help` and the autocomplete panel.
/// True while an agent turn is in flight: `Some(token)` cancels on first
/// Ctrl-C (token consumed), a second press — or any press at the prompt —
/// exits. Single mutex = no TOCTOU between flag and token.
static TURN_STATE: StdMutex<Option<tokio_util::sync::CancellationToken>> = StdMutex::new(None);

/// Installs the single global Ctrl-C policy:
/// - during a turn: cancel the turn (first press), the turn handler reports it
/// - at the prompt: exit cleanly
async fn spawn_ctrl_c_policy() {
    loop {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        let token = TURN_STATE.lock().ok().and_then(|mut g| g.take());
        if let Some(t) = token {
            t.cancel(); // first press mid-turn: cancel, stay alive
        } else {
            // Say something — a bare exit(0) mid-turn looks like a crash.
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = write!(
                std::io::stdout(),
                "\x1b[?25h\r\n\x1b[2m(interrupted — bye)\x1b[0m\r\n"
            );
            let _ = std::io::stdout().flush();
            std::process::exit(0);
        }
    }
}
use crate::config::Config;
use crate::{DEFAULT_SYS_PROMPT, build_agent, load_or_create_system_prompt_at};

pub mod attachments;
pub mod commands;
mod dispatch;
mod empty_turn;
pub mod format;
mod gateway_cmds;
mod handlers;
mod key_watcher;
mod prompt_turn;
mod session;
mod status;

pub(crate) use commands::{REGISTRY, completion_matches_dyn};
pub use commands::{ReplCommand, ResumeArgs, SysAction, parse_command};
pub(crate) use format::build_user_message_with_attachments;
pub use format::{THINKING_STYLE, fmt_event, fmt_usage, format_core_error};
pub(crate) use gateway_cmds::{
    GATEWAY_HANDLE, gateway_boot_rows, handle_gateway, spawn_gateway_boot_watcher,
    start_gateway_in_background,
};
// Test-only surface (kept out of the non-test build to satisfy unused-imports).
#[cfg(test)]
pub(crate) use gateway_cmds::{
    GatewayAction, PairingArgs, apply_connect, apply_disconnect, apply_enable,
    gateway_status_lines, parse_gateway_args,
};
pub(crate) use handlers::{
    expand_skill_command, handle_model, handle_sys, handle_thinking, reload_agent,
};
pub(crate) use session::{
    dispatch_agent_event, handle_resume, maybe_overflow_compact, maybe_threshold_compact,
    persist_turn_messages, print_exit_hint,
};
pub(crate) use status::{
    SessionTotals, handle_compact, handle_context_window, handle_usage, turn_footer,
};

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
fn say(tui: Option<&crate::composer::SharedTui>, msg: &str) {
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
        // Breathing room when a modal is dismissed with no output: command
        // cards skip their trailing gap, so without this the next prompt
        // would jam against the card. Idempotent after output with a gap.
        t.ensure_gap(1);
        let cols = crossterm::terminal::size()
            .map(|(c, _)| c)
            .unwrap_or(t.last_width);
        if cols == t.last_width {
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

fn with_modal_sync<T>(tui: Option<&crate::composer::SharedTui>, f: impl FnOnce() -> T) -> T {
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
    if crate::setup::get_user_context_window().is_none() {
        tokio::spawn(crate::setup::fetch_litellm_context_windows());
        tokio::spawn(crate::setup::fetch_models_dev_context());
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

    let mut unconfigured = config.model.is_none();
    if unconfigured {
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
        if crate::setup::get_user_context_window().is_none() {
            tokio::spawn(crate::setup::fetch_litellm_context_windows());
            tokio::spawn(crate::setup::fetch_models_dev_context());
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
    let tui = interactive.then(|| {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        let shared = std::sync::Arc::new(std::sync::Mutex::new({
            let mut t = crate::composer::Tui::new().expect("composer init");
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
        (shared, stop)
    });

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

    // pi's hideThinkingBlock — toggled with /thinking, session-only.
    // Reasoning is ON by default — user wants to see thinking (high effort).
    // Bare /thinking toggles visibility; picker sets level persisted to config.
    let mut hide_thinking = config.reasoning_hidden();
    // Wire default: if no effort saved yet, enable reasoning so ThinkingDelta
    // actually streams on openrouter/zen etc. Persist once so future sessions
    // keep it without relying on this default branch.
    if config.thinking_effort.is_none() {
        config.thinking_effort = Some("high".to_string());
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            if saved.thinking_effort.is_none() {
                saved.thinking_effort = Some("high".to_string());
                let _ = crate::setup::save_saved_config_at(&path, &saved);
            }
        }
    }
    // Gateway autostart (default off; /gateway autostart on to enable): boot the
    // in-process daemon when any
    // platform is enabled. Silent when nothing is configured. Shows a LIVE
    // boot panel above the input (per-platform connecting → connected as …
    // plus a shimmer-bar line); when every platform resolves, the final
    // state is committed as ONE card with no trailing gap. No follow-ups.
    if let Some((shared, _)) = tui.as_ref() {
        let cfg = gray_gateway::config::load_gateway_config();
        if cfg.autostart
            && cfg.platforms.values().any(|p| p.enabled)
            && let Some(board) = start_gateway_in_background(Some(shared))
        {
            shared
                .lock()
                .expect("tui lock")
                .begin_gateway_boot("Gateway autostarted", &board);
            spawn_gateway_boot_watcher(shared.clone(), board);
        }
    }
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
                )
                .await?;
            }
            ReplCommand::Prompt(prompt_text) => {
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
                    break;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
