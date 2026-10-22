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

pub(crate) static TURN_STATE: StdMutex<Option<tokio_util::sync::CancellationToken>> =
    StdMutex::new(None);

pub(crate) static TURN_IN_FLIGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(crate) struct TurnInFlightGuard;

impl Drop for TurnInFlightGuard {
    fn drop(&mut self) {
        TURN_IN_FLIGHT.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

pub(crate) fn mark_turn_in_flight() -> TurnInFlightGuard {
    TURN_IN_FLIGHT.store(true, std::sync::atomic::Ordering::Relaxed);
    TurnInFlightGuard
}

const TURN_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

async fn drain_in_flight_turn(limit: std::time::Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    while TURN_IN_FLIGHT.load(std::sync::atomic::Ordering::Relaxed) {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    true
}

pub(crate) fn take_pending_history(h: &mut std::sync::Arc<Vec<Message>>) -> Vec<Message> {
    let arc = std::mem::replace(h, std::sync::Arc::new(Vec::new()));
    std::sync::Arc::try_unwrap(arc).unwrap_or_else(|a| (*a).clone())
}

fn signal_exit_text(session_id: Option<&str>, styled: bool) -> String {
    match session_id {
        Some(id) => format!("{}\r\n", session::exit_hint_line(id, styled)),
        None => String::new(),
    }
}

fn exit_from_signal(code: i32, clear_band: bool) -> ! {
    use std::io::IsTerminal as _;

    jobs::stop_all_blocking();
    gray_tools::shell::kill::kill_tracked_groups();
    if session::exit_hint_printed() {
        let _ = std::io::stdout().flush();
        std::process::exit(code);
    }

    let shared = clear_band.then(crate::host::registered_tui).flatten();
    let mut tui_guard = None;
    if let Some(shared) = &shared {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(250);
        loop {
            match shared.try_lock() {
                Ok(g) => {
                    tui_guard = Some(g);
                    break;
                }
                Err(std::sync::TryLockError::Poisoned(e)) => {
                    tui_guard = Some(e.into_inner());
                    break;
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }

    if let Some(t) = tui_guard.as_mut() {
        t.shutdown();
    } else {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = write!(std::io::stdout(), "\x1b[?25h\r\n");
    }
    if session::claim_exit_hint() {
        let _ = write!(
            std::io::stdout(),
            "{}",
            signal_exit_text(
                session::exit_session().as_deref(),
                std::io::stdout().is_terminal()
            )
        );
    }
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}

async fn spawn_ctrl_c_policy() {
    loop {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        let token = TURN_STATE.lock().ok().and_then(|mut g| g.take());
        if let Some(t) = token {
            t.cancel();
        } else {
            drain_in_flight_turn(TURN_DRAIN_TIMEOUT).await;

            exit_from_signal(0, true);
        }
    }
}

#[cfg(unix)]
async fn spawn_hangup_policy() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut hup), Ok(mut term)) = (
        signal(SignalKind::hangup()),
        signal(SignalKind::terminate()),
    ) else {
        return;
    };
    let code = tokio::select! {
        _ = hup.recv() => 129,
        _ = term.recv() => 143,
    };
    if let Some(t) = TURN_STATE.lock().ok().and_then(|mut g| g.take()) {
        t.cancel();
    }
    drain_in_flight_turn(TURN_DRAIN_TIMEOUT).await;

    exit_from_signal(code, code == 143);
}

#[cfg(not(unix))]
async fn spawn_hangup_policy() {}
use crate::config::Config;
use crate::{DEFAULT_SYS_PROMPT, build_agent, load_or_create_system_prompt_at};

pub mod attachments;
pub mod commands;
mod customize;
pub(crate) mod dispatch;
pub mod format;
pub(crate) mod gateway_panel;
pub(crate) mod handlers;
pub(crate) mod jobs;
pub(crate) mod key_watcher;
pub(crate) mod maintenance;
pub(crate) mod notice;
pub(crate) mod pager;
pub(crate) mod picker;
pub(crate) mod plugin_cmds;
pub(crate) mod prompt_turn;
mod session;
pub(crate) mod status;
pub(crate) mod usage_panel;
mod user_cmds;

pub(crate) use commands::{REGISTRY, completion_fill, completion_matches_dyn, is_builtin_command};
pub use commands::{ReplCommand, ResumeArgs, SysAction, parse_command};
pub(crate) use format::build_user_message_with_attachments;
pub use format::{THINKING_STYLE, fmt_usage, format_core_error};
pub(crate) use handlers::{
    expand_skill_command, handle_model, handle_sys, handle_undo, reload_agent,
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

pub(crate) type TuiOpt = Option<(
    crate::composer::SharedTui,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
)>;

pub(crate) struct SessionState {
    pub(crate) full_save_pending: bool,
    pub(crate) store: crate::session_store::JsonlSessionStore,
    pub(crate) session_id: crate::session_store::SessionId,

    pub(crate) _open_guard: Option<crate::session_store::SessionOpenGuard>,
}

pub(crate) fn say_lines(
    tui: Option<&crate::composer::SharedTui>,
    lines: Vec<ratatui::text::Line<'static>>,
) {
    if crate::ide::emit_output(
        &lines
            .iter()
            .map(|l| format!("{l}"))
            .collect::<Vec<_>>()
            .join("\n"),
    ) {
        return;
    }
    if let Some(t) = tui {
        let mut t = t.lock().expect("tui lock");
        t.push_styled_lines_with_hyperlinks(lines, &[], 0);
        t.ensure_gap();
    } else {
        for line in lines {
            println!("{line}");
        }
    }
}

pub(crate) fn say(tui: Option<&crate::composer::SharedTui>, msg: &str) {
    if crate::ide::emit_output(msg) {
        return;
    }
    if let Some(t) = tui {
        let mut t = t.lock().expect("tui lock");

        for line in msg.split('\n') {
            t.push_dim(format!("  {line}"));
        }

        t.ensure_gap();
    } else {
        println!("{msg}");
    }
}

pub(crate) fn say_err(tui: Option<&crate::composer::SharedTui>, msg: &str) {
    if crate::ide::emit_output(msg) {
        return;
    }
    if let Some(t) = tui {
        let mut t = t.lock().expect("tui lock");

        for line in msg.split('\n') {
            t.push_dim(format!("  {line}"));
        }

        t.ensure_gap();
    } else {
        eprintln!("{msg}");
    }
}

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
        let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
        let mut saved = crate::setup::load_saved_config_at(&path);

        saved.remember_effort(
            &crate::setup::effort_memory_key(
                &config.provider_id,
                &config.base_url,
                config.model.as_deref().unwrap_or_default(),
            ),
            &new,
        );
        let _ = crate::setup::save_saved_config_at(&path, &saved);
    }
    Some((old, new))
}

pub(crate) fn clamp_thinking_to_model(config: &mut Config) -> Option<(String, String)> {
    let model = config.model.clone().unwrap_or_default();
    clamp_thinking_to_model_name(config, &model)
}

pub(crate) fn normalize_adopted_model(config: &mut Config) -> Option<(String, String)> {
    crate::setup::canonicalize_adopted_model(config);
    clamp_thinking_to_model(config)
}

pub(crate) fn push_provider_connected(
    config: &mut Config,
    tui: &TuiOpt,
    hide_thinking: Option<&mut bool>,
) {
    crate::setup::set_active_model_provider(&config.base_url);
    let clamped = clamp_thinking_to_model(config);
    if let Some(h) = hide_thinking {
        *h = config.reasoning_hidden();
    }
    let Some((shared, _)) = tui else {
        if let Some((old, new)) = clamped {
            say(
                None,
                &format!(
                    "Thinking effort clamped from {old} to {new} (not supported by this model)"
                ),
            );
        }
        return;
    };
    let mut t = shared.lock().expect("tui lock");
    if let Some(m) = &config.model {
        t.set_model(m.clone());
        t.set_model_label(crate::setup::composite_label_for(config));
        t.set_model_label_parts(crate::setup::composite_label_parts_for(config));
    }

    if let Some(eff) = &config.thinking_effort {
        t.set_thinking_effort(eff.clone());
        t.set_hide_thinking(config.reasoning_hidden());
    }

    let prov_name = connected_provider_name(config);
    let detail = model_receipt_label(config);
    t.push_action(&format!("Connected to {prov_name}"), detail.as_deref());
    if let Some((old, new)) = clamped {
        notice::push_notice(
            &mut t,
            "Thinking effort clamped",
            &format!("from {old} to {new} (not supported by this model)"),
            false,
        );
    }
    t.ensure_gap();
    let _ = t.draw();
}

pub(crate) fn model_receipt_label(config: &Config) -> Option<String> {
    let m = config.model.as_ref()?;
    Some(
        match (
            &config.thinking_effort,
            crate::setup::composite_label_for(config),
        ) {
            (_, Some(composite)) if config.fast_mode == Some(true) => {
                format!("{composite} · fast")
            }
            (_, Some(composite)) => composite,
            (Some(eff), None) if crate::setup::supported_thinking_levels(m).len() > 1 => {
                format!("{m} · {}", crate::setup::effort_chip(eff, config))
            }
            _ => m.clone(),
        },
    )
}

fn connected_provider_name(config: &Config) -> String {
    if let Ok(home) = crate::setup::gray_home()
        && let Some(installed) = crate::providers::resolve_provider_connection(config, &home)
        && !installed.provider.name.trim().is_empty()
    {
        return installed.provider.name.clone();
    }
    if let Some(name) = crate::setup::load_catalog().ok().and_then(|c| {
        c.values()
            .find(|p| p.base_url == config.base_url)
            .map(|p| p.name.clone())
    }) {
        return name;
    }
    endpoint_host(&config.base_url).unwrap_or_else(|| "provider".to_string())
}

fn endpoint_host(base_url: &str) -> Option<String> {
    let rest = base_url.split_once("://").map_or(base_url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next()?.trim();
    (!host.is_empty()).then(|| host.to_string())
}

fn split_plugin_command(line: &str) -> Option<(String, Vec<String>)> {
    let words = shlex::split(line.trim().strip_prefix('/')?)?;
    let (first, rest) = words.split_first()?;
    if first.is_empty() {
        return None;
    }
    Some((format!("/{first}"), rest.to_vec()))
}

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

async fn shutdown_shell_tasks(
    agent: Option<&gray_core::agent::Agent>,
    session_state: &Option<SessionState>,
    cwd: &std::path::Path,
) {
    if let Some(agent) = agent {
        let sid = session_state.as_ref().map(|s| s.session_id.as_str());
        jobs::stop_all(
            agent.executor_handle().as_ref(),
            &jobs::session_ctx(cwd, sid),
        )
        .await;
    }
}

async fn shutdown_hooks(agent: Option<&gray_core::agent::Agent>) {
    let hooks: Vec<Arc<dyn PluginHooks>> = agent.map(|a| a.hooks().to_vec()).unwrap_or_default();
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        futures::future::join_all(hooks.iter().map(|h| h.shutdown())),
    )
    .await;
}

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

pub(crate) const CONTINUE_PROMPT: &str = "continue";

pub(crate) const CONTINUE_GHOST: &str = "Please continue…";

pub async fn run_repl_mode(
    config: &mut Config,
    resume_last: bool,
    session_id: Option<&str>,
) -> anyhow::Result<()> {
    let _ = crossterm::terminal::disable_raw_mode();
    crate::tui::clear_screen();
    let cwd = std::env::current_dir()?;
    log::debug!(target: "gray_timing", "repl start elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());
    tokio::task::spawn_blocking(crate::setup::context::warm_rates_from_disk_cache);
    // auth.json v1→v2: single-envelope plugin entries become account
    // pools. Backs up to auth.json.v1.bak before rewriting; a no-op once
    // migrated (and when there are no plugin entries at all).
    if let Ok(path) = crate::setup::catalog::auth_store_path() {
        let store = crate::auth::CredentialStore::new(path);
        if let Err(e) = store.migrate_to_v2() {
            log::warn!("auth store v2 migration skipped: {e:#}");
        }
    }

    use std::io::IsTerminal;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if interactive && std::env::var("TERM").is_ok_and(|t| t.trim() == "dumb") {
        eprintln!(
            "gray: the interactive TUI cannot run on TERM=dumb — use `gray -p \"...\"` or a real terminal"
        );
        std::process::exit(2);
    }
    if interactive {
        gray_plugin::builder::set_autostart(true);
    }

    crate::composer::input::watch_dead_stdin();

    crate::setup::set_user_context_window(config.context_window);
    crate::setup::set_user_reserve_tokens(config.context_reserve);
    crate::setup::set_user_keep_recent_tokens(config.context_keep);
    crate::setup::set_active_model_provider(&config.base_url);

    let offline = crate::config::offline();
    // Flag-based connects (--base-url/--model) skip /models, so lean mode
    // still owes the endpoint one cheap /models probe: it fills the context
    // cache so the UI shows the endpoint's real context_length, not the 256k
    // fallback. Result is cached on disk for later runs.
    let custom_base = crate::setup::normalize_custom_base_url(&config.base_url)
        != crate::setup::normalize_custom_base_url(crate::config::DEFAULT_BASE_URL);
    let needs_endpoint_probe = !config.lean
        || (custom_base && config.context_window.is_none() && {
            crate::setup::context::ensure_disk_loaded();
            config
                .model
                .as_deref()
                .is_some_and(|m| crate::setup::get_cached_model_context(m).is_none())
        });
    if !offline && !config.uses_plugin_credentials() && !config.bare && needs_endpoint_probe {
        let base = config.base_url.clone();
        let key = config.api_key.clone();
        tokio::task::spawn_blocking(move || {
            crate::setup::fetch_live_provider_models(&base, key.as_deref());
        });
    }
    if !offline && !config.bare {
        tokio::spawn(crate::setup::fetch_models_dev_context());
        tokio::spawn(crate::setup::fetch_recommended_models());
        if crate::setup::get_user_context_window().is_none() {
            tokio::spawn(crate::setup::fetch_litellm_context_windows());
            tokio::spawn(crate::setup::fetch_openrouter_rates());
        }
    }

    tokio::spawn(spawn_ctrl_c_policy());
    tokio::spawn(spawn_hangup_policy());

    crate::ask::install(None, interactive);

    std::thread::spawn(crate::shell_drain::sweep_old_shell_logs);

    let mut unconfigured = config.model.is_none();

    if unconfigured && interactive {
        let ready = crate::setup::run_onboarding(config).await?;

        unconfigured = config.model.is_none();
        if !ready {
            print!(
                "\r\x1b[2mrunning without a provider — send a message to set one up (or /models)\x1b[0m\r\n"
            );
        }
        print!("\r\n");
        crate::setup::set_active_model_provider(&config.base_url);

        crate::setup::set_user_context_window(config.context_window);
        crate::setup::set_user_reserve_tokens(config.context_reserve);
        crate::setup::set_user_keep_recent_tokens(config.context_keep);

        if !offline && crate::setup::get_user_context_window().is_none() {
            tokio::spawn(crate::setup::fetch_litellm_context_windows());
            if let Some(m) = config.model.clone()
                && crate::setup::get_cached_model_context(&m).is_none()
            {
                let base = config.base_url.clone();
                let key = config.api_key.clone();
                tokio::task::spawn_blocking(move || {
                    crate::setup::fetch_live_provider_models(&base, key.as_deref());
                });
            }
        }
    }

    let mut agent: Option<Agent> = None;
    let mut session_state: Option<SessionState> = None;
    let mut session_totals = SessionTotals::default();
    let mut pending_history: std::sync::Arc<Vec<Message>> = std::sync::Arc::new(Vec::new());
    let mut resumed_session_info: Option<(SessionId, std::sync::Arc<Vec<Message>>)> = None;

    type Resumed = (
        SessionId,
        crate::session_store::SessionMeta,
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
            Err(e) => {
                say(None, &format!("could not resume session {id}: {e}"));
            }
        }
    }

    if resume_last
        && loaded.is_none()
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let cwd_now = std::env::current_dir().ok();

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
                            say(None, &format!("could not resume: {e}"));
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

    log::debug!(target: "gray_timing", "repl session_load_done elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());
    if let Some((sid, meta, entries, store)) = loaded {
        let open_guard = Some(store.acquire_open(&sid).await?);
        if config.model.is_none() && !meta.model.is_empty() {
            config.model = Some(meta.model.clone());
        }

        if let Some((old, new)) = normalize_adopted_model(config) {
            say(
                None,
                &format!(
                    "Thinking effort clamped from {old} to {new} (not supported by this model)"
                ),
            );
        }
        if let Some(ledger) = gray_plugin::builder::current_file_ledger() {
            ledger.clear();
        }
        session_state = Some(SessionState {
            full_save_pending: false,
            session_id: sid.clone(),
            store,
            _open_guard: open_guard,
        });
        log::debug!(target: "gray_timing", "repl open_done elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());
        session::export_session_env(sid.as_str());
        session_totals =
            SessionTotals::from_entries(&entries, config.model.as_deref().unwrap_or(""));
        log::debug!(target: "gray_timing", "repl totals_done elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());
        let shared_history = std::sync::Arc::new(
            entries
                .into_iter()
                .map(|e| e.message)
                .collect::<Vec<Message>>(),
        );
        pending_history = shared_history.clone();
        resumed_session_info = Some((sid, shared_history));
    }

    if let Some((old, new)) = normalize_adopted_model(config) {
        say(
            None,
            &format!("Thinking effort clamped from {old} to {new} (not supported by this model)"),
        );
    }

    let startup_hint = if interactive && resumed_session_info.is_none() {
        let cwd2 = cwd.clone();
        Some(tokio::spawn(async move {
            crate::resume::startup_interrupted_hint(&cwd2).await
        }))
    } else {
        None
    };
    log::debug!(target: "gray_timing", "repl pre_tui elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());
    let tui: TuiOpt = if interactive {
        use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
        if crossterm::terminal::size().is_err() {
            anyhow::bail!(
                "not a terminal — the interactive composer needs a real terminal \
                 (could not query terminal size); pipe input or run under a TTY"
            );
        }
        let mut t = {
            let t = if resumed_session_info.is_some() {
                crate::composer::Tui::new_no_intro()
            } else {
                crate::composer::Tui::new()
            }
            .map_err(|e| {
                anyhow::anyhow!(
                    "not a terminal — the interactive composer needs a real terminal \
                 (cursor position could not be read: {e:#}); pipe input or run under a TTY"
                )
            })?;
            log::debug!(target: "gray_timing", "repl tui_new elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());
            t
        };
        if let Some(m) = &config.model {
            t.set_model(m.clone());
            t.set_model_label(crate::setup::composite_label_for(config));
            t.set_model_label_parts(crate::setup::composite_label_parts_for(config));
        }

        let _ = clamp_thinking_to_model(config);
        if let Some(eff) = &config.thinking_effort {
            t.set_thinking_effort(eff.clone());
        }
        t.set_hide_thinking(config.reasoning_hidden());
        t.set_cwd(cwd.display().to_string());

        if let Some((sid, shared_history)) = resumed_session_info.take() {
            let n_entries = shared_history.len();
            let _ = t.draw();
            log::debug!(target: "gray_timing", "repl first_draw elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());

            let tool_defs = if config.bare {
                Vec::new()
            } else {
                tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    crate::plugin_status::booted_tool_defs(
                        Some(crate::host::default_handler(cwd.to_path_buf())),
                        crate::extra_surface_plugins(config, true),
                    ),
                )
                .await
                .ok()
                .flatten()
                .unwrap_or_default()
            };
            let (tool_labels, tool_previews) = session::tool_display_maps(&tool_defs);
            let replay_started = std::time::Instant::now();
            session::note_resume_trailer(sid.as_str());
            t.replay_session_history_with_tools(
                shared_history,
                &cwd,
                crate::tool_fmt::ToolDisplay {
                    labels: Some(&tool_labels),
                    previews: Some(&tool_previews),
                },
                vec![format!(
                    "{} Resumed session {} ({} messages)",
                    crate::tui::glyph("\u{2b22}", "#"),
                    sid.as_str(),
                    n_entries
                )],
            );
            log::debug!(target: "gray_timing", "repl history_replay elapsed_ms={} entries={}", replay_started.elapsed().as_millis(), n_entries);
        }
        let shared = std::sync::Arc::new(std::sync::Mutex::new(t));
        crate::host::register_tui(&shared);
        if let Some(task) = startup_hint {
            let shared = shared.clone();
            tokio::spawn(async move {
                if let Ok(Some(h)) = task.await
                    && let Ok(mut t) = shared.lock()
                {
                    if t.is_home() {
                        t.set_home_notice(Some(h));
                    } else {
                        t.push_dim(h);
                        t.ensure_gap();
                    }
                }
            });
        }

        crate::ask::install(Some(shared.clone()), interactive);
        if let Ok(mut t) = shared.lock() {
            t.enable_reveal_pacing();
        }
        log::debug!(target: "gray_timing", "repl ready elapsed_ms={}", crate::turn_caps::process_start().elapsed().as_millis());
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let ticker_stop = stop.clone();
        let ticker_tui = shared.clone();
        tokio::spawn(async move {
            let mut last_status = std::time::Instant::now();
            let exe_watch = crate::exe_watch::ExeWatch::capture();
            let mut last_exe_check = std::time::Instant::now();
            let mut exe_notified = None;
            loop {
                if ticker_stop.load(AtomicOrdering::Relaxed) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(16)).await;
                if ticker_stop.load(AtomicOrdering::Relaxed) {
                    break;
                }
                if let Ok(mut t) = ticker_tui.try_lock() {
                    if ticker_stop.load(AtomicOrdering::Relaxed) {
                        break;
                    }
                    t.tick_reveal();
                    if last_status.elapsed() >= std::time::Duration::from_millis(100) {
                        last_status = std::time::Instant::now();
                        t.tick_status();
                    }
                    if last_exe_check.elapsed() >= std::time::Duration::from_secs(30) {
                        last_exe_check = std::time::Instant::now();
                        if let Some(w) = exe_watch.as_ref()
                            && w.replaced()
                            && exe_notified != w.disk_id()
                        {
                            exe_notified = w.disk_id();
                            let notice = "gray was updated — /restart to load it".to_string();
                            if t.is_home() {
                                t.set_home_update(Some(notice));
                            } else {
                                t.push_dim(notice);
                            }
                        }
                    }
                }
            }
        });
        Some((shared, stop))
    } else {
        None
    };

    if let Some((shared, _)) = &tui {
        let shared = shared.clone();
        let bare = config.bare;
        tokio::spawn(async move {
            if let Some(outcome) = crate::auto_update::maybe_run(bare).await
                && let Some(line) = outcome.notice()
                && let Ok(mut t) = shared.lock()
            {
                if t.is_home() {
                    t.set_home_update(Some(line));
                } else {
                    t.push_dim(line);
                    t.ensure_gap();
                }
            }
        });
    }

    if interactive
        && !config.bare
        && let Some((shared, _)) = &tui
    {
        tokio::spawn(crate::quality_of_life::detect::run_for_home(shared.clone()));
    }

    if interactive
        && !config.bare
        && let Some((shared, _)) = &tui
    {
        if agent.is_none() {
            crate::plugin_status::kickoff(
                shared.clone(),
                crate::host::default_handler(cwd.to_path_buf()),
            );
        } else if let Ok(mut t) = shared.lock() {
            t.set_plugin_stats(crate::plugin_status::stats_line());
        }
    }

    if interactive
        && !cfg!(windows)
        && let Ok(home) = crate::setup::gray_home()
    {
        let watcher_tui = tui.as_ref().map(|(shared, _)| shared.clone());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let Some(sid) = crate::host::live_session() else {
                    continue;
                };
                if crate::session_mailbox::session_inbox_pending(&home, &sid) {
                    crate::host::request_wake();
                }
                // Keep the "Queued follow-up inputs" preview fresh even
                // mid-turn: peek (non-consuming) the session's sends and
                // repaint only when the set changed.
                if let Some(shared) = &watcher_tui
                    && let Ok(mut t) = shared.try_lock()
                {
                    let pending: Vec<(String, Vec<std::path::PathBuf>)> =
                        crate::session_send::peek_sends(&home, &sid)
                            .into_iter()
                            .map(|m| {
                                (
                                    crate::session_send::prompt_for(&m.from, &m.text),
                                    m.attachments,
                                )
                            })
                            .collect();
                    if t.queued_sends != pending {
                        t.queued_sends = pending;
                        let _ = t.draw();
                    }
                }
            }
        });
    }

    let mut hide_thinking = config.reasoning_hidden();

    if config.thinking_effort.is_none()
        && crate::setup::model_supports_reasoning(&config.model.clone().unwrap_or_default())
            != Some(false)
    {
        config.thinking_effort = Some("high".to_string());
        if let Ok(path) = crate::setup::saved_config_path() {
            let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
            let mut saved = crate::setup::load_saved_config_at(&path);
            if saved.thinking_effort.is_none() {
                saved.thinking_effort = Some("high".to_string());
                let _ = crate::setup::save_saved_config_at(&path, &saved);
            }
        }
    }

    let mut pending_command: Option<ReplCommand> = None;
    let mut pending_images: Vec<std::path::PathBuf> = Vec::new();

    let mut last_turn_resumable = false;

    let mut bg_wake: Option<tokio::task::JoinHandle<()>> = None;

    if interactive {
        jobs::spawn_footer_poller();
    }

    if interactive && tui.is_some() && crate::quality_of_life::detect::take_pending_setup() {
        crate::quality_of_life::setup::handle(&tui, None).await;
    }

    loop {
        crate::host::clear_wake();

        session::remember_exit_session(session_state.as_ref().map(|s| s.session_id.as_str()));

        for lines in crate::host::take_host_say() {
            say_lines(tui.as_ref().map(|(s, _)| s), lines);
        }

        if interactive && pending_command.is_none() {
            let sid = session_state
                .as_ref()
                .map(|s| s.session_id.as_str().to_string());
            crate::host::set_live_session(sid.as_deref());
            crate::host::set_background_source(
                agent
                    .as_ref()
                    .map(|a| (a.executor_handle(), jobs::session_ctx(&cwd, sid.as_deref()))),
            );
            if let Some(h) = bg_wake.take() {
                h.abort();
            }

            bg_wake = notice::arm_background_wake(agent.as_ref(), sid.as_deref(), &cwd);
            if let Some((cards, prompt, sends)) =
                notice::idle_wake(agent.as_ref(), sid.as_deref(), &cwd)
            {
                for card in &cards {
                    match (card, tui.as_ref()) {
                        (notice::WakeCard::Cron(c), Some((shared, _))) => {
                            let mut t = shared.lock().expect("tui lock");
                            let (header, body) = notice::cron_card_lines(c, t.width());
                            t.push_tool_box(header, body);
                        }
                        (notice::WakeCard::Job(j), Some((shared, _))) => {
                            let (header, body) = notice::job_card_lines(j);
                            shared.lock().expect("tui lock").push_tool_box(header, body);
                        }
                        (notice::WakeCard::Job(j), None) => say(
                            None,
                            &format!(
                                "background job {} finished ({}) after {} \u{b7} log {}",
                                j.id, j.outcome, j.elapsed, j.log
                            ),
                        ),
                        (notice::WakeCard::Cron(c), None) => say(
                            None,
                            &format!(
                                "cron: {}",
                                crate::repl::notice::format_delivery_plain(
                                    &c.name, &c.body, c.reminder, c.failed
                                )
                            ),
                        ),
                        (notice::WakeCard::Text(text), _) => {
                            say(tui.as_ref().map(|(s, _)| s), text);
                        }
                    }
                }

                if let Some(msg) =
                    crate::turn_caps::check_caps(config, session_totals.turns, session_totals.cost)
                {
                    say(tui.as_ref().map(|(s, _)| s), &msg);
                    crate::session_send::hold_back(&sends);
                } else {
                    let mut images = Vec::new();
                    for m in &sends {
                        let line = crate::session_send::prompt_for(&m.from, &m.text);
                        match tui.as_ref() {
                            Some((shared, _)) => {
                                let mut t = shared.lock().expect("tui lock");
                                t.push_user_prompt(&line, &m.attachments, true);
                                let _ = t.draw();
                            }
                            None => say(None, &line),
                        }
                        images.extend(m.attachments.iter().cloned());
                    }

                    if let Some((shared, _)) = tui.as_ref() {
                        let mut t = shared.lock().expect("tui lock");
                        if let (Ok(home), Some(sid)) = (crate::setup::gray_home(), sid.as_deref()) {
                            t.queued_sends = crate::session_send::peek_sends(&home, sid)
                                .into_iter()
                                .map(|m| {
                                    (
                                        crate::session_send::prompt_for(&m.from, &m.text),
                                        m.attachments,
                                    )
                                })
                                .collect();
                        }
                    }
                    crate::session_send::mark_delivered(&sends);
                    let turn = prompt_turn::run_prompt_turn(
                        prompt,
                        &mut images,
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
                        &mut last_turn_resumable,
                    )
                    .await;

                    if turn.is_err() || agent.is_none() {
                        crate::session_send::hold_back(&sends);
                    }
                    turn?;
                }
                continue;
            }
        }
        let cmd = if let Some(c) = pending_command.take() {
            c
        } else {
            if let Some((shared, _)) = tui.as_ref() {
                shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .allow_empty_submit = last_turn_resumable && agent.is_some();
            }
            let (line_text, images) = if interactive {
                let (shared, stop) = tui.as_ref().expect("interactive implies tui");
                let (txt, imgs) = {
                    let pair = match crate::composer::input::read_line(shared)? {
                        Some(v) => v,
                        None => {
                            let sid = session_state.as_ref().map(|s| s.session_id.as_str());
                            let exec = agent.as_ref().map(|a| a.executor_handle());
                            if let Some(warning) =
                                jobs::confirm_quit(exec.as_deref(), &jobs::session_ctx(&cwd, sid))
                            {
                                say(Some(shared), &warning);
                                continue;
                            }
                            stop.store(true, std::sync::atomic::Ordering::Relaxed);
                            shared.lock().expect("tui lock").shutdown();
                            print_exit_hint(&session_state);
                            crate::ask::shutdown();
                            shutdown_hooks(agent.as_ref()).await;
                            shutdown_shell_tasks(agent.as_ref(), &session_state, &cwd).await;
                            break;
                        }
                    };

                    if pair.0.is_empty() && pair.1.is_empty() && crate::host::wake_requested() {
                        continue;
                    }

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
                    crate::ask::shutdown();
                    shutdown_hooks(agent.as_ref()).await;
                    shutdown_shell_tasks(agent.as_ref(), &session_state, &cwd).await;
                    break;
                }
                (buf.trim().to_string(), Vec::new())
            };
            pending_images = images;
            let parsed = parse_command(&line_text);

            if parsed.runs_local()
                && let Some((shared, _)) = tui.as_ref()
            {
                shared
                    .lock()
                    .expect("tui lock")
                    .retract_slash_echo(&line_text);
            }
            expand_skill_command(parsed, cwd.as_path(), tui.as_ref().map(|(s, _)| s), false)
        };

        if !matches!(&cmd, ReplCommand::Prompt(_) | ReplCommand::Empty) {
            pending_images.clear();
        }
        match cmd {
            ReplCommand::Empty => {
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
                        &mut last_turn_resumable,
                    )
                    .await?;
                } else if last_turn_resumable && agent.is_some() {
                    if let Some(msg) = crate::turn_caps::check_caps(
                        config,
                        session_totals.turns,
                        session_totals.cost,
                    ) {
                        say(tui.as_ref().map(|(s, _)| s), &msg);
                        shutdown_shell_tasks(agent.as_ref(), &session_state, &cwd).await;
                        break;
                    }
                    if let Some((shared, _)) = tui.as_ref() {
                        shared.lock().expect("tui lock").push_user_prompt(
                            CONTINUE_PROMPT,
                            &[],
                            true,
                        );
                    } else {
                        say(
                            None,
                            &format!("{} {CONTINUE_PROMPT}", crate::tui::arrow_glyph()),
                        );
                    }
                    prompt_turn::run_prompt_turn(
                        CONTINUE_PROMPT.to_string(),
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
                        &mut last_turn_resumable,
                    )
                    .await?;
                }
            }
            ReplCommand::Prompt(prompt_text) => {
                if let Some(msg) =
                    crate::turn_caps::check_caps(config, session_totals.turns, session_totals.cost)
                {
                    say(tui.as_ref().map(|(s, _)| s), &msg);
                    shutdown_shell_tasks(agent.as_ref(), &session_state, &cwd).await;
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
                    &mut last_turn_resumable,
                )
                .await?;
            }
            other => {
                if matches!(
                    other,
                    ReplCommand::New(_) | ReplCommand::Resume(_) | ReplCommand::Compact(_)
                ) {
                    last_turn_resumable = false;
                }
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
                    shutdown_shell_tasks(agent.as_ref(), &session_state, &cwd).await;
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
