pub mod ask;
pub mod auth;
pub mod auto_title;
pub mod auto_update;
pub mod cache;
pub mod cmd;
pub mod compact;
pub mod composer;
pub mod config;
pub mod cron;
pub(crate) mod exe_watch;
pub(crate) mod failed_turn;
pub mod feedback;
pub(crate) mod gateway_pid;
pub mod host;
pub mod ide;
pub mod keymap;
pub mod logging;
pub(crate) mod mascot;
pub mod plugin_check;
pub mod plugin_cli;
pub(crate) mod plugin_status;
pub mod print;
mod print_meter;
pub mod profile;
pub mod project_trust;
pub mod prompt_templates;
pub mod providers;
pub mod quality_of_life;
pub mod repl;
pub mod resume;
mod rotation;
pub mod search;
pub mod session_mailbox;
pub mod session_read;
pub mod session_send;
pub mod session_store;
pub mod setup;
pub mod shell_drain;
pub mod skills;
pub mod skills_tool;
pub mod spill;
pub mod statusline;
pub mod sys_editor;
pub mod system_prompt;
pub mod term_keys;
pub(crate) mod text_width;
pub mod theme;
pub mod tool_fmt;
pub mod tui;
pub mod turn_caps;
pub mod update;
pub(crate) mod util;

use clap::{ArgGroup, Parser};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use config::Config;
pub use print::run_print_mode;
pub use profile::{build_registry, take_profile_warnings};
pub use repl::{ReplCommand, parse_command, run_repl_mode};
pub use tui::{clear_screen, print_wrapped};

pub const LEGACY_SYS_PROMPT: &str = r#"<!--
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

pub const DEFAULT_SYS_PROMPT: &str = r#"<!--
Gray generated this file and refreshes it when the shipped default changes —
but only while it stays byte-identical to a default it wrote (the managed
stamp on line 1 is how it knows). Edit anything, even whitespace, and it
becomes yours forever. `/agentsmd` Ctrl-R restores the default and its stamp.

This file IS the stored system prompt. Comments like this one stay in the
file but are stripped before the prompt reaches the model. After it, gray
appends a small runtime block: identity, harness facts, and your working
directory / repo / branch. Per turn it may also inject ephemeral context —
<available_skills> for the turn's directory, <project_context> from the
nearest AGENTS.md / CLAUDE.md above cwd, plugin docs. Lean mode (default)
skips those; opt in with "lean": false or GRAY_LEAN=0.

Edit with `/agentsmd` (Ctrl-S save & apply, Ctrl-R reset to default,
Ctrl-X cancel).
-->
You are Gray, running on the user's machine.
"#;

const MANAGED_STAMP_PREFIX: &str = "<!-- gray:managed sha256:";

fn managed_stamp(body: &str) -> String {
    use sha2::Digest;
    let digest = format!("{:x}", sha2::Sha256::digest(body.as_bytes()));
    format!("{MANAGED_STAMP_PREFIX}{} -->", &digest[..16])
}

pub fn stamped_system_prompt(body: &str) -> String {
    format!("{}\n{}", managed_stamp(body), body)
}

fn managed_body(file: &str) -> Option<&str> {
    let (first, rest) = file.split_once('\n')?;
    (first == managed_stamp(rest)).then_some(rest)
}

pub const BARE_SYS_PROMPT: &str = "You are a helpful assistant that can interact with a computer.";

pub fn sys_prompt_path() -> anyhow::Result<PathBuf> {
    Ok(crate::setup::gray_home()?.join("AGENTS.md"))
}

pub fn load_or_create_system_prompt_at(path: &Path) -> anyhow::Result<String> {
    if let Ok(file) = std::fs::read_to_string(path) {
        if let Some(body) = managed_body(&file) {
            if body != DEFAULT_SYS_PROMPT
                && std::fs::write(path, stamped_system_prompt(DEFAULT_SYS_PROMPT)).is_ok()
            {
                return Ok(DEFAULT_SYS_PROMPT.to_string());
            }
            return Ok(body.to_string());
        }
        if file == DEFAULT_SYS_PROMPT || file == LEGACY_SYS_PROMPT {
            let _ = std::fs::write(path, stamped_system_prompt(&file));
        }
        return Ok(file);
    }

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
            std::fs::write(path, stamped_system_prompt(DEFAULT_SYS_PROMPT))?;
            Ok(DEFAULT_SYS_PROMPT.to_string())
        }
        Err(e) => Err(e.into()),
    }
}

pub fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .ok()
        .filter(|&w| w >= 20)
        .unwrap_or(80)
}

pub fn rule(label: &str) -> String {
    let prefix = format!("\u{2500}\u{2500} {label} ");
    let used = prefix.chars().count();
    let fill = term_width().saturating_sub(used);
    format!("{prefix}{}", "\u{2500}".repeat(fill))
}

pub use gray_plugin::builder::{
    PROMPT_CACHE_KEY_MAX_LENGTH, clamp_prompt_cache_key, provider_cache_key,
};

fn cache_warm_policy(
    config: &Config,
    model: &str,
    plugin_warm_replay: bool,
    cache_ttl_secs: u64,
) -> Option<gray_core::cache_warm::CacheWarmPolicy> {
    let cacheable = !config.uses_plugin_credentials() || plugin_warm_replay;
    if !cacheable {
        log::info!(target: "gray_agent",
            "cache warm disabled: provider is plugin-credentialed without request.warm_replay");
    }
    if config.uses_plugin_credentials() && plugin_warm_replay {
        log::info!(target: "gray_agent", "cache warm enabled: plugin declares request.warm_replay");
    }
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
    build_agent_with(config, cwd, session_id, true).await
}

pub fn set_active_model(config: &Config) {
    gray_plugin::set_active_model(serde_json::json!({
        "id": config.model,
        "parts": config.model_parts,
        "provider_id": config.provider_id,
        "base_url": config.base_url,
    }));
}

pub async fn build_model_agent(
    config: &Config,
    cwd: &Path,
    session_id: Option<&str>,
) -> anyhow::Result<gray_core::agent::Agent> {
    set_active_model(config);
    build_agent_with(config, cwd, session_id, false).await
}

async fn build_agent_with(
    config: &Config,
    cwd: &Path,
    session_id: Option<&str>,
    spawn_plugins: bool,
) -> anyhow::Result<gray_core::agent::Agent> {
    set_active_model(config);
    let Some(model) = &config.model else {
        anyhow::bail!(
            "no model configured yet — set one with the models picker (TUI), --model/--api-key, or GRAY_MODEL/GRAY_API_KEY"
        );
    };

    let api_key = config.api_key.as_deref().unwrap_or("");

    let body = if config.bare {
        BARE_SYS_PROMPT.to_string()
    } else {
        load_or_create_system_prompt_at(&sys_prompt_path()?)?
    };

    let build_started = std::time::Instant::now();
    let dynamic = if config.uses_plugin_credentials() {
        let home = setup::gray_home()?;
        Some(crate::providers::connect_dynamic_provider(config, &home).await?)
    } else {
        None
    };
    log::debug!(target: "gray_timing", "build_agent provider_connect elapsed_ms={}", build_started.elapsed().as_millis());

    let wire_model = crate::setup::wire_model_for(config).unwrap_or_else(|| model.clone());

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

    let cache_ttl_secs = dynamic
        .as_ref()
        .and_then(|p| p.installed().provider.transport.request.cache_ttl_secs)
        .unwrap_or(300);
    crate::cache::set_cache_ttl(std::time::Duration::from_secs(cache_ttl_secs));
    let cache_warm = cache_warm_policy(config, &wire_model, plugin_warm_replay, cache_ttl_secs);

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
    let prebooted_plugins = if !config.bare && spawn_plugins {
        match crate::plugin_status::take_boot().await {
            Some(Ok(plugins)) => Some(plugins),
            Some(Err(e)) => return Err(anyhow::anyhow!(e)),
            None => None,
        }
    } else {
        None
    };
    let config_bare = config.bare;
    let agent = gray_plugin::builder::build_agent(gray_plugin::builder::BuilderOptions {
        model: wire_model.clone(),
        api_key: api_key.to_string(),
        base_url: config.base_url.clone(),
        reasoning_effort,
        temperature: config.temperature,
        top_p: config.top_p,

        context_window: (!config.bare)
            .then(|| crate::setup::context::resolve_model_context_length(model)),
        session_id: session_id.map(str::to_string),
        cwd: cwd.to_path_buf(),

        system_prompt: gray_plugin::builder::SystemPrompt::Build({
            let session_cwd = cwd.to_path_buf();
            Box::new(move |_registry: &gray_tools::Registry| {
                let mut prompt = system_prompt::build_system_prompt(Some(body));
                if !identity.is_empty() {
                    if !prompt.is_empty() {
                        prompt.push_str("\n\n");
                    }
                    prompt.push_str(&identity);
                }

                if !config_bare {
                    prompt.push_str("\n\n");
                    prompt.push_str(&system_prompt::harness_facts(Some(
                        std::time::Duration::from_secs(cache_ttl_secs),
                    )));
                    prompt.push_str("\n\n");
                    prompt.push_str(&system_prompt::session_card(&session_cwd));
                }

                prompt
            })
        }),

        extra_plugins: extra_surface_plugins(config, spawn_plugins),

        bare: config.bare || !spawn_plugins,
        host_handler: Some(host::default_handler(cwd.to_path_buf())),
        profile_path: "gray.yml".to_string(),
        abort_on_spawn_failure: true,
        wrap_executor: None,
        dynamic_provider_profile: dynamic.as_ref().map(|provider| provider.profile().clone()),
        dynamic_credential_source: dynamic
            .as_ref()
            .map(|provider| provider.credential_source()),
        prebooted_plugins,
    })
    .await?;
    tokio::spawn(gray_plugin::refresh_dynamic_tools());
    log::debug!(target: "gray_timing", "build_agent plugins+agent elapsed_ms={}", build_started.elapsed().as_millis());
    for w in gray_plugin::builder::take_builder_warnings() {
        profile::queue_profile_warning(w);
    }
    if let Some(t) = crate::host::registered_tui()
        && let Ok(mut t) = t.lock()
    {
        t.set_plugin_stats(crate::plugin_status::stats_line());
        if !t.modal_open {
            let _ = t.draw();
        }
    }

    let window = crate::setup::context::resolve_model_context_length(model);
    let (ceiling, price_cliff) = crate::compact::ceiling_for(
        config.compact_at,
        window,
        crate::setup::context::user_reserve_tokens_for(window),
        crate::setup::model_price_cliff(model).map(|c| c as usize),
    );
    let mut agent = agent
        .with_compaction_budget(config.context_reserve, config.context_keep)
        .with_compact_ceiling(ceiling, price_cliff);

    agent.set_prefix_rewrite_ok(!config.uses_plugin_credentials() || plugin_warm_replay);

    agent.set_lean_prompt(config.lean);

    Ok(agent
        .with_tool_timeout(crate::shell_drain::SHELL_TOOL_TIMEOUT)
        .with_cache_warm(cache_warm))
}

pub(crate) fn extra_surface_plugins(
    config: &Config,
    spawn_plugins: bool,
) -> Vec<Arc<dyn gray_plugin::Plugin>> {
    let mut extra: Vec<Arc<dyn gray_plugin::Plugin>> = Vec::new();
    if !(config.bare || config.lean || !spawn_plugins) {
        extra.push(Arc::new(crate::skills_tool::SkillsPlugin::default()));
        extra.push(Arc::new(crate::skills_tool::ProjectContextPlugin));
    }
    extra
}

#[derive(Parser, Debug, Clone)]
#[command(
    name = "gray",
    version,
    about = "gray — a minimal, modular agent harness in Rust.",
    after_help = "Run with no arguments for the interactive REPL. Use -p for one-shot print mode.",
    group(ArgGroup::new("one_shot_input").args(["print", "input_json"]))
)]
pub struct Cli {
    #[arg(
        long,
        help = "Model id to use (env GRAY_MODEL; overrides the saved config)"
    )]
    pub model: Option<String>,

    #[arg(
        long,
        value_name = "K=V,...",
        help = "Composite model slot selection, e.g. lead=m1,sidekick=m2"
    )]
    pub model_parts: Option<String>,

    #[arg(long, help = "Provider base URL (env GRAY_BASE_URL)")]
    pub base_url: Option<String>,

    #[arg(
        short = 'p',
        long = "print",
        value_name = "PROMPT",
        help = "One-shot print mode: answer PROMPT ('-' reads stdin) and exit"
    )]
    pub print: Option<String>,

    #[arg(
        long,
        value_name = "PATH",
        conflicts_with = "print",
        requires = "json",
        help = "Read a structured input envelope from PATH ('-' = stdin); implies one-shot"
    )]
    pub input_json: Option<PathBuf>,

    #[arg(
        long,
        requires = "one_shot_input",
        help = "Emit machine-readable JSONL instead of rendered text"
    )]
    pub json: bool,

    /// Cap on model requests; 0 = unlimited (default 32 when omitted)
    #[arg(long, requires = "json", value_parser = clap::value_parser!(u32))]
    pub max_requests: Option<u32>,

    #[arg(
        long,
        requires = "json",
        value_name = "USD",
        help = "USD per 1M input tokens, for cost accounting"
    )]
    pub input_price: Option<f64>,

    #[arg(
        long,
        requires = "json",
        value_name = "USD",
        help = "USD per 1M output tokens, for cost accounting"
    )]
    pub output_price: Option<f64>,

    #[arg(long, help = "Provider API key (env GRAY_API_KEY)")]
    pub api_key: Option<String>,

    #[arg(
        short = 'c',
        long = "continue",
        help = "Continue the most recent session in this directory"
    )]
    pub continue_last: bool,

    #[arg(long, value_name = "ID", help = "Attach to a session by id or prefix")]
    pub session: Option<String>,

    #[arg(
        short = 'r',
        long = "resume",
        value_name = "ID",
        num_args = 0..=1,
        conflicts_with = "session",
        conflicts_with = "continue_last",
        help = "Resume a session: pick interactively, or pass an id/prefix"
    )]
    pub resume: Option<Option<String>>,

    #[arg(long, value_name = "TOKENS", value_parser = parse_context_window_cli, help = "Override the model context window, e.g. 128000, 128k, 1m")]
    pub context_window: Option<usize>,

    #[arg(long, value_name = "TOKENS", value_parser = parse_context_window_cli, help = "Tokens reserved for the reply when compacting context")]
    pub context_reserve: Option<usize>,

    #[arg(long, value_name = "TOKENS", value_parser = parse_context_window_cli, help = "Tokens kept verbatim at the tail during compaction")]
    pub context_keep: Option<usize>,

    #[arg(long, value_name = "auto|TOKENS", value_parser = parse_compact_at_cli, help = "Compact once context passes this many tokens, or 'auto'")]
    pub compact_at: Option<crate::setup::CompactAt>,

    #[arg(
        long = "dump-manifest",
        help = "Print the resolved gray.yml tool manifests as JSON and exit"
    )]
    pub dump_manifest: bool,

    #[arg(
        long = "skill",
        help = "Print the gray agent-skill doc (for embedding in other agents) and exit"
    )]
    pub skill: bool,

    #[arg(
        long,
        help = "Bare mode: no plugins, memory, background jobs or auto-compact"
    )]
    pub bare: bool,

    #[arg(long, help = "Lean system prompt: skip plugin-provided prompt extras")]
    pub lean: bool,

    #[arg(long, value_name = "N", help = "Stop the turn after N agent steps")]
    pub max_turns: Option<u32>,

    #[arg(
        long,
        value_name = "USD",
        help = "Stop the turn when estimated spend exceeds USD"
    )]
    pub max_cost_usd: Option<f64>,

    #[arg(
        long,
        value_name = "SECS",
        help = "Stop the turn after SECS wall-clock seconds"
    )]
    pub max_wall_secs: Option<u64>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

fn parse_context_window_cli(s: &str) -> Result<usize, String> {
    crate::setup::parse_context_window(s)
        .ok_or_else(|| format!("invalid context window '{s}' — use e.g. 128000, 128k, 1m"))
}

fn parse_compact_at_cli(s: &str) -> Result<crate::setup::CompactAt, String> {
    crate::setup::parse_compact_at(s).ok_or_else(|| {
        format!("invalid --compact-at '{s}' — use \"auto\" or e.g. 90000, 90k, 1.5m")
    })
}

#[derive(Parser, Debug, Clone)]
pub enum Commands {
    #[command(about = "Find files by name pattern under PATH (defaults to .)")]
    Find {
        #[arg(value_name = "PATTERN", help = "File-name pattern, e.g. '*.rs'")]
        pattern: String,

        #[arg(
            value_name = "PATH",
            help = "Directory to search (default: current directory)"
        )]
        path: Option<String>,

        #[arg(long, value_name = "N", help = "Cap the number of results")]
        limit: Option<usize>,
    },

    #[command(about = "Search file contents — regex by default (-i ignore-case, -F literal)")]
    Grep {
        #[arg(
            value_name = "PATTERN",
            help = "Regex pattern (or literal text with -F)"
        )]
        pattern: String,

        #[arg(
            value_name = "PATH",
            help = "File or directory to search (default: current directory)"
        )]
        path: Option<String>,

        #[arg(long, value_name = "N", help = "Cap the number of matches")]
        limit: Option<usize>,

        #[arg(
            long,
            value_name = "GLOB",
            help = "Only search files matching GLOB, e.g. '*.toml'"
        )]
        glob: Option<String>,

        #[arg(long, short, help = "Case-insensitive matching")]
        ignore_case: bool,

        #[arg(long, short = 'F', help = "Treat PATTERN as literal text, not regex")]
        literal: bool,

        #[arg(
            long,
            value_name = "N",
            help = "Print N lines of context around each match"
        )]
        context: Option<usize>,
    },

    #[command(about = "Resume a past session — picker by default, or pass an id / --last")]
    Resume {
        #[arg(value_name = "SESSION_ID", help = "Session id or unique prefix")]
        session_id: Option<String>,

        #[arg(long, help = "Resume the most recent session")]
        last: bool,

        #[arg(long, help = "Include sessions from other directories")]
        all: bool,
    },

    #[command(
        about = "Read a session's recent conversation, or list live sessions",
        after_help = "Examples:\n  gray read --live            live sessions: id, pid, cwd, model, last activity\n  gray read <SESSION_ID>      last 20 messages, tool calls summarized\n  gray read <SESSION_ID> --last 50 --json\nReads the owner's session store ($GRAY_USER_HOME, else $GRAY_HOME). A session is live only while its .open lock is held."
    )]
    Read {
        #[arg(value_name = "SESSION_ID", help = "Session id or unique prefix")]
        session: Option<String>,

        #[arg(long, help = "List live sessions instead of reading one")]
        live: bool,

        #[arg(
            long,
            value_name = "N",
            default_value_t = 20,
            help = "Show the last N messages"
        )]
        last: usize,

        #[arg(long, help = "Emit JSON instead of rendered text")]
        json: bool,
    },

    #[command(about = "Send a message (and attachments) into a live session")]
    Send {
        #[arg(value_name = "SESSION_ID", help = "Session id or unique prefix")]
        session: String,

        #[arg(value_name = "TEXT", default_value = "", help = "Message text")]
        text: String,

        #[arg(
            long = "attach",
            value_name = "PATH",
            help = "Attach a file (repeatable)"
        )]
        attach: Vec<std::path::PathBuf>,

        #[arg(long, value_name = "LABEL", help = "Sender label shown in the session")]
        from: Option<String>,

        #[arg(long, value_name = "SECS", help = "Wait up to SECS for the reply")]
        wait: Option<u64>,
    },

    #[command(
        visible_alias = "plugins",
        about = "Manage plugins (install, update, enable)"
    )]
    Plugin {
        #[command(subcommand)]
        cmd: PluginCmd,
    },

    #[command(
        disable_help_flag = true,
        about = "The always-on gateway daemon: chat apps, cron, scheduling (gateway plugin)"
    )]
    Gateway {
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "ARGS",
            help = "Forwarded to the installed gateway plugin"
        )]
        args: Vec<String>,
    },

    #[command(about = "Trust a directory's .gray/prompts, SKILL.md and AGENTS.md files")]
    Trust {
        #[arg(
            value_name = "DIR",
            help = "Directory to trust (default: current directory)"
        )]
        dir: Option<std::path::PathBuf>,

        #[arg(long, help = "Revoke trust for DIR")]
        revoke: bool,
    },

    #[command(visible_alias = "upgrade", about = "Install a newer gray build")]
    Update {
        #[arg(
            long,
            value_name = "on|off|status",
            help = "Turn auto-update on/off, or show its state"
        )]
        auto: Option<String>,

        #[arg(long, help = "Also update plugins and provider CLIs")]
        everything: bool,
    },

    #[command(about = "Manage saved sessions")]
    Sessions {
        #[command(subcommand)]
        cmd: SessionsCmd,
    },

    #[command(
        about = "Machine-readable command registry — list and run commands (used by gateways)"
    )]
    Cmd {
        #[command(subcommand)]
        cmd: CmdVerb,
    },

    #[command(about = "Read spilled tool output that overflowed the transcript")]
    Spill {
        #[command(subcommand)]
        cmd: SpillCmd,
    },

    #[cfg(target_os = "linux")]
    #[command(hide = true)]
    SearchServe {
        #[arg(long, value_name = "DIR")]
        root: std::path::PathBuf,

        #[arg(long, value_name = "PATH")]
        sock: std::path::PathBuf,

        #[arg(long, value_name = "DIR")]
        index_dir: Option<std::path::PathBuf>,
    },

    #[command(
        about = "Install a catalog plugin or register a native executable (gray install plugin NAME)"
    )]
    Install {
        #[command(subcommand)]
        cmd: InstallCmd,
    },

    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Parser, Debug, Clone)]
pub enum SpillCmd {
    #[command(about = "Show the first N lines of a spill")]
    Head {
        #[arg(value_name = "HANDLE", help = "Spill handle, e.g. spill:abc123")]
        handle: String,

        #[arg(long, short = 'n', default_value_t = 50, help = "Number of lines")]
        lines: usize,
    },

    #[command(about = "Show the last N lines of a spill")]
    Tail {
        #[arg(value_name = "HANDLE", help = "Spill handle, e.g. spill:abc123")]
        handle: String,

        #[arg(long, short = 'n', default_value_t = 50, help = "Number of lines")]
        lines: usize,
    },

    #[command(about = "Search inside a spill")]
    Grep {
        #[arg(value_name = "HANDLE", help = "Spill handle, e.g. spill:abc123")]
        handle: String,

        #[arg(value_name = "PATTERN", help = "Regex pattern (literal with -F)")]
        pattern: String,

        #[arg(
            long,
            short = 'n',
            default_value_t = 0,
            help = "Context lines around each match"
        )]
        context: usize,

        #[arg(long, short, help = "Case-insensitive matching")]
        ignore_case: bool,

        #[arg(long, short = 'F', help = "Treat PATTERN as literal text, not regex")]
        literal: bool,

        #[arg(long, value_name = "N", help = "Cap the number of matches")]
        limit: Option<usize>,
    },

    #[command(about = "Show spill store stats")]
    Stats,
}

#[derive(Parser, Debug, Clone)]
pub enum SessionsCmd {
    #[command(about = "Delete saved sessions older than N days")]
    Prune {
        #[arg(long, default_value_t = 90, help = "Age threshold in days")]
        older_than_days: u64,
    },
}

#[derive(Parser, Debug, Clone)]
pub enum CmdVerb {
    #[command(about = "List the command table as JSON")]
    List,

    #[command(about = "Run a command by name")]
    Run {
        #[arg(value_name = "NAME", help = "Command name, e.g. 'usage', 'compact'")]
        name: String,

        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "ARGS",
            help = "Arguments passed through to the command"
        )]
        args: Vec<String>,
    },

    #[command(about = "Answer a pending command interaction (option/select prompts)")]
    Interact {
        #[arg(
            value_name = "TOKEN",
            help = "Interaction token from the pending prompt"
        )]
        token: String,

        #[arg(
            value_name = "ACTION",
            help = "Chosen action; omit to re-show the prompt"
        )]
        action: Option<String>,
    },
}

#[derive(Parser, Debug, Clone)]
pub enum PluginCmd {
    #[command(about = "List installed plugins")]
    List,

    #[command(about = "Install plugins by index name, https URL, or local path")]
    Install {
        #[arg(
            value_name = "SPEC",
            num_args = 1..,
            help = "Plugin names from the index, https URLs, or local dirs/files"
        )]
        specs: Vec<String>,

        #[arg(short, long, help = "Reinstall even if already present")]
        force: bool,

        #[arg(short = 'y', long, help = "Skip the capability/trust prompt")]
        yes: bool,
    },

    #[command(about = "Remove an installed plugin")]
    Remove {
        #[arg(value_name = "NAME")]
        name: String,
    },

    #[command(about = "Update plugins — a name or 'all'")]
    Update {
        #[arg(default_value = "all", value_name = "NAME|all")]
        target: String,

        #[arg(long, help = "Only report what would update")]
        check: bool,
    },

    #[command(about = "Enable a disabled plugin")]
    Enable {
        #[arg(value_name = "NAME")]
        name: String,

        #[arg(long, help = "Load it in every GRAY_HOME (e.g. memory everywhere)")]
        global: bool,
    },

    #[command(about = "Disable a plugin without removing it")]
    Disable {
        #[arg(value_name = "NAME")]
        name: String,
    },

    #[command(about = "Run conformance checks on a plugin directory (for authors)")]
    Check {
        #[arg(value_name = "DIR")]
        dir: String,
    },

    #[command(about = "Show a plugin's declared capabilities; --all grants them")]
    Capabilities {
        #[arg(value_name = "NAME")]
        name: Option<String>,

        #[arg(long, help = "Grant every declared capability (needs NAME)")]
        all: bool,
    },
}

#[derive(Parser, Debug, Clone)]
pub enum InstallCmd {
    #[command(about = "Register a plugin command from the catalog, PATH, or GRAY_PLUGIN_PATH")]
    Plugin {
        #[arg(value_name = "NAME")]
        name: String,
    },
}

#[path = "lib_tests.rs"]
#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "input_json_tests.rs"]
mod input_json_tests;
