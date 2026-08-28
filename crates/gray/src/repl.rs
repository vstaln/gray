//! Interactive REPL mode for Gray.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gray_core::agent::{Agent, ToolContext};
use gray_core::error::CoreError;
use gray_core::event::AgentEvent;
use gray_core::message::Message;
use gray_session::{
    default_root, JsonlSessionStore, SessionId, SessionMeta, SessionStore,
};

use std::sync::Mutex as StdMutex;


/// Static slash-command table driving both `/help` and the autocomplete panel.
pub(crate) const COMMANDS: &[(&str, &str)] = &[
    ("connect", "connect a provider & setup API key"),
    ("model", "switch or pick a model"),
    ("thinking", "set reasoning effort (off, minimal, low, medium, high, xhigh, max) — /effort is alias"),
    ("resume", "resume a previous conversation"),
    ("new", "start a fresh conversation"),
    ("compact", "compress conversation context into a structured summary"),
    ("sys", "view, edit, or restore the system prompt"),
    ("help", "print the command list"),
    ("quit", "exit (Ctrl-C exits at the prompt, cancels mid-turn)"),
];

const ALIASES: &[(&str, &str)] = &[
    ("clear", "new"),
    ("reset", "new"),
    ("exit", "quit"),
    ("keys", "connect"),
    ("key", "connect"),
    ("providers", "connect"),
    ("provider", "connect"),
    ("login", "connect"),
    ("effort", "thinking"),
    ("compress", "compact"),
];

/// Commands matching `filter` (the text after '/'), auto-sorted by relevance.
pub(crate) fn completion_matches(filter: &str) -> Vec<(&'static str, &'static str)> {
    let f = filter.to_lowercase();
    let mut matches: Vec<(&'static str, &'static str)> = Vec::new();

    for &(name, desc) in COMMANDS {
        let is_match = f.is_empty()
            || name.to_lowercase().contains(&f)
            || desc.to_lowercase().contains(&f)
            || ALIASES.iter().any(|(alias, target)| *target == name && alias.contains(&f));

        if is_match {
            matches.push((name, desc));
        }
    }

    matches.sort_by_key(|(n, _)| {
        let nl = n.to_lowercase();
        if nl == f {
            0
        } else if nl.starts_with(&f) {
            1
        } else if ALIASES.iter().any(|(alias, target)| *target == *n && *alias == f) {
            2
        } else if ALIASES.iter().any(|(alias, target)| *target == *n && alias.starts_with(&f)) {
            3
        } else {
            4
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

/// Formats a token count as exact total number.
pub fn fmt_usage(total: usize) -> String {
    format!("{total}")
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn media_type_for_path(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png".to_string(),
        Some("jpg") | Some("jpeg") => "image/jpeg".to_string(),
        Some("webp") => "image/webp".to_string(),
        Some("gif") => "image/gif".to_string(),
        Some("bmp") => "image/bmp".to_string(),
        _ => "image/png".to_string(),
    }
}

fn build_user_message_with_images(text: &str, image_paths: &[std::path::PathBuf]) -> Message {
    if image_paths.is_empty() {
        return Message::user(text);
    }
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(gray_core::message::ContentBlock::text(text.to_string()));
    }
    for path in image_paths {
        if let Ok(bytes) = std::fs::read(path) {
            let media_type = media_type_for_path(path);
            let data = base64_encode(&bytes);
            blocks.push(gray_core::message::ContentBlock::image(media_type, data));
        }
    }
    if blocks.is_empty() {
        // image read failed, fallback to text placeholder
        return Message::user(text);
    }
    Message::new(gray_core::message::Role::User, blocks)
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
                    "\n\x1b[2m\u{b7} {} tok{cached}\x1b[0m\n",
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
    /// Open the provider selection menu (`/connect` or `/provider`).
    Provider,
    /// Start a fresh conversation (`/new` or `/clear [prompt]`).
    New(Option<String>),
    /// Resume a previous session (`/resume [id|--last|--all]`).
    Resume(ResumeArgs),
    /// Compress conversation context window (`/compact` or `/compress [instructions]`).
    Compact(Option<String>),
    /// Set reasoning effort (`/thinking [level]` or `/effort [level]`; bare toggles hide/show).
    Thinking(Option<String>),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeArgs {
    pub target: Option<String>,
    pub last: bool,
    pub all: bool,
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

fn parse_resume_args(rest: &str) -> ResumeArgs {
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let mut target: Option<String> = None;
    let mut last = false;
    let mut all = false;
    for tok in tokens {
        match tok {
            "--last" => last = true,
            "--all" => all = true,
            s if s.starts_with("--") => {}
            s => {
                if target.is_none() {
                    target = Some(s.to_string());
                }
            }
        }
    }
    ResumeArgs { target, last, all }
}

/// Parses a line of input into a [`ReplCommand`].
pub fn parse_command(line: &str) -> ReplCommand {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        ReplCommand::Empty
    } else if trimmed == "/quit" || trimmed == "/exit" {
        ReplCommand::Quit
    } else if trimmed == "/resume" {
        ReplCommand::Resume(ResumeArgs { target: None, last: false, all: false })
    } else if let Some(rest) = trimmed.strip_prefix("/resume ") {
        ReplCommand::Resume(parse_resume_args(rest))
    } else if trimmed == "/sys" {
        ReplCommand::Sys(SysAction::Edit)
    } else if trimmed == "/sys show" {
        ReplCommand::Sys(SysAction::Show)
    } else if trimmed == "/sys reset" {
        ReplCommand::Sys(SysAction::Reset)
    } else if trimmed == "/new" || trimmed == "/clear" || trimmed == "/reset" {
        ReplCommand::New(None)
    } else if let Some(rest) = trimmed.strip_prefix("/new ") {
        let arg = rest.trim();
        ReplCommand::New((!arg.is_empty()).then(|| arg.to_string()))
    } else if let Some(rest) = trimmed.strip_prefix("/clear ") {
        let arg = rest.trim();
        ReplCommand::New((!arg.is_empty()).then(|| arg.to_string()))
    } else if let Some(rest) = trimmed.strip_prefix("/reset ") {
        let arg = rest.trim();
        ReplCommand::New((!arg.is_empty()).then(|| arg.to_string()))
    } else if trimmed == "/compact" || trimmed == "/compress" {
        ReplCommand::Compact(None)
    } else if let Some(rest) = trimmed.strip_prefix("/compact ") {
        let arg = rest.trim();
        ReplCommand::Compact((!arg.is_empty()).then(|| arg.to_string()))
    } else if let Some(rest) = trimmed.strip_prefix("/compress ") {
        let arg = rest.trim();
        ReplCommand::Compact((!arg.is_empty()).then(|| arg.to_string()))
    } else if trimmed == "/thinking" || trimmed == "/effort" {
        ReplCommand::Thinking(None)
    } else if let Some(rest) = trimmed.strip_prefix("/thinking ") {
        let arg = rest.trim();
        ReplCommand::Thinking((!arg.is_empty()).then(|| arg.to_string()))
    } else if let Some(rest) = trimmed.strip_prefix("/effort ") {
        let arg = rest.trim();
        ReplCommand::Thinking((!arg.is_empty()).then(|| arg.to_string()))
    } else if trimmed == "/connect" || trimmed == "/provider" || trimmed == "/providers" || trimmed == "/login" || trimmed == "/key" || trimmed == "/keys" || trimmed.starts_with("/key ") {
        ReplCommand::Provider
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



/// Handles the `/sys` command family: edit, show, reset.
async fn handle_sys(config: &Config, cwd: &Path, action: SysAction, agent: &mut Option<Agent>, tui: Option<&crate::composer::SharedTui>) {
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
            let tui_snap = tui.cloned();
            let editor_paused = if let Some(shared) = &tui_snap {
                if let Ok(mut t) = shared.try_lock() {
                    t.pending_resize = Some((t.last_width, std::time::Instant::now() + std::time::Duration::from_secs(3600)));
                    true
                } else { false }
            } else { false };
            let mut editor = crate::sys_editor::SysEditor::new(&initial, &path);
            let res = editor.run();
            if editor_paused {
                if let Some(shared) = &tui_snap {
                    let mut t = shared.lock().expect("tui lock");
                    t.pending_resize = None;
                    if let Ok((cols, _)) = crossterm::terminal::size() {
                        t.last_width = cols;
                    }
                    let _ = t.terminal.clear();
                    let _ = t.draw();
                }
            }
            match res {
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
            t.push_action("Model set to", Some(&m));
        } else {
            println!("✓ Model set to {m}");
        }
        reload_agent(agent, config, cwd).await;
        return;
    }

    let bg = tui.map(|shared| shared.lock().expect("tui lock").snapshot());
    match crate::setup::run_model_menu(config, bg.as_ref()).await {
        Ok(true) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                if let Some(m) = &config.model {
                    t.set_model(m.clone());
                    t.push_action("Model set to", Some(m));
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

/// Handles `/thinking` / `/effort`: direct set (`/thinking high`), toggle visibility (bare `/thinking`), or picker.
async fn handle_thinking(
    config: &mut Config,
    cwd: &Path,
    direct: Option<String>,
    agent: &mut Option<Agent>,
    tui: Option<&crate::composer::SharedTui>,
    hide_thinking: &mut bool,
) {
    if let Some(eff) = direct {
        let eff_clean = eff.to_lowercase();
        if eff_clean == "off" || crate::setup::THINKING_LEVELS.iter().any(|(l, _)| *l == eff_clean) {
            config.thinking_effort = Some(eff_clean.clone());
            if let Ok(path) = crate::setup::saved_config_path() {
                let mut saved = crate::setup::load_saved_config_at(&path);
                saved.thinking_effort = Some(eff_clean.clone());
                let _ = crate::setup::save_saved_config_at(&path, &saved);
            }
            *hide_thinking = eff_clean == "off";
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                t.set_thinking_effort(eff_clean.clone());
                t.set_hide_thinking(*hide_thinking);
                t.push_action("Thinking effort set to", Some(&eff_clean));
            } else {
                println!("✓ Thinking effort set to {eff_clean}");
            }
            reload_agent(agent, config, cwd).await;
            return;
        }
        let msg = format!("unknown level '{eff_clean}' — try: off, minimal, low, medium, high, xhigh, max");
        if let Some(shared) = tui {
            shared.lock().expect("tui lock").push_dim(format!("╰ {msg}"));
        } else {
            println!("{msg}");
        }
        return;
    }

    let has_explicit_level = config.thinking_effort.is_some();
    let bg = tui.map(|shared| shared.lock().expect("tui lock").snapshot());
    match crate::setup::run_effort_menu(config, bg.as_ref()).await {
        Ok(true) => {
            if let Some(shared) = tui {
                let mut t = shared.lock().expect("tui lock");
                if let Some(eff) = &config.thinking_effort {
                    t.set_thinking_effort(eff.clone());
                    *hide_thinking = eff == "off";
                    t.set_hide_thinking(*hide_thinking);
                    t.push_action("Thinking effort set to", Some(eff));
                }
            }
            reload_agent(agent, config, cwd).await;
        }
        Ok(false) => {
            if !has_explicit_level {
                *hide_thinking = !*hide_thinking;
                let (msg, eff) = if *hide_thinking { ("thinking hidden — /thinking to show", "off") } else { ("thinking shown", "high") };
                if let Some(shared) = tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.set_hide_thinking(*hide_thinking);
                    t.set_thinking_effort(eff.to_string());
                    t.push_dim(format!("╰ {msg}"));
                } else {
                    println!("{msg}");
                }
            }
        }
        Err(e) => {
            if let Some(shared) = tui {
                shared.lock().expect("tui lock").push_dim(format!("╰ error: {e}"));
            } else {
                println!("effort error: {e}");
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

fn print_exit_hint(session_state: &Option<SessionState>) {
    if let Some(state) = session_state {
        println!("\x1b[2mTo resume: gray resume {}\x1b[0m", state.session_id.as_str());
        let _ = std::io::stdout().flush();
    }
}

async fn handle_resume(
    config: &Config,
    cwd: &Path,
    args: ResumeArgs,
    agent: &mut Option<Agent>,
    session_state: &mut Option<SessionState>,
    tui: Option<&crate::composer::SharedTui>,
) {
    let bg = tui.as_ref().map(|s| s.lock().expect("tui lock").snapshot());
    let target_id: Option<SessionId> = if let Some(raw) = args.target.as_deref() {
        if let Some(root) = default_root() {
            let store = JsonlSessionStore::new(root);
            if let Some(id) = crate::resume::resolve_prefix(&store, raw, args.all).await {
                Some(id)
            } else {
                let sid = SessionId::new(raw);
                match store.load(&sid).await {
                    Ok(_) => Some(sid),
                    Err(e) => {
                        let msg = format!("no session matching '{raw}': {e}");
                        if let Some(shared) = &tui {
                            shared.lock().expect("tui lock").push_dim(format!("╰ {msg}"));
                        } else {
                            println!("{msg}");
                        }
                        return;
                    }
                }
            }
        } else {
            None
        }
    } else if args.last {
        let Some(root) = default_root() else { return; };
        let store = JsonlSessionStore::new(root);
        let summaries = store.list().await;
        let cwd_now = std::env::current_dir().ok();
        let filt = if args.all { None } else { cwd_now.as_deref() };
        match crate::resume::latest_summary(&summaries, filt) {
            Some(s) => Some(s.id.clone()),
            None => {
                let msg = if args.all { "no saved sessions" } else { "no saved sessions in this directory (try /resume --all or --all)" };
                if let Some(shared) = &tui {
                    shared.lock().expect("tui lock").push_dim(format!("╰ {msg}"));
                } else {
                    println!("{msg}");
                }
                return;
            }
        }
    } else {
        match crate::resume::run_resume_picker(args.all, bg.as_ref()).await {
            Ok(Some(id)) => Some(id),
            Ok(None) => return,
            Err(e) => {
                if let Some(shared) = &tui {
                    shared.lock().expect("tui lock").push_dim(format!("╰ resume picker error: {e}"));
                } else {
                    println!("resume picker error: {e}");
                }
                return;
            }
        }
    };

    let Some(sid) = target_id else { return; };
    let Some(root) = default_root() else { return; };
    let store = JsonlSessionStore::new(root);
    match store.load(&sid).await {
        Ok((_meta, entries)) => {
            let history: Vec<Message> = entries.iter().map(|e| e.message.clone()).collect();
            let n = history.len();
            match build_agent(config, cwd) {
                Ok(built) => {
                    *agent = Some(built.with_messages(history));
                    *session_state = Some(SessionState { session_id: sid.clone(), store });
                    if let Some(shared) = &tui {
                        let mut t = shared.lock().expect("tui lock");
                        t.replay_session_history(&entries, cwd);
                        t.push_dim(format!("\u{2b22} Resumed session {} ({n} messages)", sid.as_str()));
                    } else {
                        println!("\x1b[2m\u{2b22} Resumed session {} ({n} messages)\x1b[0m", sid.as_str());
                    }
                }
                Err(e) => {
                    let msg = format!("could not resume (no provider): {e}");
                    if let Some(shared) = &tui {
                        shared.lock().expect("tui lock").push_dim(format!("╰ {msg}"));
                    } else {
                        println!("{msg}");
                    }
                }
            }
        }
        Err(e) => {
            let msg = format!("could not resume session {}: {e}", sid.as_str());
            if let Some(shared) = &tui {
                shared.lock().expect("tui lock").push_dim(format!("╰ {msg}"));
            } else {
                println!("{msg}");
            }
        }
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
    latest_usage: Option<gray_core::event::Usage>,
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
        let new_messages = &agent.messages()[initial_count..];
        for (i, msg) in new_messages.iter().enumerate() {
            let usage = if i == new_messages.len() - 1 {
                latest_usage
            } else {
                None
            };
            if let Err(e) = state.store.append_with_usage(&state.session_id, msg, usage).await {
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
                if let Ok(built) = build_agent(config, &cwd) {
                    agent = Some(built.with_messages(history));
                }
                session_state = Some(SessionState {
                    session_id: sid.clone(),
                    store,
                });
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
        if let Some(latest) = crate::resume::latest_summary(&summaries, cwd_now.as_deref()).or_else(|| crate::resume::latest_summary(&summaries, None)) {
            match store.load(&latest.id).await {
                Ok((meta, entries)) => {
                    if config.model.is_none() && !meta.model.is_empty() {
                        config.model = Some(meta.model.clone());
                    }
                    let history: Vec<Message> = entries.iter().map(|e| e.message.clone()).collect();
                    pending_history = history.clone();
                    if let Ok(built) = build_agent(config, &cwd) {
                        agent = Some(built.with_messages(history));
                    }
                    session_state = Some(SessionState {
                        session_id: latest.id.clone(),
                        store,
                    });
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
        let shared = crate::composer::SharedTui(
            std::sync::Arc::new(std::sync::Mutex::new({
                let mut t = crate::composer::Tui::new().expect("composer init");
                if let Some(m) = &config.model {
                    t.set_model(m.clone());
                }
                if let Some(eff) = &config.thinking_effort {
                    t.set_thinking_effort(eff.clone());
                }
                t.set_cwd(cwd.display().to_string());
                if let Some((ref sid, ref entries)) = resumed_session_info {
                    t.replay_session_history(entries, &cwd);
                    t.push_dim(format!("\u{2b22} Resumed session {} ({} messages)", sid.as_str(), entries.len()));
                }
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
    let mut pending_command: Option<ReplCommand> = None;
    let mut pending_images: Vec<std::path::PathBuf> = Vec::new();

    loop {
        let cmd = if let Some(c) = pending_command.take() {
            c
        } else {
            let (line_text, images) = if interactive {
                let (shared, stop) = tui.as_ref().expect("interactive implies tui");
                let (txt, imgs) = {
                    let mut t = shared.lock().expect("tui lock");
                    let pair = match t.read_line()? {
                        Some(v) => v,
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
                    if matches!(parse_command(&pair.0), ReplCommand::Quit) {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    pair
                };
                (txt, imgs)
            } else {
                print!("\u{203a} ");
                std::io::stdout().flush()?;
                let mut buf = String::new();
                if std::io::stdin().read_line(&mut buf)? == 0 {
                    break;
                }
                (buf.trim().to_string(), Vec::new())
            };
            pending_images = images;
            parse_command(&line_text)
        };
        // Clear pending images for non-prompt commands (keep for Prompt/Empty+images)
        if !matches!(&cmd, ReplCommand::Prompt(_) | ReplCommand::Empty) {
            pending_images.clear();
        }

        match cmd {
            ReplCommand::Empty => {
                if !pending_images.is_empty() {
                    // image(s) without text: treat as prompt with images
                    let images = std::mem::take(&mut pending_images);
                    let prompt_text = String::new();
                    // fall through to Prompt handling by constructing message immediately
                    // reuse Prompt logic inline
                    if agent.is_none() {
                        if unconfigured {
                            let bg = tui.as_ref().map(|(shared, _)| shared.lock().expect("tui lock").snapshot());
                            match crate::setup::run_provider_menu(config, bg.as_ref()).await {
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
                            Ok(built) => {
                                if !pending_history.is_empty() {
                                    agent = Some(built.with_messages(std::mem::take(&mut pending_history)));
                                } else {
                                    agent = Some(built);
                                }
                            }
                            Err(e) => {
                                println!("{e}");
                                continue;
                            }
                        }
                    }
                    let agent = agent.as_mut().expect("agent built above");
                    let cancel = tokio_util::sync::CancellationToken::new();
                    *TURN_STATE.lock().expect("turn state lock") = Some(cancel.clone());
                    let ctx = ToolContext { cwd: cwd.clone(), cancel: cancel.clone() };
                    let user_msg = build_user_message_with_images(&prompt_text, &images);
                    let initial_count = agent.messages().len();
                    let (shared, _) = if interactive { (Some(tui.as_ref().expect("interactive implies tui")), ()) } else { (None, ()) };
                    let tui_stream = shared.as_ref().map(|(s, _)| (*s).clone());
                    if let Some(s) = &tui_stream { s.lock().expect("tui lock").begin_turn("Working"); }
                    let ticker_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let ticker_stopped = ticker_stop.clone();
                    let ticker_tui = tui_stream.clone();
                    let _ticker_task = tokio::spawn(async move {
                        while !ticker_stopped.load(std::sync::atomic::Ordering::Relaxed) {
                            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                            if let Some(shared) = &ticker_tui { if let Ok(mut t) = shared.lock() { let _ = t.draw(); } }
                        }
                    });
                    let watch_cancel = cancel.clone();
                    let watch_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let watcher_stopped = watch_stop.clone();
                    let _key_watcher = tokio::task::spawn_blocking(move || {
                        use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
                        loop {
                            if watcher_stopped.load(std::sync::atomic::Ordering::Relaxed) { return; }
                            match poll(std::time::Duration::from_millis(50)) { Ok(true) => {} _ => continue, }
                            if let Ok(Event::Key(KeyEvent { code, modifiers, kind, .. })) = read() {
                                if kind == KeyEventKind::Release { continue; }
                                if code == KeyCode::Esc || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL)) { watch_cancel.cancel(); return; }
                            }
                        }
                    });
                    let mut current_tool_name: Option<String> = None;
                    let mut current_tool_args: Option<serde_json::Value> = None;
                    let mut turn_usage: Option<gray_core::event::Usage> = None;
                    let run_result = {
                        let mut on_event = |ev: &gray_core::event::AgentEvent| {
                            if let Some(shared) = &tui_stream && let Ok(mut t) = shared.lock() {
                                match ev {
                                    gray_core::event::AgentEvent::ThinkingDelta { delta } => t.stream_thinking(delta),
                                    gray_core::event::AgentEvent::TextDelta { delta } => t.stream_text(delta),
                                    gray_core::event::AgentEvent::ToolCallStart { name, .. } => { t.end_thinking(); current_tool_name = Some(name.clone()); current_tool_args = None; }
                                    gray_core::event::AgentEvent::ToolCallEnd { args, .. } => { t.end_thinking(); let name = current_tool_name.as_deref().unwrap_or("tool"); current_tool_args = Some(args.clone()); let header = crate::tool_fmt::format_tool_call_header(name, args, Some(&cwd)); t.push_line_spans(header); }
                                    gray_core::event::AgentEvent::ToolResult { output, is_error, .. } => { let name = current_tool_name.take().unwrap_or_default(); let args = current_tool_args.take(); let lines = crate::tool_fmt::format_tool_result_lines_with_context(&name, args.as_ref(), output, *is_error, Some(&cwd)); if !lines.is_empty() { t.push_styled_lines(lines); } }
                                    gray_core::event::AgentEvent::TurnEnd { usage, .. } => { turn_usage = Some(*usage); t.end_thinking(); t.set_usage(*usage); if usage.total() > 0 { t.push_usage(format!("\u{2b22} {} tok", crate::repl::fmt_usage(usage.total()))); } }
                                    _ => {}
                                }
                            } else if !interactive {
                                match ev {
                                    gray_core::event::AgentEvent::TextDelta { delta } => print!("{delta}"),
                                    gray_core::event::AgentEvent::ThinkingDelta { delta } => print!("{THINKING_STYLE}{delta}\x1b[0m"),
                                    gray_core::event::AgentEvent::ToolCallStart { name, .. } => { current_tool_name = Some(name.clone()); current_tool_args = None; }
                                    gray_core::event::AgentEvent::ToolCallEnd { args, .. } => { let name = current_tool_name.as_deref().unwrap_or("tool"); current_tool_args = Some(args.clone()); println!("\n{}", crate::tool_fmt::format_tool_call_header_plain(name, args, Some(&cwd))); }
                                    gray_core::event::AgentEvent::ToolResult { output, is_error, .. } => { let name = current_tool_name.take().unwrap_or_default(); let args = current_tool_args.take(); let res = crate::tool_fmt::format_tool_result_plain_with_context(&name, args.as_ref(), output, *is_error, Some(&cwd)); if !res.is_empty() { print!("{res}"); } }
                                    gray_core::event::AgentEvent::TurnEnd { usage, .. } => { turn_usage = Some(*usage); if usage.total() > 0 { println!("\n\x1b[2m\u{2b22} {} tok\x1b[0m", crate::repl::fmt_usage(usage.total())); } }
                                    _ => {}
                                }
                                let _ = std::io::stdout().flush();
                            }
                        };
                        let mut run_future = Box::pin(agent.run_streaming(user_msg, ctx, &mut on_event));
                        tokio::select! { res = &mut run_future => res, _ = cancel.cancelled() => Err(gray_core::error::CoreError::Cancelled), }
                    };
                    TURN_STATE.lock().expect("turn state lock").take();
                    watch_stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    ticker_stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    if let Some(s) = &tui_stream { s.lock().expect("tui lock").end_turn(); }
                    match run_result {
                        Ok(_) => { persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await; }
                        Err(gray_core::error::CoreError::Cancelled) => { persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await; if interactive { if let Some((shared, _)) = &tui { let mut t = shared.lock().expect("tui lock"); t.end_thinking(); t.stream("(interrupted)\n"); } } else { println!("(interrupted)"); } }
                        Err(e) => { persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await; if interactive { if let Some((shared, _)) = &tui { let mut t = shared.lock().expect("tui lock"); t.end_thinking(); t.stream(&format!("agent error: {e}\n")); } } else { eprintln!("agent error: {e}"); } }
                    }
                    continue;
                }
                pending_images.clear();
                continue;
            }
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
                handle_sys(config, &cwd, action, &mut agent, tui.as_ref().map(|(s, _)| s)).await;
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
            ReplCommand::Resume(args) => {
                handle_resume(config, &cwd, args, &mut agent, &mut session_state, tui.as_ref().map(|(s, _)| s)).await;
                continue;
            }
            ReplCommand::New(initial_prompt) => {
                pending_history.clear();
                agent = build_agent(config, &cwd).ok();
                session_state = None;
                let mut short_id = String::new();
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
                    short_id = session_id.as_str().split('-').next().unwrap_or("new").to_string();
                    session_state = Some(SessionState { store, session_id });
                }

                if let Some((shared, _)) = &tui {
                    let mut t = shared.lock().expect("tui lock");
                    t.reset_usage();
                    let detail = if !short_id.is_empty() {
                        Some(format!("({short_id})"))
                    } else {
                        None
                    };
                    t.push_action("New conversation started", detail.as_deref());
                } else {
                    if !short_id.is_empty() {
                        println!("✓ New conversation started ({short_id})");
                    } else {
                        println!("✓ New conversation started");
                    }
                }

                if let Some(prompt_text) = initial_prompt {
                    if let Some((shared, _)) = &tui {
                        shared.lock().expect("tui lock").push_user_prompt(&prompt_text);
                    } else {
                        println!("❯ {prompt_text}");
                    }
                    pending_command = Some(ReplCommand::Prompt(prompt_text));
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
            ReplCommand::Thinking(level) => {
                handle_thinking(config, &cwd, level, &mut agent, tui.as_ref().map(|(s, _)| s), &mut hide_thinking).await;
                continue;
            }
            ReplCommand::Provider => {
                let bg = tui.as_ref().map(|(shared, _)| shared.lock().expect("tui lock").snapshot());
                match crate::setup::run_provider_menu(config, bg.as_ref()).await {
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
            ReplCommand::Unknown(_) => {
                println!("unknown command");
                continue;
            }
            ReplCommand::Prompt(prompt_text) => {
                if agent.is_none() {
                    if unconfigured {
                        let bg = tui.as_ref().map(|(shared, _)| shared.lock().expect("tui lock").snapshot());
                        match crate::setup::run_provider_menu(config, bg.as_ref()).await {
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
                        Ok(built) => {
                            if !pending_history.is_empty() {
                                agent = Some(built.with_messages(std::mem::take(&mut pending_history)));
                            } else {
                                agent = Some(built);
                            }
                        }
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
                let images = std::mem::take(&mut pending_images);
                let user_msg = build_user_message_with_images(&prompt_text, &images);
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
                        match poll(std::time::Duration::from_millis(50)) {
                            Ok(true) => {}
                            _ => continue,
                        }
                        if let Ok(Event::Key(KeyEvent {
                            code,
                            modifiers,
                            kind,
                            ..
                        })) = read()
                        {
                            if kind == KeyEventKind::Release {
                                continue;
                            }
                            if code == KeyCode::Esc
                                || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
                            {
                                watch_cancel.cancel();
                                return;
                            }
                        }
                    }
                });

                let mut current_tool_name: Option<String> = None;
                let mut current_tool_args: Option<serde_json::Value> = None;
                let mut turn_usage: Option<gray_core::event::Usage> = None;
                let run_result = {
                    let mut on_event = |ev: &AgentEvent| {
                        if let Some(shared) = &tui_stream
                            && let Ok(mut t) = shared.lock()
                        {
                            match ev {
                                AgentEvent::ThinkingDelta { delta } => t.stream_thinking(delta),
                                AgentEvent::TextDelta { delta } => t.stream_text(delta),
                                AgentEvent::ToolCallStart { name, .. } => {
                                    t.end_thinking();
                                    current_tool_name = Some(name.clone());
                                    current_tool_args = None;
                                }
                                AgentEvent::ToolCallEnd { args, .. } => {
                                    t.end_thinking();
                                    let name = current_tool_name.as_deref().unwrap_or("tool");
                                    current_tool_args = Some(args.clone());
                                    let header = crate::tool_fmt::format_tool_call_header(name, args, Some(&cwd));
                                    t.push_line_spans(header);
                                }
                                AgentEvent::ToolResult { output, is_error, .. } => {
                                    let name = current_tool_name.take().unwrap_or_default();
                                    let args = current_tool_args.take();
                                    let lines = crate::tool_fmt::format_tool_result_lines_with_context(&name, args.as_ref(), output, *is_error, Some(&cwd));
                                    if !lines.is_empty() {
                                        t.push_styled_lines(lines);
                                    }
                                }
                                AgentEvent::TurnEnd { usage, .. } => {
                                    turn_usage = Some(*usage);
                                    t.end_thinking();
                                    t.set_usage(*usage);
                                    if usage.total() > 0 {
                                        t.push_usage(format!(
                                            "\u{2b22} {} tok",
                                            fmt_usage(usage.total())
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        } else if !interactive {
                            match ev {
                                AgentEvent::TextDelta { delta } => print!("{delta}"),
                                AgentEvent::ThinkingDelta { delta } => print!("{THINKING_STYLE}{delta}\x1b[0m"),
                                AgentEvent::ToolCallStart { name, .. } => {
                                    current_tool_name = Some(name.clone());
                                    current_tool_args = None;
                                }
                                AgentEvent::ToolCallEnd { args, .. } => {
                                    let name = current_tool_name.as_deref().unwrap_or("tool");
                                    current_tool_args = Some(args.clone());
                                    println!("\n{}", crate::tool_fmt::format_tool_call_header_plain(name, args, Some(&cwd)));
                                }
                                AgentEvent::ToolResult { output, is_error, .. } => {
                                    let name = current_tool_name.take().unwrap_or_default();
                                    let args = current_tool_args.take();
                                    let res = crate::tool_fmt::format_tool_result_plain_with_context(&name, args.as_ref(), output, *is_error, Some(&cwd));
                                    if !res.is_empty() {
                                        print!("{res}");
                                    }
                                }
                                AgentEvent::TurnEnd { usage, .. } => {
                                    turn_usage = Some(*usage);
                                    if usage.total() > 0 {
                                        println!("\n\x1b[2m\u{2b22} {} tok\x1b[0m", fmt_usage(usage.total()));
                                    }
                                }
                                _ => {}
                            }
                            let _ = std::io::stdout().flush();
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
                        persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await;
                    }
                    Err(CoreError::Cancelled) => {
                        persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await;
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
                        persist_turn_messages(&mut session_state, agent, config, &cwd, initial_count, turn_usage).await;
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
        assert_eq!(parse_command("/key"), ReplCommand::Provider);
        assert_eq!(parse_command("/keys"), ReplCommand::Provider);
        assert_eq!(parse_command("/key openrouter"), ReplCommand::Provider);
        assert_eq!(parse_command("/key  deepseek "), ReplCommand::Provider);
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
        assert_eq!(parse_command("/new"), ReplCommand::New(None));
        assert_eq!(parse_command("  /new  "), ReplCommand::New(None));
        assert_eq!(parse_command("/clear"), ReplCommand::New(None));
        assert_eq!(parse_command("/new make a snake game"), ReplCommand::New(Some("make a snake game".into())));
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
        assert_eq!(parse_command("/thinking"), ReplCommand::Thinking(None));
        assert_eq!(parse_command("  /thinking  "), ReplCommand::Thinking(None));
        assert_eq!(parse_command("/thinking off"), ReplCommand::Thinking(Some("off".into())));
        assert_eq!(parse_command("/thinking high"), ReplCommand::Thinking(Some("high".into())));
        assert_eq!(parse_command("/effort"), ReplCommand::Thinking(None));
        assert_eq!(parse_command("/effort high"), ReplCommand::Thinking(Some("high".into())));
        assert_eq!(parse_command("/effort low"), ReplCommand::Thinking(Some("low".into())));
    }

    #[test]
    fn parse_command_identifies_resume() {
        assert_eq!(parse_command("/resume"), ReplCommand::Resume(ResumeArgs { target: None, last: false, all: false }));
        assert_eq!(parse_command("/resume --last"), ReplCommand::Resume(ResumeArgs { target: None, last: true, all: false }));
        assert_eq!(parse_command("/resume --all"), ReplCommand::Resume(ResumeArgs { target: None, last: false, all: true }));
        assert_eq!(parse_command("/resume abc123"), ReplCommand::Resume(ResumeArgs { target: Some("abc123".into()), last: false, all: false }));
        assert_eq!(parse_command("/resume abc --last --all"), ReplCommand::Resume(ResumeArgs { target: Some("abc".into()), last: true, all: true }));
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
    fn fmt_usage_formats_exact_total() {
        assert_eq!(fmt_usage(0), "0");
        assert_eq!(fmt_usage(500), "500");
        assert_eq!(fmt_usage(999), "999");
        assert_eq!(fmt_usage(1000), "1000");
        assert_eq!(fmt_usage(1200), "1200");
        assert_eq!(fmt_usage(10500), "10500");
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
    fn fmt_event_turn_end_formats_clean_usage() {
        let usage = Usage { input_tokens: 100, output_tokens: 200, reasoning_tokens: 64, ..Default::default() };
        assert_eq!(
            fmt_event(&AgentEvent::turn_end(StopReason::EndTurn, usage)),
            "\n\x1b[2m· 300 tok\x1b[0m\n"
        );
    }
}
