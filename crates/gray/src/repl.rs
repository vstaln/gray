//! Interactive REPL mode for Gray.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gray_core::agent::{Agent, ToolContext};
use gray_core::error::CoreError;
use gray_core::event::AgentEvent;
use gray_core::message::Message;
use gray_session::{
    default_root, JsonlSessionStore, SessionId, SessionMeta, SessionStore, SessionSummary,
};

use std::sync::Mutex as StdMutex;


/// Static slash-command table driving both `/help` and the autocomplete panel.
pub(crate) const COMMANDS: &[(&str, &str)] = &[
    ("connect", "connect a provider & setup API key"),
    ("new", "start a fresh conversation"),
    ("compact", "compress conversation context into a structured summary"),
    ("compress", "alias for /compact"),
    ("model", "switch or pick a model"),
    ("provider", "configure provider (API key, accounts, free tier)"),
    ("key", "add or update a provider API key (input hidden)"),
    ("sys", "view, edit, or restore the system prompt"),
    ("thinking", "toggle hiding reasoning (shows a Thinking… label)"),
    ("help", "print the command list"),
    ("quit", "exit (Ctrl-C exits at the prompt, cancels mid-turn)"),
];

/// Commands matching `filter` (the text after '/'), auto-sorted by relevance.
pub(crate) fn completion_matches(filter: &str) -> Vec<(&'static str, &'static str)> {
    let f = filter.to_lowercase();
    let mut matches: Vec<(&'static str, &'static str)> = COMMANDS
        .iter()
        .filter(|(n, desc)| n.to_lowercase().contains(&f) || desc.to_lowercase().contains(&f))
        .copied()
        .collect();

    matches.sort_by_key(|(n, _)| {
        let nl = n.to_lowercase();
        if nl == f {
            0
        } else if nl.starts_with(&f) {
            1
        } else {
            2
        }
    });

    matches
}

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
            let _ = write!(std::io::stdout(), "\x1b[?25h\r\n\x1b[2m(interrupted — bye)\x1b[0m\r\n");
            let _ = std::io::stdout().flush();
            std::process::exit(0);
        }
    }
}
use crate::{build_agent, load_or_create_system_prompt_at, DEFAULT_SYS_PROMPT};
use crate::config::Config;

/// Maximum characters of error output rendered in tool results.
const MAX_ERROR_DISPLAY_CHARS: usize = 200;

/// Truncates a string slice to at most `max_chars` unicode scalar values / chars.
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Formats a token count: `<1_000` as-is, else one decimal place with `k`.
pub fn fmt_usage(total: usize) -> String {
    if total < 1_000 {
        format!("{total}")
    } else {
        format!("{:.1}k", total as f64 / 1000.0)
    }
}

/// ANSI dim + italic — pi's styling for rendered thinking blocks
/// (italic muted color; dim stands in for pi's `thinkingText` theme color).
pub const THINKING_STYLE: &str = "\x1b[2m\x1b[3m";

/// Formats an [`AgentEvent`] for display in the interactive REPL.
pub fn fmt_event(event: &AgentEvent) -> String {
    match event {
        AgentEvent::Start | AgentEvent::ToolCallEnd { .. } => String::new(),
        AgentEvent::TextDelta { delta } => delta.clone(),
        AgentEvent::ThinkingDelta { delta } => {
            // Streamed live, dim+italic like pi's rendered thinking blocks.
            format!("{THINKING_STYLE}{}\x1b[0m", delta.clone())
        }
        AgentEvent::ToolCallStart { name, .. } => {
            format!("\n\x1b[2m· {name}\x1b[0m\n")
        }
        AgentEvent::ToolResult {
            output, is_error, ..
        } => {
            if *is_error {
                let truncated = truncate_chars(output, MAX_ERROR_DISPLAY_CHARS);
                format!("\x1b[31m✗ {truncated}\x1b[0m\n")
            } else {
                String::new()
            }
        }
        AgentEvent::TurnEnd { usage, .. } => {
            if usage.total() > 0 {
                let reasoning = if usage.reasoning_tokens > 0 {
                    format!(" · {} think", fmt_usage(usage.reasoning_tokens))
                } else {
                    String::new()
                };
                let cached = if usage.cached_tokens > 0 {
                    format!(
                        " · {} cached ({:.0}%)",
                        fmt_usage(usage.cached_tokens),
                        usage.cache_hit_rate() * 100.0
                    )
                } else {
                    String::new()
                };
                format!(
                    "\n\x1b[2m\u{b7} {} tok{reasoning}{cached}\x1b[0m\n",
                    fmt_usage(usage.total())
                )
            } else {
                "\n".to_string()
            }
        }
    }
}

/// Parsed command or input from the REPL prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplCommand {
    /// Exit the REPL cleanly (`/quit` or `/exit`).
    Quit,
    /// Open the system-prompt file in `$EDITOR` (`/sys`), print it (`/sys show`),
    /// or restore the default (`/sys reset`).
    Sys(SysAction),
    /// Open the provider selection menu (`/provider`).
    Provider,
    /// Add/update an API key in the CLI (`/key [provider]`), opencode-style.
    Key(Option<String>),
    /// Start a fresh conversation (`/new`).
    New,
    /// Compress conversation context window (`/compact` or `/compress [instructions]`).
    Compact(Option<String>),
    /// Toggle hiding thinking blocks (`/thinking`).
    Thinking,
    /// Print the command list (`/help`).
    Help,
    /// Open the model picker (`/model`) or set directly (`/model provider/id`).
    Model(Option<String>),
    /// Unknown slash command (`/word`).
    Unknown(String),
    /// Regular user prompt to feed to the agent.
    Prompt(String),
    /// Blank line, should be ignored.
    Empty,
}

/// What to do when the user types `/sys`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SysAction {
    /// Edit `~/.gray/sys.md` in `$EDITOR`.
    Edit,
    /// Print the current prompt file contents and path.
    Show,
    /// Overwrite the file with the shipped default.
    Reset,
}

/// Parses a line of input into a [`ReplCommand`].
pub fn parse_command(line: &str) -> ReplCommand {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        ReplCommand::Empty
    } else if trimmed == "/quit" || trimmed == "/exit" {
        ReplCommand::Quit
    } else if trimmed == "/sys" {
        ReplCommand::Sys(SysAction::Edit)
    } else if trimmed == "/sys show" {
        ReplCommand::Sys(SysAction::Show)
    } else if trimmed == "/sys reset" {
        ReplCommand::Sys(SysAction::Reset)
    } else if trimmed == "/new" {
        ReplCommand::New
    } else if trimmed == "/compact" || trimmed == "/compress" {
        ReplCommand::Compact(None)
    } else if let Some(rest) = trimmed.strip_prefix("/compact ") {
        let arg = rest.trim();
        ReplCommand::Compact((!arg.is_empty()).then(|| arg.to_string()))
    } else if let Some(rest) = trimmed.strip_prefix("/compress ") {
        let arg = rest.trim();
        ReplCommand::Compact((!arg.is_empty()).then(|| arg.to_string()))
    } else if trimmed == "/thinking" {
        ReplCommand::Thinking
    } else if trimmed == "/connect" || trimmed == "/provider" || trimmed == "/providers" || trimmed == "/login" {
        ReplCommand::Provider
    } else if trimmed == "/key" || trimmed == "/keys" {
        ReplCommand::Key(None)
    } else if let Some(rest) = trimmed
        .strip_prefix("/key ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        ReplCommand::Key(Some(rest.to_string()))
    } else if trimmed == "/help" {
        ReplCommand::Help
    } else if let Some(rest) = trimmed.strip_prefix("/model") {
        let arg = rest.trim();
        ReplCommand::Model((!arg.is_empty()).then(|| arg.to_string()))
    } else if trimmed.starts_with('/') {
        ReplCommand::Unknown(trimmed.to_string())
    } else {
        ReplCommand::Prompt(trimmed.to_string())
    }
}

struct SessionState {
    store: JsonlSessionStore,
    session_id: SessionId,
}

/// Picks the most recently started session summary (pure, unit-tested).
fn latest_summary(summaries: &[SessionSummary]) -> Option<&SessionSummary> {
    summaries.iter().max_by_key(|s| s.started_at)
}

/// Handles the `/sys` command family: edit, show, reset.
async fn handle_sys(config: &Config, cwd: &Path, action: SysAction, agent: &mut Option<Agent>) {
    let path = match crate::sys_prompt_path() {
        Ok(p) => p,
        Err(e) => {
            println!("{e}");
            return;
        }
    };
    match action {
        SysAction::Show => {
            match load_or_create_system_prompt_at(&path) {
                Ok(body) => {
                    println!("system prompt: {}", path.display());
                    println!("---");
                    println!("{body}");
                    println!("---");
                }
                Err(e) => println!("failed to read {}: {e}", path.display()),
            }
        }
        SysAction::Reset => {
            if let Err(e) = std::fs::write(&path, DEFAULT_SYS_PROMPT) {
                println!("failed to reset {}: {e}", path.display());
                return;
            }
            println!("system prompt restored to default ({})", path.display());
            reload_agent(agent, config, cwd).await;
        }
        SysAction::Edit => {
            // Make sure the file exists before opening an editor on it.
            let initial = match load_or_create_system_prompt_at(&path) {
                Ok(b) => b,
                Err(e) => {
                    println!("{e}");
                    return;
                }
            };
            let mut editor = crate::sys_editor::SysEditor::new(&initial, &path);
            match editor.run() {
                Ok(Some(saved)) => {
                    if let Err(e) = std::fs::write(&path, &saved) {
                        println!("failed to save {}: {e}", path.display());
                        return;
                    }
                    println!("system prompt saved — applies from your next message");
                    reload_agent(agent, config, cwd).await;
                }
                Ok(None) => {
                    println!("prompt unchanged");
                }
                Err(e) => {
                    println!("editor error: {e}");
                }
            }
        }
    }
}

/// Rebuilds the agent after a system-prompt change, preserving conversation history.
async fn reload_agent(agent: &mut Option<Agent>, config: &Config, cwd: &Path) {
    let old = agent.take();
    let mut rebuilt = match build_agent(config, cwd) {
        Ok(a) => a,
        Err(e) => {
            println!("{e}");
            *agent = old;
            return;
        }
    };
    if let Some(old) = old {
        rebuilt = rebuilt.with_messages(old.messages().to_vec());
    }
    *agent = Some(rebuilt);
}

/// Handles `/model`: interactive picker (no arg) or direct set (`/model provider/id`).
/// Switching updates the live agent and persists to ~/.gray/config.json.
async fn handle_model(
    config: &mut Config,
    cwd: &Path,
    direct: Option<String>,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
) {
    if let Some(m) = direct {
        config.model = Some(m.clone());
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            saved.model = Some(m.clone());
            let _ = crate::setup::save_saved_config_at(&path, &saved);
        }
        if let Some(shared) = tui {
            let mut t = shared.lock().expect("tui lock");
            t.set_model(m.clone());
            t.push_dim(format!("╰ model set to {m}"));
        } else {
            println!("model set to {m}");
        }
        reload_agent(agent, config, cwd).await;
        return;
    }

    match crate::setup::run_model_menu(config).await {
        Ok(true) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                if let Some(m) = &config.model {
                    t.set_model(m.clone());
                    t.push_dim(format!("╰ model set to {m}"));
                }
            }
            reload_agent(agent, config, cwd).await;
        }
        Ok(false) => {}
        Err(e) => {
            if let Some(shared) = tui {
                shared.lock().expect("tui lock").push_dim(format!("╰ error: {e}"));
            } else {
                println!("model error: {e}");
            }
        }
    }
}

/// Handles the `/compact` / `/compress` command family.
async fn handle_compact(
    config: &Config,
    cwd: &Path,
    custom_instructions: Option<String>,
    agent: &mut Option<Agent>,
    session_state: &mut Option<SessionState>,
    tui: Option<&crate::composer::SharedTui>,
) {
    if agent.is_none() {
        reload_agent(agent, config, cwd).await;
    }
    let Some(ag) = agent.as_mut() else {
        if let Some(shared) = tui {
            shared.lock().expect("tui lock").push_dim("╰ error: agent could not be initialized".to_string());
        } else {
            println!("error: agent could not be initialized");
        }
        return;
    };

    let messages = ag.messages().to_vec();
    if messages.is_empty() {
        if let Some(shared) = tui {
            shared.lock().expect("tui lock").push_dim("╰ nothing to compact (conversation is empty)".to_string());
        } else {
            println!("nothing to compact (conversation is empty)");
        }
        return;
    }

    if let Some(shared) = tui {
        shared.lock().expect("tui lock").set_status(Some("Compacting conversation context"));
    }

    let transcript = crate::compact::serialize_conversation(&messages);
    let prompt = crate::compact::build_summarization_prompt(&transcript, custom_instructions.as_deref());

    let summary_res = ag.complete_prompt(&prompt, Some(crate::compact::SUMMARIZATION_SYSTEM_PROMPT)).await;

    if let Some(shared) = tui {
        shared.lock().expect("tui lock").set_status(None);
    }

    match summary_res {
        Ok(summary) => {
            let summary_trimmed = summary.trim().to_string();
            let msg_count = messages.len();

            let summary_user = Message::user(format!(
                "<conversation_summary>\n{}\n</conversation_summary>\n\nPlease continue assisting based on the summary above.",
                summary_trimmed
            ));
            let summary_asst = Message::assistant(
                "Understood. I have reviewed the conversation summary and context, and I am ready to continue."
            );

            let new_messages = vec![summary_user.clone(), summary_asst.clone()];
            ag.set_messages(new_messages);

            // Record to session storage if active
            if let Some(state) = session_state {
                let _ = state.store.append(&state.session_id, &summary_user).await;
                let _ = state.store.append(&state.session_id, &summary_asst).await;
            }

            if let Some(shared) = tui {
                shared.lock().expect("tui lock").push_dim(format!(
                    "╰ compressed context ({} turns -> structured summary)",
                    msg_count
                ));
            } else {
                println!("compressed context ({} turns -> structured summary)", msg_count);
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                shared.lock().expect("tui lock").push_dim(format!("╰ compaction failed: {e}"));
            } else {
                println!("compaction failed: {e}");
            }
        }
    }
}

/// pi-style exit hint: how to reopen this conversation later.
fn print_exit_hint(session_state: &Option<SessionState>) {
    if let Some(state) = session_state {
        println!(
            "\x1b[2mTo resume this session: gray --session {}\x1b[0m",
            state.session_id.as_str()
        );
        let _ = std::io::stdout().flush();
    }
}

/// Appends whatever messages reached memory this turn (success, cancel, or
/// error) to the session store, so the JSONL transcript never diverges from
/// in-memory history.
async fn persist_turn_messages(
    session_state: &mut Option<SessionState>,
    agent: &Agent,
    config: &Config,
    cwd: &Path,
    initial_count: usize,
) {
    if session_state.is_none()
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        let session_id = SessionId::generate();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let meta = SessionMeta::new(
            session_id.clone(),
            timestamp,
            cwd.to_path_buf(),
            config.model.clone().unwrap_or_else(|| "unset".into()),
        );
        store.create(meta).await;
        *session_state = Some(SessionState { store, session_id });
    }
    if let Some(state) = session_state
        && agent.messages().len() > initial_count
    {
        for msg in &agent.messages()[initial_count..] {
            if let Err(e) = state.store.append(&state.session_id, msg).await {
                log::warn!(target: "gray_session", "session append failed: {e}");
            }
        }
    }
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
    let interactive = std::io::stdin().is_terminal();
    use std::io::IsTerminal;

    // boot: no forced wizard. A dim hint appears when unconfigured,
    // and the provider picker fires the moment credentials are needed.
    tokio::spawn(spawn_ctrl_c_policy());

    let mut unconfigured = config.model.is_none();
    if unconfigured {
        let ready = crate::setup::run_onboarding(config).await?;
        if !ready {
            print!("\r\x1b[2mrunning without a provider — send a message to set one up (or /provider)\x1b[0m\r\n");
        }
        print!("\r\n");
    } else if !interactive {
        // In the interactive composer the banner is inserted into scrollback
        // below (a direct print here gets scrolled off by the viewport
        // anchoring in Tui::new).
        crate::tui::print_logo();
        print!("\r\n");
        print!(
            "\r\x1b[1mgray\x1b[0m\x1b[2m {} \u{b7} Run /help for commands\x1b[0m\r\n",
            env!("CARGO_PKG_VERSION")
        );
    }

    // The agent is built lazily so the REPL opens even with no model/key configured;
    // we surface a friendly hint on first use instead of refusing to start.
    let mut agent: Option<Agent> = None;
    let mut session_state: Option<SessionState> = None;

    // `--session <id>`: reopen that exact session.
    if let Some(id) = session_id
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        match store.load(&SessionId::new(id)).await {
            Ok((_, entries)) => {
                let history: Vec<Message> = entries.into_iter().map(|e| e.message).collect();
                match build_agent(config, &cwd) {
                    Ok(built) => {
                        let n = history.len();
                        agent = Some(built.with_messages(history));
                        session_state = Some(SessionState {
                            session_id: SessionId::new(id),
                            store,
                        });
                        println!("\x1b[2mresumed {n}-message session {id}\x1b[0m");
                    }
                    Err(e) => println!("could not resume (no provider): {e}"),
                }
            }
            Err(e) => println!("could not resume session {id}: {e}"),
        }
    }

    // `-c`: reopen the most recent session instead of starting blank.
    if resume_last
        && session_state.is_none()
        && let Some(root) = default_root()
    {
        let store = JsonlSessionStore::new(root);
        if let Some(latest) = latest_summary(&store.list().await) {
            match store.load(&latest.id).await {
                Ok((_, entries)) => {
                    let history: Vec<Message> =
                        entries.into_iter().map(|e| e.message).collect();
                    match build_agent(config, &cwd) {
                        Ok(built) => {
                            let n = history.len();
                            agent = Some(built.with_messages(history));
                            session_state = Some(SessionState {
                                session_id: latest.id.clone(),
                                store,
                            });
                            println!("\x1b[2mresumed {n}-message session {}\x1b[0m", latest.id.as_str());
                        }
                        Err(e) => println!("could not resume (no provider): {e}"),
                    }
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
        let shared = crate::composer::SharedTui(
            std::sync::Arc::new(std::sync::Mutex::new({
                let mut t = crate::composer::Tui::new().expect("composer init");
                if let Some(m) = &config.model {
                    t.set_model(m.clone());
                }
                t.set_cwd(cwd.display().to_string());
                t
            })),
        );
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
                    // acquire the lock (blocked on read_line). Don't
                    // repaint the viewport after the main thread has
                    // already started shutdown.
                    if ticker_stop.load(AtomicOrdering::Relaxed) {
                        break;
                    }
                    t.tick_status();
                }
            }
        });
        (shared, stop)
    });

    // pi's hideThinkingBlock — toggled with /thinking, session-only.
    // Default hidden (codex-style) — prevents reasoning spill into transcript (see screenshot).
    let mut hide_thinking = true;

    loop {
        let line = if interactive {
            let (shared, stop) = tui.as_ref().expect("interactive implies tui");
            let line = {
                let mut t = shared.lock().expect("tui lock");
                let l = match t.read_line()? {
                    Some(l) => l,
                    None => {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                        t.shutdown();
                        print_exit_hint(&session_state);
                        break;
                    }
                };
                // Quit will shut down immediately after this block;
                // set the stop flag now while we still hold the lock so
                // the ticker's try_lock gap can't slip a draw in between.
                if matches!(parse_command(&l), ReplCommand::Quit) {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                l
            };
            line
        } else {
            print!("\u{203a} ");
            std::io::stdout().flush()?;
            let mut buf = String::new();
            if std::io::stdin().read_line(&mut buf)? == 0 {
                break;
            }
            buf.trim().to_string()
        };

        match parse_command(&line) {
            ReplCommand::Empty => continue,
            ReplCommand::Quit => {
                if let Some((shared, stop)) = &tui {
                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    let mut t = shared.lock().expect("tui lock");
                    t.shutdown();
                    print_exit_hint(&session_state);
                } else {
                    print_exit_hint(&session_state);
                }
                break;
            }
            ReplCommand::Sys(action) => {
                handle_sys(config, &cwd, action, &mut agent).await;
                continue;
            }
            ReplCommand::Model(direct) => {
                handle_model(config, &cwd, direct, &mut agent, tui.as_ref().map(|(s, _)| s)).await;
                continue;
            }
            ReplCommand::Help => {
                println!("{}", crate::rule("commands"));
                for (name, desc) in COMMANDS {
                    println!("  /{name:<8} {desc}");
                }
                continue;
            }
            ReplCommand::New => {
                agent = None;
                session_state = None;
                if let Some(root) = default_root() {
                    let store = JsonlSessionStore::new(root);
                    let session_id = SessionId::generate();
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let meta = SessionMeta::new(
                        session_id.clone(),
                        timestamp,
                        cwd.to_path_buf(),
                        config.model.clone().unwrap_or_else(|| "unset".into()),
                    );
                    store.create(meta).await;
                    let short_id = session_id.as_str().split('-').next().unwrap_or("new").to_string();
                    session_state = Some(SessionState { store, session_id });
                    reload_agent(&mut agent, config, &cwd).await;
                    if let Some((shared, _)) = &tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.push_dim(format!("╰ started fresh conversation ({short_id})"));
                    } else {
                        println!("started fresh conversation ({short_id})");
                    }
                } else {
                    reload_agent(&mut agent, config, &cwd).await;
                    if let Some((shared, _)) = &tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.push_dim("╰ started fresh conversation".to_string());
                    } else {
                        println!("started fresh conversation");
                    }
                }
                continue;
            }
            ReplCommand::Compact(instructions) => {
                handle_compact(
                    config,
                    &cwd,
                    instructions,
                    &mut agent,
                    &mut session_state,
                    tui.as_ref().map(|(s, _)| s),
                ).await;
                continue;
            }
            ReplCommand::Thinking => {
                hide_thinking = !hide_thinking;
                let (msg, effort) = if hide_thinking {
                    ("thinking hidden — /thinking to show it again", "off")
                } else {
                    ("thinking shown", "high")
                };
                if let Some((shared, _)) = &tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.set_hide_thinking(hide_thinking);
                    t.set_thinking_effort(effort.to_string());
                    t.push_dim(format!("╰ {msg}"));
                } else {
                    println!("{msg}");
                }
                continue;
            }
            ReplCommand::Provider => {
                match crate::setup::run_provider_menu(config).await {
                    Ok(true) => {
                        unconfigured = false;
                        if let Some((shared, _)) = &tui {
                            let mut t = shared.lock().expect("tui lock");
                            if let Some(m) = &config.model {
                                t.set_model(m.clone());
                            }
                            let model_str = config.model.as_deref().unwrap_or("default");
                            let catalog = crate::setup::load_catalog().ok();
                            let prov_name = catalog
                                .as_ref()
                                .and_then(|c| c.values().find(|p| p.base_url == config.base_url))
                                .map(|p| p.name.as_str())
                                .unwrap_or("provider");
                            t.push_dim(format!("╰ connected to {prov_name} · {model_str}"));
                        }
                        reload_agent(&mut agent, config, &cwd).await;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        if let Some((shared, _)) = &tui {
                            shared.lock().expect("tui lock").push_dim(format!("╰ error: {e}"));
                        } else {
                            println!("provider error: {e}");
                        }
                    }
                }
                continue;
            }
            ReplCommand::Key(pid) => {
                match crate::setup::run_key_setup(config, pid) {
                    Ok(true) => {
                        unconfigured = false;
                        reload_agent(&mut agent, config, &cwd).await;
                    }
                    Ok(false) => {}
                    Err(e) => println!("key error: {e}"),
                }
                continue;
            }
            ReplCommand::Unknown(_) => {
                println!("unknown command");
                continue;
            }
            ReplCommand::Prompt(prompt_text) => {
                if agent.is_none() {
                    if unconfigured {
                        match crate::setup::run_provider_menu(config).await {
                            Ok(true) => {
                                unconfigured = false;
                                if let Some((shared, _)) = &tui {
                                    if let Some(m) = &config.model {
                                        shared.lock().expect("tui lock").set_model(m.clone());
                                    }
                                }
                                print!("\r\n");
                            }
                            Ok(false) => {
                                continue;
                            }
                            Err(e) => {
                                println!("provider error: {e}");
                                continue;
                            }
                        }
                    }
                    match build_agent(config, &cwd) {
                        Ok(built) => agent = Some(built),
                        Err(e) => {
                            println!("{e}");
                            continue;
                        }
                    }
                }
                let agent = agent.as_mut().expect("agent built above");
                let cancel = tokio_util::sync::CancellationToken::new();
                *TURN_STATE.lock().expect("turn state lock") = Some(cancel.clone());
                let ctx = ToolContext {
                    cwd: cwd.clone(),
                    cancel: cancel.clone(),
                };
                let user_msg = Message::user(&prompt_text);
                let initial_count = agent.messages().len();

                let (shared, _) = if interactive {
                    (Some(tui.as_ref().expect("interactive implies tui")), ())
                } else { (None, ()) };

                // status row on; events stream straight into the composer
                let tui_stream = shared.as_ref().map(|(s, _)| (*s).clone());
                if let Some(s) = &tui_stream {
                    s.lock().expect("tui lock").begin_turn("Working");
                }

                // Raw mode swallows ^C (no SIGINT), so a watcher must read
                // key events during the turn and translate Ctrl-C into cancel.
                let watch_cancel = cancel.clone();
                let watch_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let watcher_stopped = watch_stop.clone();
                let _key_watcher = tokio::task::spawn_blocking(move || {
                    use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
                    loop {
                        // abort() can't kill a blocking task — poll the flag so
                        // the thread actually exits when the turn ends.
                        if watcher_stopped.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        match poll(std::time::Duration::from_millis(100)) {
                            Ok(true) => {}
                            _ => continue,
                        }
                        if let Ok(Event::Key(KeyEvent {
                            code: KeyCode::Char('c'),
                            modifiers,
                            kind: KeyEventKind::Press,
                            ..
                        })) = read()
                            && modifiers.contains(KeyModifiers::CONTROL)
                        {
                            watch_cancel.cancel();
                            return;
                        }
                    }
                });

                let run_result = {
                    let mut on_event = |ev: &AgentEvent| {
                        if let Some(shared) = &tui_stream
                            && let Ok(mut t) = shared.lock()
                        {
                            // Thinking/text get composer-level styling (ANSI
                            // would be stripped by stream()); any other event
                            // closes the thinking run first.
                            match ev {
                                AgentEvent::ThinkingDelta { delta } => t.stream_thinking(delta),
                                AgentEvent::TextDelta { delta } => t.stream_text(delta),
                                other => {
                                    t.end_thinking();
                                    t.stream(&crate::repl::fmt_event(other));
                                }
                            }
                        }
                    };
                    let mut run_future =
                        Box::pin(agent.run_streaming(user_msg, ctx, &mut on_event));
                    tokio::select! {
                        res = &mut run_future => res,
                        _ = cancel.cancelled() => Err(CoreError::Cancelled),
                    }
                };
                TURN_STATE.lock().expect("turn state lock").take();
                // signal the watcher to exit; it dies within one 100ms tick.
                // (Never .await it here without the flag — deadlock.)
                watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Some(s) = &tui_stream {
                    s.lock().expect("tui lock").end_turn();
                }

                match run_result {
                    Ok(_) => {
                        persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count).await;
                    }
                    Err(CoreError::Cancelled) => {
                        persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count).await;
                        if interactive {
                            if let Some((shared, _)) = &tui {
                                let mut t = shared.lock().expect("tui lock");
                                t.end_thinking();
                                t.stream("(interrupted)\n");
                            }
                        } else {
                            println!("(interrupted)");
                        }
                    }
                    Err(e) => {
                        persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count).await;
                        if interactive {
                            if let Some((shared, _)) = &tui {
                                let mut t = shared.lock().expect("tui lock");
                                t.end_thinking();
                                t.stream(&format!("agent error: {e}\n"));
                            }
                        } else {
                            eprintln!("agent error: {e}");
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::event::{StopReason, Usage};
    use serde_json::json;

    #[test]
    fn latest_summary_picks_max_started_at() {
        use gray_session::{SessionId, SessionSummary};
        let mk = |id: &str, t: u64| SessionSummary {
            id: SessionId::new(id),
            started_at: t,
            cwd: std::path::PathBuf::from("/tmp"),
            first_user_text: None,
        };
        let v = vec![mk("a", 5), mk("b", 99), mk("c", 1)];
        assert_eq!(latest_summary(&v).unwrap().id.as_str(), "b");
        assert!(latest_summary(&[]).is_none());
    }

    #[test]
    fn parse_command_identifies_quit_and_exit() {
        assert_eq!(parse_command("/quit"), ReplCommand::Quit);
        assert_eq!(parse_command("/exit"), ReplCommand::Quit);
        assert_eq!(parse_command("  /quit  "), ReplCommand::Quit);
        assert_eq!(parse_command("/exit\n"), ReplCommand::Quit);
        assert_eq!(parse_command("  /exit\r\n"), ReplCommand::Quit);
    }

    #[test]
    fn parse_command_identifies_empty_lines() {
        assert_eq!(parse_command(""), ReplCommand::Empty);
        assert_eq!(parse_command("   "), ReplCommand::Empty);
        assert_eq!(parse_command("\t\n"), ReplCommand::Empty);
    }

    #[test]
    fn parse_command_identifies_key() {
        assert_eq!(parse_command("/key"), ReplCommand::Key(None));
        assert_eq!(parse_command("/keys"), ReplCommand::Key(None));
        assert_eq!(
            parse_command("/key openrouter"),
            ReplCommand::Key(Some("openrouter".into()))
        );
        assert_eq!(parse_command("/key  deepseek "), ReplCommand::Key(Some("deepseek".into())));
        // near-misses stay unknown
        assert_eq!(parse_command("/keyboard"), ReplCommand::Unknown("/keyboard".into()));
    }

    #[test]
    fn parse_command_identifies_provider() {
        assert_eq!(parse_command("/connect"), ReplCommand::Provider);
        assert_eq!(parse_command("/provider"), ReplCommand::Provider);
        assert_eq!(parse_command("/providers"), ReplCommand::Provider);
        assert_eq!(parse_command("/login"), ReplCommand::Provider);
        assert_eq!(parse_command("/help"), ReplCommand::Help);
        assert_eq!(
            parse_command("/model openai/gpt-4o"),
            ReplCommand::Model(Some("openai/gpt-4o".into()))
        );
        assert_eq!(parse_command("/model"), ReplCommand::Model(None));
        assert_eq!(parse_command("  /provider  "), ReplCommand::Provider);
        assert_eq!(parse_command("/provider\n"), ReplCommand::Provider);
        // near-misses stay unknown
        assert_eq!(
            parse_command("/provider extra"),
            ReplCommand::Unknown("/provider extra".to_string())
        );
    }

    #[test]
    fn parse_command_identifies_unknown_slash_commands() {
        assert_eq!(
            parse_command("/foo"),
            ReplCommand::Unknown("/foo".to_string())
        );
        assert_eq!(
            parse_command("  /custom_cmd  "),
            ReplCommand::Unknown("/custom_cmd".to_string())
        );
        assert_eq!(parse_command("/"), ReplCommand::Unknown("/".to_string()));
    }

    #[test]
    fn parse_command_identifies_new_and_compact() {
        assert_eq!(parse_command("/new"), ReplCommand::New);
        assert_eq!(parse_command("  /new  "), ReplCommand::New);
        assert_eq!(parse_command("/compact"), ReplCommand::Compact(None));
        assert_eq!(parse_command("/compress"), ReplCommand::Compact(None));
        assert_eq!(
            parse_command("/compact focus on auth"),
            ReplCommand::Compact(Some("focus on auth".into()))
        );
        assert_eq!(
            parse_command("/compress remember the db port"),
            ReplCommand::Compact(Some("remember the db port".into()))
        );
    }

    #[test]
    fn parse_command_identifies_thinking_toggle() {
        assert_eq!(parse_command("/thinking"), ReplCommand::Thinking);
        assert_eq!(parse_command("  /thinking  "), ReplCommand::Thinking);
        // near-misses stay unknown
        assert_eq!(
            parse_command("/thinking off"),
            ReplCommand::Unknown("/thinking off".to_string())
        );
    }

    #[test]
    fn parse_command_identifies_prompts() {
        assert_eq!(
            parse_command("hello world"),
            ReplCommand::Prompt("hello world".to_string())
        );
        assert_eq!(
            parse_command("  calculate 1 + 1  "),
            ReplCommand::Prompt("calculate 1 + 1".to_string())
        );
        assert_eq!(
            parse_command("write a test"),
            ReplCommand::Prompt("write a test".to_string())
        );
    }

    #[test]
    fn fmt_usage_formats_under_and_over_1000() {
        assert_eq!(fmt_usage(0), "0");
        assert_eq!(fmt_usage(500), "500");
        assert_eq!(fmt_usage(999), "999");
        assert_eq!(fmt_usage(1000), "1.0k");
        assert_eq!(fmt_usage(1200), "1.2k");
        assert_eq!(fmt_usage(1250), "1.2k");
        assert_eq!(fmt_usage(10500), "10.5k");
    }

    #[test]
    fn fmt_event_renders_start_and_tool_call_end_empty() {
        assert_eq!(fmt_event(&AgentEvent::Start), "");
        assert_eq!(
            fmt_event(&AgentEvent::tool_call_end("call_1", json!({"path": "foo.txt"}))),
            ""
        );
    }

    #[test]
    fn fmt_event_renders_text_delta() {
        assert_eq!(fmt_event(&AgentEvent::text_delta("Hello, ")), "Hello, ");
        assert_eq!(fmt_event(&AgentEvent::text_delta("world!")), "world!");
    }

    #[test]
    fn fmt_event_renders_tool_call_start_chip() {
        assert_eq!(
            fmt_event(&AgentEvent::tool_call_start("call_1", "read")),
            "\n\x1b[2m· read\x1b[0m\n"
        );
        assert_eq!(
            fmt_event(&AgentEvent::tool_call_start("call_2", "bash")),
            "\n\x1b[2m· bash\x1b[0m\n"
        );
    }

    #[test]
    fn fmt_event_renders_tool_result_non_error_empty() {
        assert_eq!(
            fmt_event(&AgentEvent::tool_result("call_1", "success output", false)),
            ""
        );
    }

    #[test]
    fn fmt_event_renders_tool_result_error() {
        assert_eq!(
            fmt_event(&AgentEvent::tool_result("call_1", "file not found", true)),
            "\x1b[31m✗ file not found\x1b[0m\n"
        );
    }

    #[test]
    fn fmt_event_renders_tool_result_error_truncation_over_200_chars() {
        let long_error = "a".repeat(250);
        let rendered = fmt_event(&AgentEvent::tool_result("call_1", &long_error, true));
        let expected = format!("\x1b[31m✗ {}\x1b[0m\n", "a".repeat(200));
        assert_eq!(rendered, expected);
    }

    #[test]
    fn fmt_event_renders_tool_result_error_exact_200_chars() {
        let exact_error = "b".repeat(200);
        let rendered = fmt_event(&AgentEvent::tool_result("call_1", &exact_error, true));
        let expected = format!("\x1b[31m✗ {}\x1b[0m\n", "b".repeat(200));
        assert_eq!(rendered, expected);
    }

    #[test]
    fn fmt_event_renders_turn_end_zero_usage() {
        assert_eq!(
            fmt_event(&AgentEvent::turn_end(StopReason::EndTurn, Usage::new(0, 0))),
            "\n"
        );
    }

    #[test]
    fn fmt_event_renders_turn_end_with_usage() {
        assert_eq!(
            fmt_event(&AgentEvent::turn_end(StopReason::EndTurn, Usage::new(100, 200))),
            "\n\x1b[2m· 300 tok\x1b[0m\n"
        );
        assert_eq!(
            fmt_event(&AgentEvent::turn_end(StopReason::EndTurn, Usage::new(400, 800))),
            "\n\x1b[2m· 1.2k tok\x1b[0m\n"
        );
    }

    #[test]
    fn fmt_event_thinking_delta_is_dim_italic() {
        assert_eq!(
            fmt_event(&AgentEvent::thinking_delta("pondering")),
            "\x1b[2m\x1b[3mpondering\x1b[0m"
        );
    }

    #[test]
    fn fmt_event_turn_end_shows_reasoning_tokens_when_present() {
        let usage = Usage { input_tokens: 100, output_tokens: 200, reasoning_tokens: 64, ..Default::default() };
        assert!(fmt_event(&AgentEvent::turn_end(StopReason::EndTurn, usage)).contains("64 think"));
        // Zero reasoning tokens must not add a "think" segment.
        let plain = fmt_event(&AgentEvent::turn_end(
            StopReason::EndTurn,
            Usage { input_tokens: 1, output_tokens: 2, reasoning_tokens: 0, ..Default::default() },
        ));
        assert!(!plain.contains("think"));
    }
}
