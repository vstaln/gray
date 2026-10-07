//! Gray: a minimal, modular agent harness in Rust.

pub mod ask;
pub mod auth;
pub mod cache;
pub mod compact;
pub mod composer;
pub mod config;
pub mod cron;
pub mod cron_fire;
pub mod cron_serve;
pub mod cron_status;
pub mod doctor;
pub mod feedback;
pub mod gateway;
pub mod host;
pub mod keymap;
pub mod logging;
pub(crate) mod mascot;
pub mod plugin_check;
pub mod plugin_cli;
pub mod print;
mod print_meter;
pub mod profile;
pub mod prompt_templates;
pub mod providers;
pub mod repl;
pub mod resume;
mod rotation;
pub mod search;
pub mod session_store;
pub mod setup;
pub mod shell_drain;
pub mod skills;
pub mod skills_tool;
pub mod spill;
pub mod sys_editor;
pub mod system_prompt;
pub mod term_keys;
pub(crate) mod text_width;
pub mod theme;
pub mod tool_fmt;
pub mod tui;
pub mod turn_caps;
pub mod update;

use clap::{ArgGroup, Parser};
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

This file IS the stored system prompt, sent verbatim every turn with no
runtime context appended. Only what the model can't already know: the
tools describe themselves and the task says the rest.
Per turn gray may append ephemeral context: the <available_skills> list
(fresh skill discovery for the turn's directory) — no skill tool, read
matches with bash — plus <project_context>, the nearest AGENTS.md /
CLAUDE.md above the working directory, and plugin docs. Lean mode — the
default — skips all of that: this file alone is the system prompt. Opt
back in with "lean": false or GRAY_LEAN=0. Edit with `/agentsmd`
(Ctrl-S save & apply, Ctrl-R reset to this default, Ctrl-X cancel).
-->
You are Gray, running on the user's machine.
"#;

/// `--bare` system prompt, the whole of it: mini-swe-agent's and dsh minimal's
/// one-line persona, no runtime context. The task says the rest.
pub const BARE_SYS_PROMPT: &str = "You are a helpful assistant that can interact with a computer.";

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
/// the always-on context-only `skills` + project-context plugins (tools stay bash-only),
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
/// Prompt-cache warming for a build, or `None`. Both the native Anthropic
/// Messages API and OpenAI-compatible prefix caches live ~5 min, and a replay
/// runs only when its capped output preserves the request's cache key — the
/// provider's `warm_output_cap` returns 1 everywhere except where the key
/// derives from the cap (Claude thinking budget; past pi `isReplayable`'s
/// carve-out the replay re-derives the same budget). `GRAY_NO_CACHE_WARM=1`
/// turns it off.
///
/// `plugin_warm_replay` is the manifest opt-in (`request.warm_replay`) of
/// the connected plugin provider: a verbatim host replay is only safe for
/// transports the host calls directly (a real HTTPS endpoint), not for
/// relay sidecars that spawn per-turn CLI children. `cache_ttl_secs` is the
/// provider's declared cache lifetime (`request.cache_ttl_secs`, or the
/// 5-minute default).
fn cache_warm_policy(
    config: &Config,
    model: &str,
    plugin_warm_replay: bool,
    cache_ttl_secs: u64,
) -> Option<gray_core::cache_warm::CacheWarmPolicy> {
    // Every built-in provider path caches prefixes (native Anthropic plus
    // all OpenAI-compatible base URLs — direct OpenAI, routers, local
    // servers); plugin-credentialed sidecars are excluded unless their
    // manifest opts into verbatim replay. A second, runtime gate in
    // `keep_warm` still sends zero refreshes unless the model has cache
    // prices.
    let cacheable = !config.uses_plugin_credentials() || plugin_warm_replay;
    if config.bare || !cacheable || std::env::var_os("GRAY_NO_CACHE_WARM").is_some() {
        return None;
    }
    let model = model.to_string();
    Some(gray_core::cache_warm::CacheWarmPolicy {
        ttl: std::time::Duration::from_secs(cache_ttl_secs),
        prices: std::sync::Arc::new(move || {
            let r = crate::setup::get_model_rate(&model)?;
            r.has_cache_prices.then_some(gray_core::cache_warm::Prices {
                input: r.input,
                output: r.output,
                cache_read: r.cache_read,
                cache_write: r.cache_write,
            })
        }),
    })
}

pub async fn build_agent(
    config: &Config,
    cwd: &Path,
    session_id: Option<&str>,
) -> anyhow::Result<gray_core::agent::Agent> {
    let Some(model) = &config.model else {
        anyhow::bail!(
            "no model configured yet — run /connect to set up your provider & key (or /model provider/id; /help for all commands)"
        );
    };
    // Keyless upstreams (free tiers, local servers) run with an empty key.
    let api_key = config.api_key.as_deref().unwrap_or("");
    // Bare: one fixed line, never the user's file (and never create it).
    let body = if config.bare {
        BARE_SYS_PROMPT.to_string()
    } else {
        load_or_create_system_prompt_at(&sys_prompt_path()?)?
    };

    // Plugin-backed connections own their credential through the provider
    // sidecar; a failure here must stop the build, not silently fall back
    // to an unrelated API key.
    let dynamic = if config.uses_plugin_credentials() {
        let home = setup::gray_home()?;
        Some(crate::providers::connect_dynamic_provider(config, &home).await?)
    } else {
        None
    };
    // The wire id composes here while `config.model` keeps the row for
    // display and the picker: a row with declared variants resolves its
    // (effort, fast, parts) selection to the concrete id; other rows send
    // themselves, or their `-fast`/`-priority` sibling in fast mode.
    let wire_model = crate::setup::wire_model_for(config).unwrap_or_else(|| model.clone());
    // A declared variant's effort is the row's level: clamp against the row
    // (whose efforts the catalog declared), not the concrete id.
    let effort_model = if crate::setup::variants::row_shape(model).is_some() {
        model
    } else {
        &wire_model
    };
    let reasoning_effort = config
        .thinking_effort
        .as_deref()
        .map(|effort| crate::setup::clamp_thinking_level(effort_model, effort).to_string());
    let plugin_warm_replay = dynamic
        .as_ref()
        .is_some_and(|p| p.installed().provider.transport.request.warm_replay);
    // A plugin can declare its real cache lifetime (e.g. a relay to a CLI
    // whose cache lives an hour): the tracker's cold-cache detection, the
    // footer warmth timer and miss notices all read it. Reset on every
    // build so a provider switch can't inherit a stale TTL.
    let cache_ttl_secs = dynamic
        .as_ref()
        .and_then(|p| p.installed().provider.transport.request.cache_ttl_secs)
        .unwrap_or(300);
    crate::cache::set_cache_ttl(std::time::Duration::from_secs(cache_ttl_secs));
    let cache_warm = cache_warm_policy(config, &wire_model, plugin_warm_replay, cache_ttl_secs);
    // The model cannot see its picker row or provider from inside the
    // loop; name them (Hermes' volatile prompt section) so it reports the
    // selection the user made — the picked row's label, not the wire id a
    // variant resolves to. `--bare` keeps its one-line prompt.
    let identity = if config.bare {
        String::new()
    } else {
        let provider = dynamic
            .as_ref()
            .map(|p| p.installed().provider.name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| config.provider_id.clone());
        system_prompt::identity_block(
            crate::setup::selected_model_label(config)
                .as_deref()
                .unwrap_or(""),
            model,
            &provider,
        )
    };
    let agent = gray_plugin::builder::build_agent(gray_plugin::builder::BuilderOptions {
        model: wire_model.clone(),
        api_key: api_key.to_string(),
        base_url: config.base_url.clone(),
        reasoning_effort,
        temperature: config.temperature,
        top_p: config.top_p,
        // No window = no in-loop compaction; bare runs to the provider's limit.
        context_window: (!config.bare)
            .then(|| crate::setup::context::resolve_model_context_length(model)),
        session_id: session_id.map(str::to_string),
        cwd: cwd.to_path_buf(),
        // Stored instructions verbatim; no runtime context (cwd etc.) —
        // only the identity block above follows them.
        system_prompt: gray_plugin::builder::SystemPrompt::Build(Box::new(
            move |_registry: &gray_tools::Registry| {
                let mut prompt = system_prompt::build_system_prompt(Some(body));
                if !identity.is_empty() {
                    if !prompt.is_empty() {
                        prompt.push_str("\n\n");
                    }
                    prompt.push_str(&identity);
                }
                prompt
            },
        )),
        // Sidecars get the host runner so plugin-initiated `host/run`
        // / `host/say` don't fall back to loud `{"error":…}`.
        // Bash-only tools; the context-only skills + project-context
        // plugins are always on (every profile, including the default
        // `tools-minimal`). `--lean` skips them: their whole job is
        // injecting prompt context.
        extra_plugins: if config.bare || config.lean {
            Vec::new()
        } else {
            vec![
                Arc::new(crate::skills_tool::SkillsPlugin::default()),
                Arc::new(crate::skills_tool::ProjectContextPlugin),
            ]
        },
        bare: config.bare,
        host_handler: Some(host::default_handler(cwd.to_path_buf())),
        profile_path: "gray.yml".to_string(),
        abort_on_spawn_failure: true,
        wrap_executor: None,
        dynamic_provider_profile: dynamic.as_ref().map(|provider| provider.profile().clone()),
        dynamic_credential_source: dynamic
            .as_ref()
            .map(|provider| provider.credential_source()),
    })
    .await?;
    for w in gray_plugin::builder::take_builder_warnings() {
        profile::queue_profile_warning(w);
    }
    let mut agent = agent.with_compaction_budget(config.context_reserve, config.context_keep);
    // Same rule `cache_warm_policy` applies to warming: on a relay plugin a
    // prefix rewrite throws away the upstream native session, so the
    // cold-cache stale-output mask stays off there entirely.
    agent.set_prefix_rewrite_ok(!config.uses_plugin_credentials() || plugin_warm_replay);
    // Installed sidecar plugins inject their own prompt_context docs too —
    // lean suppresses hook text at the loop, not just the two context-only
    // builtins skipped above.
    agent.set_lean_prompt(config.lean);
    // Bash bounds an explicitly requested timeout at 3600 s (and has no
    // default), so the agent-level timeout must sit above that (P2B
    // requirement): it is a last-resort stop, never a budget.
    Ok(agent
        .with_tool_timeout(crate::shell_drain::SHELL_TOOL_TIMEOUT)
        .with_cache_warm(cache_warm))
}

/// Command-line arguments for the Gray harness.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "gray",
    version,
    about = "gray — a minimal, modular agent harness in Rust.",
    after_help = "Run with no arguments for the interactive REPL. Use -p for one-shot print mode.",
    group(ArgGroup::new("one_shot_input").args(["print", "input_json"]))
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

    /// Read a versioned structured-input envelope from a file or stdin
    #[arg(long, value_name = "PATH", conflicts_with = "print", requires = "json")]
    pub input_json: Option<PathBuf>,

    /// Emit versioned NDJSON progress and a final result instead of terminal output
    #[arg(long, requires = "one_shot_input")]
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

    /// Resume a session: `-r <ID>` works like `--session <ID>`, bare `-r`
    /// opens the same picker as `gray resume`
    #[arg(
        short = 'r',
        long = "resume",
        value_name = "ID",
        num_args = 0..=1,
        conflicts_with = "session",
        conflicts_with = "continue_last"
    )]
    pub resume: Option<Option<String>>,

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

    /// Print a self-describing SKILL.md for driving gray and exit
    #[arg(long = "skill")]
    pub skill: bool,

    /// Bare run, mini-swe-agent shaped: a one-line system prompt and plain
    /// blocking bash (no job control), no compaction. Skips ~/.gray/AGENTS.md,
    /// memory, skills, project AGENTS.md/CLAUDE.md, plugins (gray.yml, installed, pi), cache warming and the update check.
    /// Model/provider config still loads. Env: GRAY_BARE=1.
    #[arg(long)]
    pub bare: bool,

    /// Lean prompt (the default): keep the stored AGENTS.md and every
    /// tool/plugin, but skip all per-turn injected context (the skills
    /// list, project AGENTS.md/CLAUDE.md, plugin docs). Costs close to
    /// --bare per request without losing the full agent. Opt out with
    /// GRAY_LEAN=0 or persisted `"lean": false`.
    #[arg(long)]
    pub lean: bool,

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
    /// Find files by glob, off a resident index when one can answer exactly
    ///
    /// The command form of the `find` tool: the same answer `fd` would give,
    /// served from the in-process index when the index can be exact about it
    /// and from `fd` itself when it cannot. Exists as a command because
    /// searching is a shell thing, and the model already has `fd`, `rg` and
    /// `grep` — the index is a faster way to run the same question, not a
    /// different answer.
    Find {
        /// Glob, e.g. `*.rs` or `crates/**/mod.rs`
        #[arg(value_name = "PATTERN")]
        pattern: String,
        /// Directory to search (default: the current directory)
        #[arg(value_name = "PATH")]
        path: Option<String>,
        /// Max results (default 100)
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },
    /// Grep file contents, off a resident index when one can answer exactly
    ///
    /// The command form of the `grep` tool, with the same decline rules: a
    /// non-git directory, `ignoreCase`, a negated or depth-anchored `glob`, an
    /// invalid pattern, or a file target all fall through to `rg`.
    Grep {
        /// Pattern (regex unless --literal)
        #[arg(value_name = "PATTERN")]
        pattern: String,
        /// File or directory to search (default: the current directory)
        #[arg(value_name = "PATH")]
        path: Option<String>,
        /// Max matches (default 100)
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        /// Basename glob filter, e.g. `*.rs`
        #[arg(long, value_name = "GLOB")]
        glob: Option<String>,
        /// Case-insensitive (served by rg: the index declines it)
        #[arg(long, short)]
        ignore_case: bool,
        /// Treat the pattern as literal text, not a regex
        #[arg(long, short = 'F')]
        literal: bool,
        /// Lines of context around each match
        #[arg(long, value_name = "N")]
        context: Option<usize>,
    },
    /// Resume a previous conversation
    Resume {
        /// Session id (three-word name or UUID, or a prefix of either). If omitted, shows picker unless --last.
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
    /// Diagnose this setup (pass --online to also reach the provider)
    Doctor {
        /// Also make one request to the provider (no tokens, just /models)
        #[arg(long)]
        online: bool,
    },
    /// Session store maintenance
    Sessions {
        #[command(subcommand)]
        cmd: SessionsCmd,
    },
    /// Read back a tool result that was too large for context
    ///
    /// A tool result over the inline budget ends in `[spilled …]` with a
    /// handle. This reads it back: `gray spill grep <handle> <pattern>` is
    /// the one that matters, and the ends are one flag away. A subcommand and
    /// not a tool — the model has a shell, and a tool entry is schema every
    /// turn pays for.
    Spill {
        #[command(subcommand)]
        cmd: SpillCmd,
    },
    /// Plugin-provided commands (`gray NAME setup`, …) — forwarded to the plugin
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// `gray spill ...` — read back a spilled tool result.
#[derive(Parser, Debug, Clone)]
pub enum SpillCmd {
    /// First N lines (default 50)
    Head {
        /// Handle from the `[spilled …]` footer
        #[arg(value_name = "HANDLE")]
        handle: String,
        /// How many lines
        #[arg(long, short = 'n', default_value_t = 50)]
        lines: usize,
    },
    /// Last N lines (default 50)
    Tail {
        /// Handle from the `[spilled …]` footer
        #[arg(value_name = "HANDLE")]
        handle: String,
        /// How many lines
        #[arg(long, short = 'n', default_value_t = 50)]
        lines: usize,
    },
    /// Matching lines, numbered against the original
    Grep {
        /// Handle from the `[spilled …]` footer
        #[arg(value_name = "HANDLE")]
        handle: String,
        /// Pattern (regex unless --literal)
        #[arg(value_name = "PATTERN")]
        pattern: String,
        /// Lines of context around each match
        #[arg(long, short = 'n', default_value_t = 0)]
        context: usize,
        /// Case-insensitive
        #[arg(long, short)]
        ignore_case: bool,
        /// Treat the pattern as literal text, not a regex
        #[arg(long, short = 'F')]
        literal: bool,
        /// Max matching lines to print (default 200)
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
    },
    /// What compression saved on this machine, by rule
    Stats,
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
    #[command(
        after_help = "Reminder (one command, no model turn):\n  gray cron add \"in 2m\" \"clean my roo\" --reminder\nThe text is stored and delivered verbatim: never reword it, never fix typos, never pass --name."
    )]
    Add {
        /// Schedule expression
        schedule: String,
        /// Prompt the daemon runs at fire time
        prompt: String,
        /// Delivery target. Default inside a gray chat: back into that chat — the result arrives as a new message and the agent gets a turn (no need to sleep or poll). local: only saves a file under cron/output, nobody is told; origin: to --origin-session (anything else saves only)
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
        /// Reminder: store the prompt verbatim and deliver it as-is at fire
        /// time. No model turn, no tools. Use for "remind me ...".
        #[arg(long)]
        reminder: bool,
    },
    /// One claim→fire→record pass (also the OS-cron/runit entry point)
    Tick {
        /// Emit one `cron_delivery` JSON line per chat-bound fire (for a host that routes them)
        #[arg(long)]
        json: bool,
    },
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
    /// Connect a chat platform: installs its app if needed, then runs its
    /// setup wizard (`gray gateway setup discord`)
    Setup {
        /// Platform app to set up; asked when several are installed
        platform: Option<String>,
    },
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
    /// Turn the gateway master switch on (run/start allowed again)
    On,
    /// Turn the gateway master switch off (run/start refuse until re-enabled)
    Off,
    /// Restart/shutdown notices for a chat adapter's daemon (JSON answers)
    #[command(subcommand)]
    Lifecycle(LifecycleCmd),
}

/// `gray gateway lifecycle ...`: the platform-agnostic restart record an
/// adapter keeps in its own state dir. Every answer is one JSON object.
#[derive(Parser, Debug, Clone)]
pub enum LifecycleCmd {
    /// Daemon starting: say how the last run ended and what to announce
    Boot {
        /// The adapter's state dir (holds lifecycle.json)
        #[arg(long)]
        dir: std::path::PathBuf,
        /// Turns that were still running when the last run ended
        #[arg(long, default_value_t = 0)]
        interrupted: usize,
    },
    /// Daemon exiting cleanly: record it and hand back the notices
    Stop {
        /// The adapter's state dir
        #[arg(long)]
        dir: std::path::PathBuf,
    },
    /// Mark the next stop as a restart (run before signalling the daemon)
    Restart {
        /// The adapter's state dir
        #[arg(long)]
        dir: std::path::PathBuf,
    },
    /// A plain stop is coming: drop any stale restart marker
    ClearRestart {
        /// The adapter's state dir
        #[arg(long)]
        dir: std::path::PathBuf,
    },
}

/// `gray plugin ...` — plugin-side tooling.
#[derive(Parser, Debug, Clone)]
pub enum PluginCmd {
    /// List installed plugins
    List,
    /// Install a plugin: index name, https tarball URL, or local executable path
    Install {
        /// Index name, https tarball URL, or executable path
        spec: String,
        /// Accept a caution scan verdict (never overrides `dangerous`)
        #[arg(short, long)]
        force: bool,
        /// Skip the confirmation prompt for unverified registry plugins
        /// (the warning still prints)
        #[arg(short = 'y', long)]
        yes: bool,
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
    /// Show declared vs granted plugin capabilities
    Capabilities {
        /// Plugin name (default: every installed plugin)
        name: Option<String>,
    },
}

#[path = "lib_tests.rs"]
#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "input_json_tests.rs"]
mod input_json_tests;
