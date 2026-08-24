//! Interactive REPL mode for Gray.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use gray_core::agent::{Agent, ToolContext};
use gray_core::error::CoreError;
use gray_core::event::AgentEvent;
use gray_core::message::Message;
use gray_session::{default_root, JsonlSessionStore, SessionId, SessionMeta, SessionStore};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;

use crate::setup::read_line;

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
            println!();
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
    /// Run the interactive provider/key/model setup wizard (`/setup`).
    Setup,
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
    } else if trimmed == "/setup" {
        ReplCommand::Setup
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
    use crate::setup::load_catalog;
    let catalog = match load_catalog() {
        Ok(c) => c,
        Err(e) => {
            println!("catalog error: {e}");
            return;
        }
    };

    // Resolve target model id: either the direct argument or a picker round.
    let new_model = match direct {
        Some(id) => id,
        None => {
            if config.model.is_none() || config.api_key.is_none() {
                crate::setup::run_setup(config).ok();
                config.model.clone().unwrap_or_default()
            } else {
                let current = config.model.clone().unwrap_or_default();
                // featured providers' flagship models, catalog order (newest first)
                let ours: Vec<_> = catalog
                    .values()
                    .filter(|p| p.featured)
                    .flat_map(|p| p.models.iter().map(move |m| (p, m)))
                    .take(8)
                    .collect();
                println!("models{}:", if ours.is_empty() { " (none matched — type any id)" } else { "" });
                for (i, (_, m)) in ours.iter().enumerate() {
                    let mark = if m.id == current { " ✓" } else { "" };
                    println!("  {}. {}{}", i + 1, m.id, mark);
                }
                println!("  [a] all providers · [enter] keep {}", current);
                let input = match read_line("model: ") {
                    Ok(l) => l,
                    Err(e) => {
                        println!("{e}");
                        return;
                    }
                };
                if input.is_empty() {
                    return; // keep current
                }
                if input.eq_ignore_ascii_case("a") {
                    // delegate to the setup picker's provider stage by re-running it
                    crate::setup::run_setup(config).ok();
                    config.model.clone().unwrap_or_default()
                } else if let Ok(n) = input.parse::<usize>() {
                    if let Some((_, m)) = ours.get(n - 1) {
                        m.id.clone()
                    } else {
                        println!("no such row");
                        return;
                    }
                } else {
                    input // free text: any id the endpoint accepts
                }
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

/// Runs Gray in interactive REPL mode.
pub async fn run_repl_mode(config: &mut Config) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;

    // fx-style welcome: one line, no art.
    println!(
        "\x1b[1m\u{2b21} gray\x1b[0m\x1b[2m {} \u{b7} Run /help for commands\x1b[0m",
        env!("CARGO_PKG_VERSION")
    );

    // pi-style boot: no forced wizard. A dim hint appears when unconfigured,
    // and the provider picker fires the moment credentials are needed.
    tokio::spawn(spawn_ctrl_c_policy());

    let mut unconfigured = config.model.is_none() || config.api_key.is_none();
    if unconfigured {
        println!("\x1b[2mno provider configured yet — send a message and I'll walk you through it (or /setup)\x1b[0m");
    }
    println!("\x1b[2m{}\x1b[0m", crate::plain_rule());

    // The agent is built lazily so the REPL opens even with no model/key configured;
    // we surface a friendly hint on first use instead of refusing to start.
    let mut agent: Option<Agent> = None;
    let mut session_state: Option<SessionState> = None;

    loop {
        print!("› ");
        std::io::stdout().flush()?;

        let read_res = tokio::select! {
            res = tokio::task::spawn_blocking(|| {
                let mut line = String::new();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) => Ok(None),
                    Ok(_) => Ok(Some(line)),
                    Err(e) => Err(e),
                }
            }) => {
                match res {
                    Ok(Ok(opt)) => opt,
                    Ok(Err(e)) => return Err(anyhow::anyhow!("failed to read stdin: {e}")),
                    Err(e) => return Err(anyhow::anyhow!("stdin reader task failed: {e}")),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!();
                break;
            }
        };

        let Some(line) = read_res else {
            // EOF (Ctrl-D)
            break;
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
                println!("  /model [provider/id]  switch or pick a model");
                println!("  /setup                re-run provider configuration");
                println!("  /sys [show|reset]     view, edit, or restore the system prompt");
                println!("  /quit                 exit (Ctrl-C exits at the prompt, cancels mid-turn)");
                continue;
            }
            ReplCommand::Setup => {
                crate::setup::run_setup(config)?;
                unconfigured = false;
                reload_agent(&mut agent, config, &cwd).await;
                continue;
            }
            ReplCommand::Unknown(_) => {
                println!("unknown command");
                continue;
            }
            ReplCommand::Prompt(prompt_text) => {
                if agent.is_none() {
                    if unconfigured {
                        crate::setup::run_setup(config)?;
                        unconfigured = false;
                        println!();
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

                        if session_state.is_none() {
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
                                session_state = Some(SessionState { store, session_id });
                            }
                        }

                        if let Some(state) = &session_state {
                            if agent.messages().len() > initial_count {
                                for msg in &agent.messages()[initial_count..] {
                                    let _ = state.store.append(&state.session_id, msg).await;
                                }
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
    fn parse_command_identifies_setup() {
        assert_eq!(parse_command("/setup"), ReplCommand::Setup);
        assert_eq!(parse_command("/help"), ReplCommand::Help);
        assert_eq!(
            parse_command("/model openai/gpt-4o"),
            ReplCommand::Model(Some("openai/gpt-4o".into()))
        );
        assert_eq!(parse_command("/model"), ReplCommand::Model(None));
        assert_eq!(parse_command("  /setup  "), ReplCommand::Setup);
        assert_eq!(parse_command("/setup\n"), ReplCommand::Setup);
        // near-misses stay unknown
        assert_eq!(
            parse_command("/setup extra"),
            ReplCommand::Unknown("/setup extra".to_string())
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
