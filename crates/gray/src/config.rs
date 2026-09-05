//! Configuration resolution for the Gray agent harness.

use std::path::PathBuf;

use crate::Cli;

/// Default API base URL pointing to OpenRouter.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

fn nonempty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

/// Resolved application configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Target model identifier (e.g. "anthropic/claude-sonnet-4"). None until set.
    pub model: Option<String>,
    /// API endpoint base URL.
    pub base_url: String,
    /// API key for authentication. None until set.
    pub api_key: Option<String>,
    /// Thinking / reasoning effort level ("off", "minimal", "low", "medium", "high", "xhigh", "max").
    pub thinking_effort: Option<String>,
    /// Show reasoning text in the transcript. None (default) = shown.
    /// `GRAY_SHOW_REASONING=0/false/no/off` hides. Effort "off" always hides.
    pub show_reasoning: Option<bool>,
    /// User override for context window in tokens. Highest priority (over auto-fetched).
    pub context_window: Option<usize>,
    /// Reserve tokens before auto-compact fires.
    pub context_reserve: Option<usize>,
    /// Tail budget kept alongside the summary after compaction.
    pub context_keep: Option<usize>,
}

impl Config {
    /// Resolves configuration from CLI arguments and environment variables.
    pub fn resolve(cli: &Cli) -> anyhow::Result<Self> {
        Self::resolve_with(cli, |k| std::env::var(k).ok())
    }

    /// Resolves configuration with a custom environment lookup function (useful for testing).
    pub fn resolve_with<F>(cli: &Cli, mut env: F) -> anyhow::Result<Self>
    where
        F: FnMut(&str) -> Option<String>,
    {
        // Saved file (~/.gray/config.json) fills anything the user didn't
        // provide via flag or environment. Flags > env > saved file.
        let saved = crate::setup::load_saved_config_at(
            &crate::setup::saved_config_path().unwrap_or_else(|_| PathBuf::from("/dev/null")),
        );

        let model = nonempty(cli.model.as_deref())
            .or_else(|| nonempty(env("GRAY_MODEL").as_deref()))
            .or(saved.model); // optional: REPL starts without a model; validated on first use

        let base_url = nonempty(cli.base_url.as_deref())
            .or_else(|| nonempty(env("GRAY_BASE_URL").as_deref()))
            .or(saved.base_url)
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        let api_key = nonempty(cli.api_key.as_deref())
            .or_else(|| nonempty(env("GRAY_API_KEY").as_deref()))
            .or_else(|| nonempty(env("OPENAI_API_KEY").as_deref()))
            .or(saved.api_key); // optional: validated on first use

        let thinking_effort =
            nonempty(env("GRAY_THINKING_EFFORT").as_deref()).or(saved.thinking_effort);

        let show_reasoning = env("GRAY_SHOW_REASONING")
            .map(|s| {
                !matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .or(saved.show_reasoning);

        let context_window = cli
            .context_window
            .or_else(|| {
                env("GRAY_CONTEXT_WINDOW").and_then(|s| crate::setup::parse_context_window(&s))
            })
            .or(saved.context_window);

        let context_reserve = cli
            .context_reserve
            .or_else(|| {
                env("GRAY_CONTEXT_RESERVE").and_then(|s| crate::setup::parse_context_window(&s))
            })
            .or(saved.context_reserve);

        let context_keep = cli
            .context_keep
            .or_else(|| {
                env("GRAY_CONTEXT_KEEP").and_then(|s| crate::setup::parse_context_window(&s))
            })
            .or(saved.context_keep);

        let config = Self {
            model,
            base_url,
            api_key,
            thinking_effort,
            show_reasoning,
            context_window,
            context_reserve,
            context_keep,
        };
        log::info!(target: "gray_config", "config resolved: model={:?}, base_url={}, api_key={}, context_window={:?}", config.model, config.base_url, config.api_key.as_deref().map(|_| "set").unwrap_or("unset"), config.context_window);
        Ok(config)
    }

    /// Whether reasoning text stays hidden: effort "off" always hides,
    /// otherwise the show_reasoning setting decides (default shown).
    pub fn reasoning_hidden(&self) -> bool {
        self.thinking_effort.as_deref() == Some("off") || !self.show_reasoning.unwrap_or(true)
    }
}
