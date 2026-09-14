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

/// scheme://host/path without userinfo, query, or fragment (for logs).
fn scrub_url(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let auth_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let host = after_scheme[..auth_end].rsplit('@').next().unwrap_or("");
    let mut path: &str = &after_scheme[auth_end..];
    path = path.split(['?', '#']).next().unwrap_or("");
    match url.split_once("://") {
        Some((scheme, _)) => format!("{scheme}://{host}{path}"),
        None => format!("{host}{path}"),
    }
}

/// Resolved application configuration.
///
/// `Debug` is redacted by hand: the config carries the plaintext API key.
#[derive(Clone, PartialEq, Eq)]
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
    /// Turn cap for this process (CLI `--max-turns` / `GRAY_MAX_TURNS`).
    pub max_turns: Option<u32>,
    /// Spend cap in micro-dollars for this process (exact `u64`: Eq-safe,
    /// unlike `f64`). Set via `--max-cost-usd` / `GRAY_MAX_COST_USD`.
    pub max_cost_micros: Option<u64>,
    /// Wall-clock cap in seconds from process start (`--max-wall-secs`).
    pub max_wall_secs: Option<u64>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("model", &self.model)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_deref().map(|_| "set"))
            .finish_non_exhaustive()
    }
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

        // Caps are CLI/env-only (never persisted): they bound one run, not
        // the identity. Non-positive values are ignored, never clamped.
        let max_turns = cli.max_turns.filter(|&n| n > 0).or_else(|| {
            env("GRAY_MAX_TURNS").and_then(|s| s.trim().parse::<u32>().ok().filter(|&n| n > 0))
        });
        let parse_usd = |s: &str| {
            s.trim()
                .parse::<f64>()
                .ok()
                .filter(|c| c.is_finite() && *c > 0.0)
                .map(|c| (c * 1_000_000.0).round() as u64)
                .filter(|&m| m > 0)
        };
        let max_cost_micros = cli
            .max_cost_usd
            .filter(|c| c.is_finite() && *c > 0.0)
            .and_then(|c| parse_usd(&c.to_string()))
            .or_else(|| env("GRAY_MAX_COST_USD").as_deref().and_then(parse_usd));
        let max_wall_secs = cli.max_wall_secs.filter(|&n| n > 0).or_else(|| {
            env("GRAY_MAX_WALL_SECS").and_then(|s| s.trim().parse::<u64>().ok().filter(|&n| n > 0))
        });

        let config = Self {
            model,
            base_url,
            api_key,
            thinking_effort,
            show_reasoning,
            context_window,
            context_reserve,
            context_keep,
            max_turns,
            max_cost_micros,
            max_wall_secs,
        };
        log::info!(target: "gray_config", "config resolved: model={:?}, base_url={}, api_key={}, context_window={:?}", config.model, scrub_url(&config.base_url), config.api_key.as_deref().map(|_| "set").unwrap_or("unset"), config.context_window);
        Ok(config)
    }

    /// Whether reasoning text stays hidden: effort "off" always hides,
    /// otherwise the show_reasoning setting decides (default shown).
    pub fn reasoning_hidden(&self) -> bool {
        self.thinking_effort.as_deref() == Some("off") || !self.show_reasoning.unwrap_or(true)
    }
}
