//! Gray: a minimal, modular agent harness in Rust.

pub mod ask;
pub mod cache;
pub mod compact;
pub mod composer;
pub mod config;
pub mod cron;
pub mod cron_fire;
pub mod cron_serve;
pub mod cron_status;
pub mod feedback;
pub mod gateway;
pub mod host;
pub mod logging;
pub mod memory;
pub mod plugin_check;
pub mod plugin_cli;
pub mod print;
mod print_meter;
pub mod profile;
pub mod repl;
pub mod resume;
mod rotation;
pub mod session_store;
pub mod setup;
pub mod shell_drain;
pub mod skills;
pub mod skills_tool;
pub mod sys_editor;
pub mod system_prompt;
pub(crate) mod text_width;
pub mod theme;
pub mod tool_fmt;
pub mod tui;
pub mod turn_caps;
pub mod update;

use clap::Parser;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use config::Config;
pub use print::run_print_mode;
pub use profile::{build_registry, take_profile_warnings};
pub use repl::{ReplCommand, parse_command, run_repl_mode};
pub use tui::{clear_screen, print_wrapped};

/// Default system prompt, shipped as markdown and materialized to `~/.gray/AGENTS.md`
/// on first run. Edit that file (or use the `/agentsmd` command) to change it.
pub const DEFAULT_SYS_PROMPT: &str = r#"<!--
Unreadable note: this HTML comment stays in the file but is stripped before
the prompt reaches the model — only the text after this note reaches it.

This file IS the stored system prompt, sent verbatim every turn. Gray
adds the runtime working directory and ephemeral per-turn context: the
<available_skills> list (fresh skill discovery for the turn's directory) — no
skill tool, read matches with bash. Edit with `/agentsmd` (Ctrl-S save &
apply, Ctrl-R reset to this default, Ctrl-X cancel).
-->
You are gray, a minimal agent running on the user's machine.
You work through one tool: blocking `bash`. Use bash to read, search, edit, and run things (e.g. `cat`, `rg`, `sed`, `python3`).
Before working in a project, read its AGENTS.md / CLAUDE.md with bash. When a task matches a skill listed in <available_skills> (appended to your context each turn), read its SKILL.md with bash (`cat <location>`) and follow its instructions. `/skills <name>` in chat pastes the skill visibly before running it.
To schedule recurring work for the user, run `gray cron add "<schedule>" "<prompt>"` (manage with `gray cron list/show/remove`).

Workflow (do every task this way):
1. Derive the contract from the repository, not just the request: search every call site and read the existing tests, types, and callers before changing anything; match sibling code and reuse its helpers. The contract includes what the request leaves implicit — exception types, error messages, parameter names, return shapes.
2. Treat the request as a checklist and cover every clause — errors, edge cases, and negative paths carry the same weight as the happy path. Fix root causes, never symptoms.
3. For bug reports, reproduce the failure against the real code before fixing it. Never let a check you wrote yourself define correctness, and never weaken correct code to make your own check pass. Never edit, skip or delete a test to make something pass.
4. Verify with the project's own build and tests; run the tests covering what you touched, whole files unmodified, and write tests for new behavior — negative and boundary cases included. A green existing suite only proves you did not regress it.
5. Before finishing, verify your own result: re-read every file you wrote and re-run your own checks (trailing newlines and exact bytes matter).

## The spec is a checklist of contracts

A request describes the happy path and leaves the rest implicit. Before writing code, answer for each clause: the exact output (bytes, whitespace, order, exception class, message), the state it owns, and the layer it belongs in. Archaeology is not implementation — if many calls have gone into reading, re-read the request for the pointer you missed.

## Your own tests are not evidence

Tests written from the same reading as the code prove the code matches your assumptions, nothing more. Before finishing: one adversarial check per clause (wrong byte, wrong exception, missing edge case), plus the project's real suite. If a check fails for an environmental reason (no network, missing binary), note it and move on. Name scratch tests so they cannot collide with the project's own test files (`zzgray_` prefix or equivalent).

## Probes are one-shot

When the environment blocks something (no network, missing binary), probe once, record the result, and stop retrying that path — spend the budget on the work.

Guidelines:
- Be concise.
- Work in parallel: when several calls don't depend on each other, send them all in one turn. Read-only and non-interfering calls run concurrently; anything that might clash is serialized for you.
- Commands run non-interactively without a TTY. Never run commands that prompt for interactive passwords (e.g. `sudo` without passwordless setup, `ssh` without keys). Use non-interactive flags (e.g. `sudo -n`) instead.
- When the next step is clear, keep going without asking, until done or truly blocked. A failed tool call means try differently, not give up.
- If a file changes unexpectedly under you (a parallel agent may be active), don't fight it: re-read before writing, reconcile instead of overwriting, and never get into an edit war.
- Ground every claim about code, tests, or tools in something you actually read or ran."#;

/// Resolves the user's system-prompt file path (`$GRAY_HOME` or `$HOME/.gray`) + `AGENTS.md`.
///
/// Single editable system prompt — users add to this one file. Migrates legacy `sys.md` if present.
pub fn sys_prompt_path() -> anyhow::Result<PathBuf> {
    Ok(crate::setup::gray_home()?.join("AGENTS.md"))
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
/// builder for REPL and `-p`).
///
/// Surface policy owned here: missing-model help text, `AGENTS.md` body,
/// the always-on context-only `skills` plugin (tools stay bash-only),
/// and the REPL/`-p` host handler.
/// `session_id` pins the Responses `prompt_cache_key` for cache affinity —
/// pass it whenever known (resume, /new); `None` uses a per-process stable id.
/// (A single function: earlier split variants had an unused `None` leg.)
///
/// Skills are `SKILL.md` files discovered via [`skills::discover_skills`]
/// (global `~/.gray/skills`, OpenCode plugins, `~/.agents/skills`,
/// `~/.claude/skills` + project skills walked up to git root) respecting
/// `.gitignore`/`.ignore`/`.fdignore`.
/// The context-only [`skills_tool::SkillsPlugin`] (always active, every
/// profile) serves the per-turn `<available_skills>` block through the
/// `prompt/context` hook — `None` when nothing is discovered, so the system
/// prefix stays byte-stable for prefix caching. No `skill` tool: tools stay
/// bash-only, the model reads matches with `cat`. `/skills <name>` pastes
/// one visibly into chat before running it.
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
    // Same directory as the tool context; never persist it in the user's file.
    let prompt_cwd = cwd.to_path_buf();

    let snapshot = if memory::disabled() {
        None
    } else {
        match setup::gray_home()
            .and_then(|home| memory::MemoryStore::new(&home, cwd)?.snapshot(session_id))
        {
            Ok(snapshot) => Some(snapshot),
            Err(_) => {
                profile::queue_profile_warning("Memory unavailable; continuing without memory. Inspect `gray memory list` and `gray memory --scope user list`.".to_string());
                None
            }
        }
    };
    let agent = gray_plugin::builder::build_agent(gray_plugin::builder::BuilderOptions {
        model: model.clone(),
        api_key: api_key.to_string(),
        base_url: config.base_url.clone(),
        reasoning_effort: config
            .thinking_effort
            .as_deref()
            .map(|effort| crate::setup::clamp_thinking_level(model, effort).to_string()),
        temperature: config.temperature,
        top_p: config.top_p,
        context_window: Some(crate::setup::context::resolve_model_context_length(model)),
        session_id: session_id.map(str::to_string),
        cwd: cwd.to_path_buf(),
        // Keep stored instructions intact; append runtime cwd before any turn.
        system_prompt: gray_plugin::builder::SystemPrompt::Build(Box::new(
            move |_registry: &gray_tools::Registry| {
                system_prompt::with_memory(
                    system_prompt::build_runtime_prompt(Some(body), &prompt_cwd),
                    snapshot.as_deref(),
                )
            },
        )),
        // Sidecars get the host runner so plugin-initiated `host/run`
        // / `host/say` don't fall back to loud `{"error":…}`.
        // Bash-only tools; the context-only skills plugin is always on
        // (every profile, including the default `tools-minimal`).
        extra_plugins: vec![Arc::new(crate::skills_tool::SkillsPlugin::default())],
        host_handler: Some(host::default_handler(cwd.to_path_buf())),
        profile_path: "gray.yml".to_string(),
        abort_on_spawn_failure: true,
        wrap_executor: None,
    })
    .await?;
    for w in gray_plugin::builder::take_builder_warnings() {
        profile::queue_profile_warning(w);
    }
    // Bash bounds an explicitly requested timeout at 3600 s (and has no
    // default), so the agent-level timeout must sit above that (P2B
    // requirement): it is a last-resort stop, never a budget.
    Ok(agent.with_tool_timeout(crate::shell_drain::SHELL_TOOL_TIMEOUT))
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

    /// Emit versioned NDJSON progress and a final result instead of terminal output
    #[arg(long, requires = "print")]
    pub json: bool,

    /// Maximum provider requests in a JSON print invocation (includes compaction)
    #[arg(long, requires = "json", value_parser = clap::value_parser!(u32).range(1..))]
    pub max_requests: Option<u32>,
    /// Conservative model input USD per million tokens for budget accounting
    #[arg(long, requires = "json")]
    pub input_price: Option<f64>,
    /// Conservative model output USD per million tokens for budget accounting
    #[arg(long, requires = "json")]
    pub output_price: Option<f64>,

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

    /// Maximum agent turns per invocation (mini-swe-agent step_limit).
    /// Env: GRAY_MAX_TURNS. Applies to REPL turns this process runs.
    #[arg(long, value_name = "N")]
    pub max_turns: Option<u32>,

    /// Maximum spend in USD per invocation (mini-swe-agent cost_limit).
    /// Env: GRAY_MAX_COST_USD. Unpriced models never trip this.
    #[arg(long, value_name = "USD")]
    pub max_cost_usd: Option<f64>,

    /// Maximum wall-clock seconds per invocation (mini-swe-agent wall_time).
    /// Env: GRAY_MAX_WALL_SECS. Measured from process start.
    #[arg(long, value_name = "SECS")]
    pub max_wall_secs: Option<u64>,

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
    /// Curated cross-session memory (local files, no model required)
    Memory(memory::MemoryArgs),
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
    /// Plugin tools (conformance check)
    #[command(visible_alias = "plugins")]
    Plugin {
        #[command(subcommand)]
        cmd: PluginCmd,
    },
    /// Gateway daemon: cron ticker + control socket, supervised as a service
    Gateway {
        #[command(subcommand)]
        cmd: GatewayCmd,
    },
    /// Update gray to the latest release
    #[command(visible_alias = "upgrade")]
    Update,
    /// Cron jobs: list/add/remove/show (file-only over $GRAY_HOME/cron, no daemon needed)
    Cron {
        #[command(subcommand)]
        cmd: CronCmd,
    },
    /// Session store maintenance
    Sessions {
        #[command(subcommand)]
        cmd: SessionsCmd,
    },
    /// Install a catalog plugin or register a native executable (gray install plugin NAME)
    Install {
        #[command(subcommand)]
        cmd: InstallCmd,
    },
    /// Plugin-provided commands (`gray NAME setup`, …) — forwarded to the plugin
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// `gray install plugin <name>` — native plugin registration.
#[derive(Parser, Debug, Clone)]
pub enum InstallCmd {
    /// Register a plugin command from PATH or GRAY_PLUGIN_PATH
    Plugin {
        /// Plugin name
        #[arg(value_name = "NAME")]
        name: String,
    },
}

/// `gray sessions ...` — session store maintenance.
#[derive(Parser, Debug, Clone)]
pub enum SessionsCmd {
    /// Delete sessions started more than N days ago
    Prune {
        /// Age threshold in days (default 90)
        #[arg(long, default_value_t = 90)]
        older_than_days: u64,
    },
}

/// `gray cron ...` — recurring/one-shot job management.
///
/// Thin CLI over `gray-cron::CronStore`: `add` runs the store's validation
/// (schedule shape) inline, so no daemon round-trip.
#[derive(Parser, Debug, Clone)]
pub enum CronCmd {
    /// List jobs (id, name, schedule, next run, last status)
    List,
    /// Add a job: schedule ("every 1h" / "30m" / "in 10m" / RFC3339 / "0 9 * * *") + prompt
    Add {
        /// Schedule expression
        schedule: String,
        /// Prompt the daemon runs at fire time
        prompt: String,
        /// Delivery target: local saves a file; origin appends to --origin-session then saves (anything else saves only)
        #[arg(long)]
        deliver: Option<String>,
        /// Origin chat session id (required with --deliver origin)
        #[arg(long = "origin-session")]
        origin_session: Option<String>,
        /// Job name (default: prompt's first line, truncated)
        #[arg(long)]
        name: Option<String>,
        /// Working dir the job runs in (must be absolute + existing)
        #[arg(long = "in", value_name = "DIR")]
        workdir: Option<PathBuf>,
        /// Comma-separated skill names (must resolve at fire time)
        #[arg(long)]
        skills: Option<String>,
        /// Absolute path to a pre-run script (stdout injected into prompt)
        #[arg(long)]
        script: Option<PathBuf>,
    },
    /// One claim→fire→record pass (also the OS-cron/runit entry point)
    Tick,
    /// Tick every 60s until SIGINT/SIGTERM
    Serve,
    /// Suspend a job (id or name)
    Pause {
        /// Job id or name
        id: String,
    },
    /// Resume a suspended job (id or name; recomputes next run)
    Resume {
        /// Job id or name
        id: String,
    },
    /// Fire a job now regardless of schedule (id or name)
    Run {
        /// Job id or name
        id: String,
    },
    /// Show one job's full record (id or name)
    Show {
        /// Job id or name
        id: String,
    },
    /// Remove a job (id or name)
    Remove {
        /// Job id or name
        id: String,
    },
}

/// `gray gateway ...` — the daemon host (hermes-shaped; adapters live elsewhere).
#[derive(Parser, Debug, Clone)]
pub enum GatewayCmd {
    /// Run in the foreground (what the service/supervisor executes)
    Run,
    /// Report daemon + service + cron-ticker health (exit 1 when not running)
    Status,
    /// Start the installed service (runit/systemd)
    Start,
    /// Stop the installed service, or SIGTERM a foreground process
    Stop,
    /// Restart the installed service
    Restart,
    /// Install (and start) a user service running `gray gateway run`
    Install {
        /// Write the service but do not start it
        #[arg(long)]
        no_start: bool,
        /// Print what would be written; write nothing
        #[arg(long)]
        print: bool,
    },
    /// Stop and remove the installed service
    Uninstall,
}

/// `gray plugin ...` — plugin-side tooling.
#[derive(Parser, Debug, Clone)]
pub enum PluginCmd {
    /// List installed plugins
    List,
    /// Install a plugin by index name or https URL
    Install {
        /// Index name or https URL
        #[arg(value_parser = |s: &str| Ok::<_, std::convert::Infallible>(gray_pkg::ops::parse_spec(s)))]
        spec: gray_pkg::ops::NameOrUrl,
    },
    /// Remove an installed plugin
    Remove {
        /// Installed plugin name
        name: String,
    },
    /// Update one plugin or all (`all`)
    Update {
        /// Plugin name or `all`
        #[arg(default_value = "all")]
        target: String,
    },
    /// Enable an installed plugin
    Enable {
        /// Installed plugin name
        name: String,
    },
    /// Disable an installed plugin
    Disable {
        /// Installed plugin name
        name: String,
    },
    /// Run the sidecar conformance checks against a plugin dir
    Check {
        /// Plugin directory (executable, plugin.sh, or single executable)
        dir: String,
    },
}

#[path = "lib_tests.rs"]
#[cfg(test)]
mod tests;
