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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;

use crate::setup::read_line;

/// Static slash-command table driving both `/help` and the autocomplete panel.
pub(crate) const COMMANDS: &[(&str, &str)] = &[
    ("new", "start a fresh conversation"),
    ("model", "switch or pick a model"),
    ("provider", "configure provider (API key, accounts, free tier)"),
    ("sys", "view, edit, or restore the system prompt"),
    ("help", "print the command list"),
    ("quit", "exit (Ctrl-C exits at the prompt, cancels mid-turn)"),
];

/// Commands whose name starts with `filter` (the text after '/'), table order.
fn completion_matches(filter: &str) -> Vec<(&'static str, &'static str)> {
    let f = filter.to_lowercase();
    COMMANDS
        .iter()
        .filter(|(n, _)| n.starts_with(&f))
        .copied()
        .collect()
}

/// Pure prompt-buffer edit op (unit-tested without a tty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PromptEdit {
    Insert(char),
    Backspace,
    Clear,
}

fn apply_edit(buf: &str, op: PromptEdit) -> String {
    let mut b = buf.to_string();
    match op {
        PromptEdit::Insert(c) => b.push(c),
        PromptEdit::Backspace => {
            b.pop();
        }
        PromptEdit::Clear => b.clear(),
    }
    b
}

/// True while an agent turn is in flight; controls what Ctrl-C means.
static TURN_ACTIVE: AtomicBool = AtomicBool::new(false);
static TURN_TOKEN: StdMutex<Option<tokio_util::sync::CancellationToken>> = StdMutex::new(None);

/// Installs the single global Ctrl-C policy:
/// - during a turn: cancel the turn (first press), the turn handler reports it
/// - at the prompt: exit cleanly
async fn spawn_ctrl_c_policy() {
    loop {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        if TURN_ACTIVE.load(Ordering::SeqCst) {
            if let Some(t) = TURN_TOKEN.lock().ok().and_then(|mut g| g.take()) {
                t.cancel();
            }
        } else {
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = write!(std::io::stdout(), "\x1b[?25h\r\n");
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

/// Formats an [`AgentEvent`] for display in the interactive REPL.
pub fn fmt_event(event: &AgentEvent) -> String {
    match event {
        AgentEvent::Start | AgentEvent::ToolCallEnd { .. } => String::new(),
        AgentEvent::TextDelta { delta } => delta.clone(),
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
                format!("\n\x1b[2m· {} tok\x1b[0m\n", fmt_usage(usage.total()))
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
    /// Start a fresh conversation (`/new`).
    New,
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
    } else if trimmed == "/provider" || trimmed == "/providers" || trimmed == "/login" {
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
            if let Err(e) = load_or_create_system_prompt_at(&path) {
                println!("{e}");
                return;
            }
            let editor = std::env::var("GRAY_EDITOR")
                .or_else(|_| std::env::var("EDITOR"))
                .or_else(|_| std::env::var("VISUAL"))
                .unwrap_or_else(|_| "vi".to_string());
            let p = path.clone();
            let status = tokio::task::spawn_blocking(move || {
                std::process::Command::new(editor).arg(&p).status()
            })
            .await;
            match status {
                Ok(Ok(s)) if s.success() => {
                    println!("system prompt saved — applies from your next message");
                    reload_agent(agent, config, cwd).await;
                }
                Ok(Ok(s)) => println!("editor exited with {s}; prompt unchanged"),
                Ok(Err(e)) => println!("could not launch editor: {e} (set $EDITOR)"),
                Err(e) => println!("editor task failed: {e}"),
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
) {
    if let Some(id) = direct {
        if Some(&id) == config.model.as_ref() {
            println!("already on {id}");
            return;
        }
        config.model = Some(id.clone());
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            saved.model = Some(id.clone());
            let _ = crate::setup::save_saved_config_at(&path, &saved);
        }
        println!("model set to {id} (saved)");
        reload_agent(agent, config, cwd).await;
        return;
    }

    if config.model.is_none() && config.api_key.is_none() && config.base_url.is_empty() {
        println!("no provider configured — run /provider to set one up");
        return;
    }

    use crate::setup::load_catalog;
    let catalog = match load_catalog() {
        Ok(c) => c,
        Err(e) => {
            println!("catalog error: {e}");
            return;
        }
    };

    // Find current provider's models, or models across catalog
    let current_provider = catalog.values().find(|p| p.base_url == config.base_url.as_str());
    let model_items: Vec<(String, String)> = if let Some(p) = current_provider {
        p.models.iter().map(|m| (m.id.clone(), m.name.clone())).collect()
    } else {
        catalog
            .values()
            .flat_map(|p| p.models.iter().map(move |m| (m.id.clone(), format!("{} ({})", m.name, p.name))))
            .collect()
    };

    let new_model = if model_items.is_empty() {
        let input = match read_line("model id: ") {
            Ok(l) => l,
            Err(_) => return,
        };
        if input.is_empty() {
            return;
        }
        input
    } else {
        match crate::setup::select_from_list("model", &model_items, true) {
            Ok(Some(i)) => model_items[i].0.clone(),
            Ok(None) => return,
            Err(e) => {
                println!("model selection error: {e}");
                return;
            }
        }
    };

    if Some(&new_model) == config.model.as_ref() {
        println!("already on {new_model}");
        return;
    }
    config.model = Some(new_model.clone());
    // persist
    let path = match crate::setup::saved_config_path() {
        Ok(p) => p,
        Err(e) => {
            println!("{e}");
            return;
        }
    };
    let mut saved = crate::setup::load_saved_config_at(&path);
    saved.model = Some(new_model.clone());
    if let Err(e) = crate::setup::save_saved_config_at(&path, &saved) {
        println!("switched for this session, but could not save: {e}");
    } else {
        println!("model set to {new_model} (saved)");
    }
    reload_agent(agent, config, cwd).await;
}

/// Repaints the prompt frame in place with ABSOLUTE cursor addressing:
/// [top rule / panel… / '› ' input / bottom rule], input row pinned one above
/// the terminal's bottom row — pi-style editor anchoring. The panel grows
/// upward without ever moving the input line; stale frame rows are cleared.
/// No scroll, no cumulative drift: every draw recomputes from `terminal::size()`.
/// `prev_panel` is the previous draw's panel row count, so shrinking the panel
/// clears its leftover rows instead of leaving ghosts between rule and input.
fn draw_prompt_frame(
    out: &mut impl Write,
    buf: &str,
    panel: &[String],
    cols: usize,
    prev_panel: usize,
) -> anyhow::Result<()> {
    let (_, rows) = crossterm::terminal::size()?;
    let rows = rows as usize;
    // All CUP rows below are 1-based. Input sits on the second-to-last screen row.
    let input_cup = rows.saturating_sub(1).max(2);
    let rule_cup = input_cup - panel.len().min(input_cup - 2) - 1; // panel grows upward, flush above input
    let prev_rule_cup = input_cup - prev_panel.min(input_cup - 2) - 1;
    let clear_from = rule_cup.min(prev_rule_cup); // cover both old and new frame tops
    let rule = format!("\x1b[2m{}\x1b[0m", "\u{2500}".repeat(cols));
    // ponytail: cursor column = char count, not display width — wide glyphs drift; unicode-width if that matters.
    let budget = cols.saturating_sub(6);
    let chars: Vec<char> = buf.chars().collect();
    let shown: String = if chars.len() > budget {
        chars[chars.len() - budget..].iter().collect()
    } else {
        buf.to_string()
    };

    write!(out, "\x1b[{clear_from};1H\x1b[J")?;
    write!(out, "\x1b[{rule_cup};1H{rule}")?;
    for (j, row) in panel.iter().enumerate() {
        write!(out, "\x1b[{};1H{row}", rule_cup + 1 + j)?;
    }
    write!(out, "\x1b[{input_cup};1H\u{203a} {shown}")?;
    write!(out, "\x1b[{rows};1H{rule}")?;
    // park the cursor on the input line, after the typed text
    write!(out, "\x1b[{input_cup};{}G", shown.chars().count() + 3)?;
    out.flush()?;
    Ok(())
}

/// Keeps the highlighted row visible within a `visible`-row window.
fn scroll_start(sel: usize, visible: usize) -> usize {
    if sel < visible { 0 } else { sel.saturating_sub(visible - 1) }
}

/// Max completion rows shown above the input line (shared by draw + erase).
const PANEL_ROWS: usize = 5;

/// Erases the whole prompt-frame zone and parks the cursor at its top, so the
/// submitted line's output starts on a clean screen with no frame residue.
fn erase_frame(out: &mut impl Write) -> anyhow::Result<()> {
    let (_, rows) = crossterm::terminal::size()?;
    let top = (rows as usize).saturating_sub(1).saturating_sub(PANEL_ROWS + 1).max(1);
    write!(out, "\x1b[{top};1H\x1b[J\x1b[{top};1H")?;
    out.flush()?;
    Ok(())
}

/// Raw-mode prompt editor: 3-line frame (top rule / '› ' buffer / bottom rule)
/// plus a pi-style slash-command completion panel when the buffer starts with '/'.
/// Returns the submitted line, or None on Ctrl-C / Ctrl-D-on-empty (exit request).
fn read_prompt_line() -> anyhow::Result<Option<String>> {
    use crossterm::event::{read, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    let mut stdout = std::io::stdout();
    let mut buf = String::new();
    let mut sel = 0usize; // highlighted row within visible matches
    let mut prev_panel = 0usize; // last draw's panel height, for ghost-row cleanup
    let mut cols = crate::term_width();

    crossterm::terminal::enable_raw_mode()?;
    let result = (|| -> anyhow::Result<Option<String>> {
        loop {
            // Panel is live while the buffer is "/..." with no whitespace yet.
            let active = buf.starts_with('/') && !buf[1..].contains(char::is_whitespace);
            let matches = if active { completion_matches(&buf[1..]) } else { Vec::new() };
            if sel >= matches.len() {
                sel = matches.len().saturating_sub(1);
            }

            let mut panel: Vec<String> = Vec::new();
            if !matches.is_empty() {
                let width = cols.saturating_sub(4);
                let start = scroll_start(sel, PANEL_ROWS);
                for (i, (name, desc)) in matches.iter().enumerate().skip(start).take(PANEL_ROWS) {
                    let body = format!("  /{name} \u{2014} {}", clip_str(desc, width));
                    panel.push(if i == sel {
                        format!("\x1b[7m{body}\x1b[0m")
                    } else {
                        body
                    });
                }
            }
            let panel: Vec<String> = panel
                .into_iter()
                .map(|l| clip_str(&l, cols))
                .collect();

            draw_prompt_frame(&mut stdout, &buf, &panel, cols, prev_panel)?;
            prev_panel = panel.len();

            match read()? {
                Event::Resize(new_cols, _) => {
                    cols = new_cols as usize; // next draw repaints at the new size
                }
                Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    return Ok(None)
                }
                Event::Key(KeyEvent { code: KeyCode::Char('d'), modifiers, kind: KeyEventKind::Press, .. })
                    if modifiers.contains(KeyModifiers::CONTROL) && buf.is_empty() =>
                {
                    return Ok(None) // EOF parity with the old cooked-mode Ctrl-D
                }
                Event::Key(KeyEvent { code, modifiers, kind: KeyEventKind::Press, .. }) if modifiers.contains(KeyModifiers::CONTROL) => {
                    match code {
                        KeyCode::Char('p') => sel = sel.saturating_sub(1),
                        KeyCode::Char('n') => sel = (sel + 1).min(matches.len().saturating_sub(1)),
                        _ => {}
                    }
                }
                Event::Key(KeyEvent { code, kind: KeyEventKind::Press, .. }) => match code {
                    KeyCode::Enter => {
                        // pi-style: complete, then submit immediately (executes the command)
                        if let Some((name, _)) = matches.get(sel) {
                            buf = format!("/{name} ");
                        }
                        return Ok(Some(buf));
                    }
                    KeyCode::Tab => {
                        if let Some((name, _)) = matches.get(sel) {
                            buf = format!("/{name} ");
                        }
                    }
                    KeyCode::Char(c) => {
                        buf = apply_edit(&buf, PromptEdit::Insert(c));
                        sel = 0;
                    }
                    KeyCode::Backspace => {
                        buf = apply_edit(&buf, PromptEdit::Backspace);
                        sel = 0;
                    }
                    KeyCode::Esc => {
                        buf = apply_edit(&buf, PromptEdit::Clear);
                        sel = 0;
                    }
                    KeyCode::Up => sel = sel.saturating_sub(1),
                    KeyCode::Down => sel = (sel + 1).min(matches.len().saturating_sub(1)),
                    _ => {}
                },
                _ => {}
            }
        }
    })();
    crossterm::terminal::disable_raw_mode()?;
    erase_frame(&mut stdout)?;
    result.map(|opt| opt.map(|mut line| {
        // strip the trailing space added by command completion ("/model " -> "/model")
        if line.starts_with('/') {
            while line.ends_with(' ') {
                line.pop();
            }
        }
        line.trim_end().to_string()
    }))
}

/// Clips to at most `max` chars.
fn clip_str(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Runs Gray in interactive REPL mode.
pub async fn run_repl_mode(config: &mut Config, resume_last: bool) -> anyhow::Result<()> {
    let _ = crossterm::terminal::disable_raw_mode();
    crate::tui::clear_screen();
    let cwd = std::env::current_dir()?;

    // pi-style boot: no forced wizard. A dim hint appears when unconfigured,
    // and the provider picker fires the moment credentials are needed.
    tokio::spawn(spawn_ctrl_c_policy());

    let mut unconfigured = config.model.is_none();
    if unconfigured {
        let ready = crate::setup::run_onboarding(config).await?;
        if !ready {
            print!("\r\x1b[2mrunning without a provider — send a message to set one up (or /provider)\x1b[0m\r\n");
        }
        print!("\r\n");
    } else {
        crate::tui::print_logo();
        print!("\r\n");
        print!(
            "\r\x1b[1m\u{2b21} gray\x1b[0m\x1b[2m {} \u{b7} Run /help for commands\x1b[0m\r\n",
            env!("CARGO_PKG_VERSION")
        );
    }

    // The agent is built lazily so the REPL opens even with no model/key configured;
    // we surface a friendly hint on first use instead of refusing to start.
    let mut agent: Option<Agent> = None;
    let mut session_state: Option<SessionState> = None;

    // `-c`: reopen the most recent session instead of starting blank.
    if resume_last
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

    // Interactive terminals get the raw-mode frame; piped input falls back
    // to plain cooked reads (scripts, tests).
    let interactive = std::io::stdin().is_terminal();
    use std::io::IsTerminal;

    loop {
        let line = if interactive {
            let line = match read_prompt_line()? {
                Some(l) => l,
                None => break,
            };
            // echo the submitted line into the transcript (frame was erased)
            println!("\x1b[2m\u{203a} {line}\x1b[0m");
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
            ReplCommand::Quit => break,
            ReplCommand::Sys(action) => {
                handle_sys(config, &cwd, action, &mut agent).await;
                continue;
            }
            ReplCommand::Model(direct) => {
                handle_model(config, &cwd, direct, &mut agent).await;
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
                println!("started a fresh conversation");
                continue;
            }
            ReplCommand::Provider => {
                match crate::setup::run_provider_menu(config).await {
                    Ok(true) => {
                        unconfigured = false;
                        reload_agent(&mut agent, config, &cwd).await;
                    }
                    Ok(false) => {}
                    Err(e) => println!("provider error: {e}"),
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
                *TURN_TOKEN.lock().expect("token lock") = Some(cancel.clone());
                TURN_ACTIVE.store(true, Ordering::SeqCst);
                let ctx = ToolContext {
                    cwd: cwd.clone(),
                    cancel: cancel.clone(),
                };
                let user_msg = Message::user(&prompt_text);
                let initial_count = agent.messages().len();

                let run_result = {
                    let mut run_future = Box::pin(agent.run(user_msg, ctx));
                    tokio::select! {
                        res = &mut run_future => res,
                        _ = cancel.cancelled() => Err(CoreError::Cancelled),
                    }
                };
                TURN_ACTIVE.store(false, Ordering::SeqCst);
                *TURN_TOKEN.lock().expect("token lock") = None;

                match run_result {
                    Ok(events) => {
                        for event in &events {
                            let rendered = fmt_event(event);
                            if !rendered.is_empty() {
                                print!("{rendered}");
                            }
                        }
                        std::io::stdout().flush()?;

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
                            session_state = Some(SessionState { store, session_id });
                        }

                        if let Some(state) = &session_state
                            && agent.messages().len() > initial_count
                        {
                            for msg in &agent.messages()[initial_count..] {
                                let _ = state.store.append(&state.session_id, msg).await;
                            }
                        }
                    }
                    Err(CoreError::Cancelled) => {
                        println!("(interrupted)");
                    }
                    Err(e) => {
                        eprintln!("agent error: {e}");
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
    fn parse_command_identifies_provider() {
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
}
