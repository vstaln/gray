//! Configuration resolution for the Gray agent harness.

use std::path::PathBuf;

use crate::Cli;

/// Default API base URL pointing to OpenRouter.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

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
    /// User override for context window in tokens. Highest priority (over auto-fetched).
    pub context_window: Option<usize>,
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

        let model = cli
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                env("GRAY_MODEL")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .or(saved.model); // optional: REPL starts without a model; validated on first use

        let base_url = cli
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                env("GRAY_BASE_URL")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .or(saved.base_url)
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        let api_key = cli
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                env("GRAY_API_KEY")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| {
                env("OPENAI_API_KEY")
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .or(saved.api_key); // optional: validated on first use

        let thinking_effort = env("GRAY_THINKING_EFFORT")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or(saved.thinking_effort);

        let context_window = cli
            .context_window
            .or_else(|| env("GRAY_CONTEXT_WINDOW").and_then(|s| crate::setup::parse_context_window(&s)))
            .or(saved.context_window);

        let config = Self {
            model,
            base_url,
            api_key,
            thinking_effort,
            context_window,
        };
        log::info!(target: "gray_config", "config resolved: model={:?}, base_url={}, api_key={}, context_window={:?}", config.model, config.base_url, config.api_key.as_deref().map(|_| "set").unwrap_or("unset"), config.context_window);
        Ok(config)
    }
}

