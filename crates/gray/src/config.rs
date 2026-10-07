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

/// API-key precedence: `--api-key` > `GRAY_API_KEY` > the stored key >
/// `OPENAI_API_KEY`. `OPENAI_API_KEY` is only a legacy fallback for the
/// OpenAI endpoint: an OpenAI Bearer against the default OpenRouter base
/// is just a bad key, so the env var is ignored there — letting the
/// connect flow's `(found OPENAI_API_KEY)` row offer it properly.
fn resolve_api_key(
    cli: &Cli,
    base_url: &str,
    env: &mut impl FnMut(&str) -> Option<String>,
    saved_key: Option<String>,
) -> Option<String> {
    nonempty(cli.api_key.as_deref())
        .or_else(|| nonempty(env("GRAY_API_KEY").as_deref()))
        .or(saved_key)
        .or_else(|| {
            let default_base = crate::setup::normalize_custom_base_url(base_url)
                == crate::setup::normalize_custom_base_url(DEFAULT_BASE_URL);
            if default_base {
                None
            } else {
                nonempty(env("OPENAI_API_KEY").as_deref())
            }
        })
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
// `Eq` is deliberately absent: the sampling passthroughs are `f32`.
#[derive(Clone, PartialEq)]
pub struct Config {
    /// Target model identifier (e.g. "anthropic/claude-sonnet-4"). None until set.
    pub model: Option<String>,
    /// API endpoint base URL.
    pub base_url: String,
    /// API key for authentication. None until set.
    pub api_key: Option<String>,
    /// Resolved provider id. Empty for the built-in API-key flow.
    pub provider_id: String,
    /// Credential mode for the active model/provider ("plugin" for dynamic).
    pub credential_source: String,
    /// Namespaced namespaced plugin credential reference.
    pub auth_ref: String,
    /// Thinking / reasoning effort level ("off", "minimal", "low", "medium", "high", "xhigh", "max").
    pub thinking_effort: Option<String>,
    /// Show reasoning text in the transcript. None (default) = shown.
    /// `GRAY_SHOW_REASONING=0/false/no/off` hides. Effort "off" always hides.
    pub show_reasoning: Option<bool>,
    /// Prefer the provider's fast-serving variant of the current model
    /// (`-fast`/`-priority` catalog rows). None/false = standard serving.
    /// `GRAY_FAST=0/false/no/off` disables; any other value enables.
    pub fast_mode: Option<bool>,
    /// Composite-row selection (`{"lead": …, "sidekick": …}`) for the
    /// current model — one option id per declared slot. Empty for every
    /// non-composite row.
    pub model_parts: std::collections::BTreeMap<String, String>,
    /// Sampling temperature sent with every chat request (`GRAY_TEMPERATURE`
    /// or saved config). None = provider default; out-of-range ignored.
    pub temperature: Option<f32>,
    /// Nucleus sampling cutoff sent with every chat request (`GRAY_TOP_P` or
    /// saved config). None = provider default; out-of-range ignored.
    pub top_p: Option<f32>,
    /// User override for context window in tokens. Highest priority (over auto-fetched).
    pub context_window: Option<usize>,
    /// Reserve tokens before auto-compact fires.
    pub context_reserve: Option<usize>,
    /// Tail budget kept alongside the summary after compaction.
    pub context_keep: Option<usize>,
    /// Program that runs the model's shell commands somewhere else, e.g.
    /// `docker exec -i dev sh -s` or `ssh box sh -s`. `GRAY_EXEC_PREFIX` or
    /// the saved `exec_prefix`; None = commands run in this process's shell.
    pub exec_prefix: Option<String>,
    /// Turn cap for this process (CLI `--max-turns` / `GRAY_MAX_TURNS`).
    pub max_turns: Option<u32>,
    /// Spend cap in micro-dollars for this process (exact `u64`: Eq-safe,
    /// unlike `f64`). Set via `--max-cost-usd` / `GRAY_MAX_COST_USD`.
    pub max_cost_micros: Option<u64>,
    /// Wall-clock cap in seconds from process start (`--max-wall-secs`).
    pub max_wall_secs: Option<u64>,
    /// `--bare` / `GRAY_BARE=1`: stock prompt + bash only (see `Cli::bare`).
    pub bare: bool,
    /// On by default; `--lean` / `GRAY_LEAN=1` force on, `GRAY_LEAN=0` or
    /// persisted `lean: false` opt out: no per-turn injected
    /// context (skills list, project rules, plugin docs). Tools, plugins
    /// and compaction stay on.
    pub lean: bool,
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
        let config = Self::resolve_with(cli, |k| std::env::var(k).ok())?;
        if let Some(prefix) = config
            .exec_prefix
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        {
            // gray-tools reads the prefix from the environment, next to the
            // other GRAY_* shell knobs. Set once here, before any tool can
            // spawn; a bad prefix is reported by the command it breaks, not
            // at boot, so an unused typo never blocks a session.
            unsafe { std::env::set_var(gray_tools::shell::spawn::EXEC_PREFIX_ENV, prefix) };
        }
        if config.bare {
            // Bare = mini-swe-agent shape: plain blocking bash (no job
            // control in its schema) and no automatic compaction. Same
            // set-once-before-any-tool rule as the exec prefix above.
            unsafe {
                std::env::set_var("GRAY_NO_JOBS", "1");
                std::env::set_var("GRAY_NO_AUTO_COMPACT", "1");
            }
        }
        Ok(config)
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

        let api_key = resolve_api_key(cli, &base_url, &mut env, saved.api_key); // optional: validated on first use

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

        let fast_mode = env("GRAY_FAST")
            .map(|s| {
                !matches!(
                    s.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "no" | "off"
                )
            })
            .or(saved.fast_mode);

        // Sampling params pass through to providers that accept them.
        // Out-of-range values are ignored, never clamped or sent.
        let temperature = env("GRAY_TEMPERATURE")
            .and_then(|s| s.trim().parse::<f32>().ok())
            .filter(|t| (0.0..=2.0).contains(t))
            .or(saved.temperature);
        let top_p = env("GRAY_TOP_P")
            .and_then(|s| s.trim().parse::<f32>().ok())
            .filter(|p| *p > 0.0 && *p <= 1.0)
            .or(saved.top_p);

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

        // Where the model's shell commands run. The saved value is the
        // durable one (it names a box); the env var is the override.
        let exec_prefix = nonempty(env("GRAY_EXEC_PREFIX").as_deref()).or(saved.exec_prefix);

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

        let bare = cli.bare || env("GRAY_BARE").is_some_and(|v| v.trim() == "1");
        // Lean is the default: the stored prompt stays the whole system
        // prefix unless hooks are wanted back (`lean: false`, GRAY_LEAN=0).
        let lean_env = env("GRAY_LEAN").map(|v| v.trim().to_string());
        let lean = if cli.lean || lean_env.as_deref() == Some("1") {
            true
        } else if lean_env.as_deref() == Some("0") {
            false
        } else {
            saved.lean.unwrap_or(true)
        };

        let config = Self {
            model,
            base_url,
            api_key,
            provider_id: nonempty(Some(saved.provider_id.as_str())).unwrap_or_default(),
            credential_source: nonempty(Some(saved.credential_source.as_str())).unwrap_or_default(),
            auth_ref: nonempty(Some(saved.auth_ref.as_str())).unwrap_or_default(),
            thinking_effort,
            show_reasoning,
            fast_mode,
            model_parts: saved.model_parts.clone(),
            temperature,
            top_p,
            context_window,
            context_reserve,
            context_keep,
            exec_prefix,
            max_turns,
            max_cost_micros,
            max_wall_secs,
            bare,
            lean,
        };
        log::info!(target: "gray_config", "config resolved: model={:?}, base_url={}, api_key={}, context_window={:?}, bare={}, lean={}", config.model, scrub_url(&config.base_url), config.api_key.as_deref().map(|_| "set").unwrap_or("unset"), config.context_window, config.bare, config.lean);
        Ok(config)
    }

    /// Whether reasoning text stays hidden: effort "off" always hides,
    /// otherwise the show_reasoning setting decides (default shown).
    pub fn reasoning_hidden(&self) -> bool {
        self.thinking_effort.as_deref() == Some("off") || !self.show_reasoning.unwrap_or(true)
    }

    /// True only for a fully identified plugin-backed provider connection.
    pub fn uses_plugin_credentials(&self) -> bool {
        self.credential_source == "plugin"
            && !self.provider_id.is_empty()
            && !self.auth_ref.is_empty()
    }
}

#[path = "config_tests.rs"]
#[cfg(test)]
mod tests;
