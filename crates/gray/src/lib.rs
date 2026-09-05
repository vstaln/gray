//! Gray: a minimal, modular agent harness in Rust.

pub mod compact;
pub mod composer;
pub mod config;
pub mod host;
pub mod logging;
pub mod plugin_check;
pub mod print;
pub mod profile;
pub mod repl;
pub mod resume;
pub mod setup;
pub mod skills;
pub mod skills_tool;
pub mod sys_editor;
pub mod system_prompt;
pub mod tool_fmt;
pub mod tui;
pub mod update;

use clap::Parser;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use config::Config;
pub use print::run_print_mode;
pub use profile::{build_registry, take_profile_warnings};
pub use repl::{ReplCommand, parse_command, run_repl_mode};
pub use tui::{clear_screen, print_wrapped};

use crate::skills_tool::SkillTool;

/// Default system prompt, shipped as markdown and materialized to `~/.gray/AGENTS.md`
/// on first run. Edit that file (or use the `/agentsmd` command) to change it.
pub const DEFAULT_SYS_PROMPT: &str = r#"You are gray, a minimal agent running on the user's machine.
You help by using tools: read files, run commands, edit code, search.

Guidelines:
- Be concise.
- Read surrounding code, types, and tests before changing anything; match existing patterns.
- Give error and edge cases the same care as happy paths; fix root causes.
- Verify by building and testing; only claim what you actually ran.
- Commands run non-interactively without a TTY. Never run commands that prompt for interactive passwords (e.g. `sudo` without passwordless setup, `ssh` without keys). For privileged operations, use non-interactive flags (e.g. `sudo -n`) or ask the user.
- When a concrete decision blocks progress, ask the user with the request_user_input tool (1-3 multiple-choice questions) instead of guessing; act on defaults for small choices.
- When referencing files or URLs in responses, format them with absolute paths or file:// links (e.g. file:///path/to/file or [label](file:///path/to/file)) and standard web URLs so they are clickable in the terminal.
- Keep going until done or truly blocked. A failed tool call means try differently, not give up."#;

/// Resolves the user's system-prompt file path (`$GRAY_HOME` or `$HOME/.gray`) + `AGENTS.md`.
///
/// Single editable system prompt — users add to this one file. Migrates legacy `sys.md` if present.
pub fn sys_prompt_path() -> anyhow::Result<PathBuf> {
    let base = std::env::var("GRAY_HOME")
        .or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray")))
        .map_err(|_| anyhow::anyhow!("cannot resolve home: set HOME or GRAY_HOME"))?;
    Ok(PathBuf::from(base).join("AGENTS.md"))
}

/// Loads the system prompt from `path`, writing the embedded default there first if absent.
/// If `AGENTS.md` is missing but legacy `sys.md` exists, migrates it.
pub fn load_or_create_system_prompt_at(path: &Path) -> anyhow::Result<String> {
    if let Ok(body) = std::fs::read_to_string(path) {
        return Ok(body);
    }
    // Migrate legacy sys.md -> AGENTS.md (one-time)
    if path.file_name().is_some_and(|n| n == "AGENTS.md")
        && let Some(parent) = path.parent()
        && let Ok(body) = std::fs::read_to_string(parent.join("sys.md"))
        && !body.trim().is_empty()
        && std::fs::write(path, &body).is_ok()
    {
        return Ok(body);
    }
    match std::fs::read_to_string(path) {
        Ok(body) => Ok(body),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, DEFAULT_SYS_PROMPT)?;
            Ok(DEFAULT_SYS_PROMPT.to_string())
        }
        Err(e) => Err(e.into()),
    }
}

/// Terminal width, queried live via crossterm on every call so resizes are picked up; falls back to 80.
pub fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .ok()
        .filter(|&w| w >= 20)
        .unwrap_or(80)
}

/// Renders an fx-style labeled rule filling the terminal width:
/// `── label ────────────────────────────`
pub fn rule(label: &str) -> String {
    let prefix = format!("\u{2500}\u{2500} {label} ");
    let used = prefix.chars().count();
    let fill = term_width().saturating_sub(used);
    format!("{prefix}{}", "\u{2500}".repeat(fill))
}

/// Prompt cache-key helpers live in the shared builder (single definition);
/// re-exported here so existing imports keep working.
pub use gray_plugin::builder::{
    PROMPT_CACHE_KEY_MAX_LENGTH, clamp_prompt_cache_key, provider_cache_key,
};

/// Builds the interactive [`gray_core::agent::Agent`]: thin surface wrapper
/// over [`gray_plugin::builder::build_agent`] (the single profile-aware
/// builder for REPL, `-p`, gateway, and cron).
///
/// Surface policy owned here: missing-model help text, `AGENTS.md` body,
/// skills + context-file discovery, the `skill` tool default, and the
/// REPL/`-p` host handler. `session_id` pins the Responses
/// `prompt_cache_key` for cache affinity — pass it whenever known (resume,
/// /new); `None` uses a per-process stable id.
/// (A single function: earlier split variants had an unused `None` leg.)
///
/// Skills are discovered via [`skills::discover_skills`] (global `~/.gray/skills`,
/// OpenCode plugins, `~/.agents/skills`, `~/.claude/skills` + project skills
/// walked up to git root) respecting `.gitignore`/`.ignore`/`.fdignore`, and
/// `AGENTS.md`/`CLAUDE.md` context files are discovered walking up to git root
/// and appended as `<project_context>` blocks. Skills are only surfaced when the
/// `read` tool is present.
///
/// Errors here are user-configuration problems (missing model or API key), so the
/// message is written for a human, not a log file.
pub async fn build_agent(
    config: &Config,
    cwd: &Path,
    session_id: Option<&str>,
) -> anyhow::Result<gray_core::agent::Agent> {
    let Some(model) = &config.model else {
        anyhow::bail!(
            "no model configured yet — run /provider (or set --model <provider/model>), then try again"
        );
    };
    // Keyless upstreams (free tiers, local servers) run with an empty key.
    let api_key = config.api_key.as_deref().unwrap_or("");
    let body = load_or_create_system_prompt_at(&sys_prompt_path()?)?;

    // Discover skills + AGENTS.md / CLAUDE.md context (prompt needs the
    // resolved registry, so this becomes a builder closure below).
    let discovered = skills::discover_skills(cwd);
    let context_files = system_prompt::discover_context_files(cwd);
    let prompt_cwd = cwd.to_path_buf();

    let agent = gray_plugin::builder::build_agent(gray_plugin::builder::BuilderOptions {
        model: model.clone(),
        api_key: api_key.to_string(),
        base_url: config.base_url.clone(),
        reasoning_effort: config.thinking_effort.clone(),
        session_id: session_id.map(str::to_string),
        cwd: cwd.to_path_buf(),
        system_prompt: gray_plugin::builder::SystemPrompt::Build(Box::new(
            move |registry: &gray_tools::Registry| {
                // Tools only appear in the prompt when they have a snippet.
                // Same plugins feed the registry and the agent hooks.
                let tool_snippets = registry.prompt_snippets();
                let selected_tools = registry.tool_names();
                let guidelines = registry.prompt_guidelines();
                let prompt_guidelines = if guidelines.is_empty() {
                    None
                } else {
                    Some(guidelines)
                };
                system_prompt::build_system_prompt(system_prompt::BuildSystemPromptOptions {
                    custom_prompt: Some(body),
                    selected_tools: Some(selected_tools),
                    tool_snippets: Some(tool_snippets),
                    prompt_guidelines,
                    append_system_prompt: None,
                    cwd: prompt_cwd,
                    context_files: Some(context_files),
                    skills: Some(discovered.skills),
                })
            },
        )),
        // Sidecars get the host runner so plugin-initiated `host/run`
        // (cron fires) / `host/say` don't fall back to loud `{"error":…}`.
        extra_tools: vec![Arc::new(SkillTool)],
        host_handler: Some(host::default_handler(cwd.to_path_buf())),
        profile_path: "gray.yml".to_string(),
        abort_on_spawn_failure: true,
        wrap_executor: None,
    })
    .await?;
    for w in gray_plugin::builder::take_builder_warnings() {
        profile::queue_profile_warning(w);
    }
    Ok(agent)
}

/// Command-line arguments for the Gray harness.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "gray",
    version,
    about = "gray — a minimal, modular agent harness in Rust.",
    after_help = "Run with no arguments for the interactive REPL. Use -p for one-shot print mode."
)]
pub struct Cli {
    /// Model to use (e.g. provider/model-id)
    #[arg(long)]
    pub model: Option<String>,

    /// Custom API base URL
    #[arg(long)]
    pub base_url: Option<String>,

    /// Print mode: execute prompt directly and print output
    #[arg(short = 'p', long = "print")]
    pub print: Option<String>,

    /// API key for authentication (overrides GRAY_API_KEY and OPENAI_API_KEY)
    #[arg(long)]
    pub api_key: Option<String>,

    /// Continue the most recent conversation
    #[arg(short = 'c', long = "continue")]
    pub continue_last: bool,

    /// Resume a specific session by id (see the hint printed on exit)
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,

    /// Override model context window in tokens (e.g. 128000 or 128k). Env: GRAY_CONTEXT_WINDOW. Highest priority over auto-fetched provider value.
    #[arg(long, value_name = "TOKENS", value_parser = parse_context_window_cli)]
    pub context_window: Option<usize>,

    /// Reserve tokens before auto-compact fires (e.g. 16k). Env: GRAY_CONTEXT_RESERVE.
    #[arg(long, value_name = "TOKENS", value_parser = parse_context_window_cli)]
    pub context_reserve: Option<usize>,

    /// Tail budget kept alongside the summary after compaction (e.g. 20k). Env: GRAY_CONTEXT_KEEP.
    #[arg(long, value_name = "TOKENS", value_parser = parse_context_window_cli)]
    pub context_keep: Option<usize>,

    /// Print the merged plugin manifest as JSON and exit
    #[arg(long = "dump-manifest")]
    pub dump_manifest: bool,

    /// Resume subcommand (picker by default; see `gray resume --help`)
    #[command(subcommand)]
    pub command: Option<Commands>,
}

fn parse_context_window_cli(s: &str) -> Result<usize, String> {
    crate::setup::parse_context_window(s)
        .ok_or_else(|| format!("invalid context window '{s}' — use e.g. 128000, 128k, 1m"))
}

/// Subcommands mirroring `codex resume` / `codex fork` ergonomics.
#[derive(Parser, Debug, Clone)]
pub enum Commands {
    /// Resume a previous conversation
    Resume {
        /// Session id (UUID or prefix). If omitted, shows picker unless --last.
        #[arg(value_name = "SESSION_ID")]
        session_id: Option<String>,
        /// Resume the most recent session without showing the picker
        #[arg(long)]
        last: bool,
        /// Show all sessions (disables cwd filtering)
        #[arg(long)]
        all: bool,
    },
    /// Messaging gateway (Telegram/Discord/Slack) — daemon on VPS
    Gateway {
        #[command(subcommand)]
        cmd: Option<GatewayCmd>,
    },
    /// Plugin tools (conformance check)
    Plugin {
        #[command(subcommand)]
        cmd: PluginCmd,
    },
    /// Update gray to the latest release
    Update,
}

#[derive(Parser, Debug, Clone)]
pub enum GatewayCmd {
    /// Run the gateway daemon (foreground)
    Run,
    /// Show gateway status
    Status,
    /// Install systemd user service (gray-gateway.service)
    Install,
    /// Uninstall systemd service
    Uninstall,
    /// Print the OAuth2 invite URL for a platform (discord)
    Invite {
        /// Platform to invite (discord)
        #[arg(default_value = "discord")]
        platform: String,
    },
    /// Approve/deny chat pairing requests (bind the owner without editing gateway.yaml)
    Pairing {
        #[command(subcommand)]
        cmd: PairingCmd,
    },
}

/// `gray plugin ...` — plugin-side tooling.
#[derive(Parser, Debug, Clone)]
pub enum PluginCmd {
    /// Run the sidecar conformance checks against a plugin dir
    Check {
        /// Plugin directory (executable, plugin.sh, or single executable)
        dir: String,
    },
}

/// `gray gateway pairing ...` — runtime owner binding without editing gateway.yaml.
#[derive(Parser, Debug, Clone)]
pub enum PairingCmd {
    /// Approve a pending DM code (`pairing approve discord ABC12345`)
    Approve {
        /// Platform the code came from
        platform: String,
        /// Pairing code the user received
        code: String,
    },
    /// Show pending + approved users (`pairing list [discord|all]`)
    List {
        /// Platform or `all`
        #[arg(default_value = "all")]
        platform: String,
    },
    /// Drop a user's approval
    Revoke {
        /// Platform
        platform: String,
        /// Approved user id
        user: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_prefers_session_id() {
        assert_eq!(
            provider_cache_key(Some("cc5d154d-4c24-42ee-b8a8-6a5735bdcfc9")),
            "cc5d154d-4c24-42ee-b8a8-6a5735bdcfc9"
        );
    }

    #[test]
    fn cache_key_fallback_is_stable_per_process() {
        // Rebuilds mid-session (reload, lazy builds) must not rotate the key.
        assert_eq!(provider_cache_key(None), provider_cache_key(None));
        assert_eq!(provider_cache_key(Some("")), provider_cache_key(None));
    }

    #[test]
    fn cache_key_clamped_to_64_chars() {
        let long = "s".repeat(100);
        assert_eq!(provider_cache_key(Some(&long)).len(), 64);
    }
}
