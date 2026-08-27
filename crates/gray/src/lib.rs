//! Gray: a minimal, modular agent harness in Rust.

pub mod composer;
pub mod config;
pub mod logo_data;
pub mod logging;
pub mod oauth;
pub mod print;
pub mod prompt_templates;
pub mod setup;
pub mod skills;
pub mod sys_editor;
pub mod repl;
pub mod streaming;
pub mod system_prompt;
pub mod tui;

use std::path::{Path, PathBuf};
use clap::Parser;

pub use config::Config;
pub use print::run_print_mode;
pub use repl::{parse_command, run_repl_mode, ReplCommand};
pub use tui::{clear_screen, print_wrapped};

use gray_core::agent::Agent;
use gray_provider::OpenAiProvider;
use gray_tools::Registry;

/// Default system prompt, shipped as markdown and materialized to `~/.gray/sys.md`
/// on first run. Edit that file (or use the `/sys` command) to change it.
pub const DEFAULT_SYS_PROMPT: &str = include_str!("../assets/SYS.md");

/// Resolves the user's system-prompt file path (`$GRAY_HOME` or `$HOME/.gray`) + `sys.md`.
pub fn sys_prompt_path() -> anyhow::Result<PathBuf> {
    let base = std::env::var("GRAY_HOME")
        .or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray")))
        .map_err(|_| anyhow::anyhow!("cannot resolve home: set HOME or GRAY_HOME"))?;
    Ok(PathBuf::from(base).join("sys.md"))
}

/// Loads the system prompt from `path`, writing the embedded default there first if absent.
pub fn load_or_create_system_prompt_at(path: &Path) -> anyhow::Result<String> {
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

pub fn format_system_prompt(body: &str, cwd: &Path) -> String {
    format!("{}\n\nCurrent working directory: {}", body.trim_end(), cwd.display())
}

/// Builds an [`Agent`] instance wired with the OpenAI provider, builtin tools, and system prompt.
///
/// Skills are discovered via [`skills::discover_skills`] (global `~/.gray/skills` +
/// `~/.pi/agent/skills` compat + project `cwd/.gray/skills` / `cwd/.pi/skills`
/// walked up to git root) respecting `.gitignore`/`.ignore`/`.fdignore`, and
/// `AGENTS.md`/`CLAUDE.md` context files are discovered walking up to git root
/// and appended as `<project_context>` blocks. Skills are only surfaced when the
/// `read` tool is present.
///
/// Errors here are user-configuration problems (missing model or API key), so the
/// message is written for a human, not a log file.
pub fn build_agent(config: &Config, cwd: &Path) -> anyhow::Result<Agent> {
    let Some(model) = &config.model else {
        anyhow::bail!(
            "no model configured yet — run /provider (or set --model <provider/model>), then try again"
        );
    };
    // Keyless upstreams (free tiers, local servers) run with an empty key.
    let api_key = config.api_key.as_deref().unwrap_or("");
    let body = load_or_create_system_prompt_at(&sys_prompt_path()?)?;

    // Discover skills + AGENTS.md / CLAUDE.md context (literal port of pi discovery)
    let discovered = skills::discover_skills(cwd);
    let context_files = system_prompt::discover_context_files(cwd);

    // Tools only appear in the prompt when they have a snippet — literal port of
    // `visibleTools = tools.filter(name => !!toolSnippets[name])`.
    let tmp_registry = Registry::builtin();
    let tool_snippets = tmp_registry.prompt_snippets();
    let selected_tools = tmp_registry.tool_names();
    let prompt_guidelines = {
        let g = tmp_registry.prompt_guidelines();
        if g.is_empty() { None } else { Some(g) }
    };

    let system_prompt = system_prompt::build_system_prompt(system_prompt::BuildSystemPromptOptions {
        custom_prompt: Some(body),
        selected_tools: Some(selected_tools),
        tool_snippets: Some(tool_snippets),
        prompt_guidelines,
        append_system_prompt: None,
        cwd: cwd.to_path_buf(),
        context_files: Some(context_files),
        skills: Some(discovered.skills),
    });

    let provider = OpenAiProvider::builder(api_key, model)
        .base_url(&config.base_url)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to initialize OpenAI provider: {e}"))?;

    let registry = Registry::builtin();
    let tool_defs = registry.defs();

    let agent = Agent::new(Box::new(provider), Box::new(registry))
        .with_system(system_prompt)
        .with_tools(tool_defs);

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_system_prompt_appends_cwd() {
        let cwd = Path::new("/workspace/test-dir");
        let formatted = format_system_prompt(DEFAULT_SYS_PROMPT, cwd);

        assert!(formatted.starts_with("You are gray, a minimal agent"));
        assert!(formatted.contains("Current working directory: /workspace/test-dir"));
    }

    #[test]
    fn rule_embeds_label_and_is_wide() {
        let r = rule("tool");
        assert!(r.starts_with("\u{2500}\u{2500} tool "));
        assert!(r.chars().count() >= crate::term_width() - 1);
        assert!(r.ends_with('\u{2500}'));
    }

    #[test]
    fn load_or_create_writes_default_then_reads_back() {
        let dir = std::env::temp_dir().join(format!("gray-sys-test-{}", std::process::id()));
        let path = dir.join("sys.md");
        let _ = std::fs::remove_dir_all(&dir);

        let first = load_or_create_system_prompt_at(&path).unwrap();
        assert_eq!(first, DEFAULT_SYS_PROMPT);
        assert!(path.exists());

        std::fs::write(&path, "custom prompt body").unwrap();
        let second = load_or_create_system_prompt_at(&path).unwrap();
        assert_eq!(second, "custom prompt body");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
