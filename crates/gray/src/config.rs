use std::path::PathBuf;

use crate::Cli;

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

fn remembered_effort(
    memory: &std::collections::BTreeMap<String, String>,
    provider_id: &str,
    base_url: &str,
    model: Option<&str>,
) -> Option<String> {
    let model = nonempty(model)?;
    memory
        .get(&crate::setup::effort_memory_key(
            provider_id,
            base_url,
            &model,
        ))
        .cloned()
}

fn nonempty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
}

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

fn desktop_bus(current: Option<&str>, home: &std::path::Path) -> Option<String> {
    let live = |addr: &str| {
        addr.strip_prefix("unix:path=")
            .is_none_or(|p| std::path::Path::new(p.split(',').next().unwrap_or(p)).exists())
    };
    if current.is_some_and(live) {
        return None;
    }
    let newest = std::fs::read_dir(home.join(".dbus/session-bus"))
        .ok()?
        .flatten()
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?;
    std::fs::read_to_string(newest.path())
        .ok()?
        .lines()
        .find_map(|l| l.strip_prefix("DBUS_SESSION_BUS_ADDRESS="))
        .map(|a| a.trim_matches('\'').to_string())
        .filter(|a| a != current.unwrap_or_default() && live(a))
}

#[derive(Clone, PartialEq)]
pub struct Config {
    pub model: Option<String>,

    pub base_url: String,

    pub api_key: Option<String>,

    pub provider_id: String,

    pub credential_source: String,

    pub auth_ref: String,

    pub thinking_effort: Option<String>,

    pub show_reasoning: Option<bool>,

    pub fast_mode: Option<bool>,

    pub model_parts: std::collections::BTreeMap<String, String>,

    pub temperature: Option<f32>,

    pub top_p: Option<f32>,

    pub context_window: Option<usize>,

    pub context_reserve: Option<usize>,

    pub context_keep: Option<usize>,

    pub compact_at: Option<crate::setup::CompactAt>,

    pub exec_prefix: Option<String>,

    pub max_turns: Option<u32>,

    pub max_cost_micros: Option<u64>,

    pub max_wall_secs: Option<u64>,

    pub bare: bool,

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
    pub fn resolve(cli: &Cli) -> anyhow::Result<Self> {
        let config = Self::resolve_with(cli, |k| std::env::var(k).ok())?;
        if let Some(prefix) = config
            .exec_prefix
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        {
            unsafe { std::env::set_var(gray_tools::shell::spawn::EXEC_PREFIX_ENV, prefix) };
        }
        if config.bare {
            unsafe {
                std::env::set_var("GRAY_NO_JOBS", "1");
                std::env::set_var("GRAY_NO_AUTO_COMPACT", "1");
            }
        }
        if let Some(home) = std::env::var_os("HOME").filter(|s| !s.is_empty())
            && let Some(bus) = desktop_bus(
                std::env::var("DBUS_SESSION_BUS_ADDRESS").ok().as_deref(),
                std::path::Path::new(&home),
            )
        {
            unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", bus) };
        }
        export_provider_env(&config);
        Ok(config)
    }

    pub fn resolve_with<F>(cli: &Cli, mut env: F) -> anyhow::Result<Self>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let saved = crate::setup::load_saved_config_at(
            &crate::setup::saved_config_path().unwrap_or_else(|_| PathBuf::from("/dev/null")),
        );

        let saved_model = saved.model.clone();
        let model = nonempty(cli.model.as_deref())
            .or_else(|| nonempty(env("GRAY_MODEL").as_deref()))
            .or(saved.model);

        let base_url = nonempty(cli.base_url.as_deref())
            .or_else(|| nonempty(env("GRAY_BASE_URL").as_deref()))
            .or(saved.base_url)
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        let provider_id = env("GRAY_PROVIDER_ID")
            .map(|v| v.trim().to_string())
            .unwrap_or_else(|| nonempty(Some(saved.provider_id.as_str())).unwrap_or_default());
        let target_effort = remembered_effort(
            &saved.effort_memory,
            &provider_id,
            &base_url,
            model.as_deref(),
        );

        let api_key = resolve_api_key(cli, &base_url, &mut env, saved.api_key);

        let thinking_effort = nonempty(env("GRAY_THINKING_EFFORT").as_deref())
            .or(target_effort)
            .or(saved.thinking_effort);

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

        let compact_at = cli
            .compact_at
            .or(env("GRAY_COMPACT_AT").and_then(|s| crate::setup::parse_compact_at(&s)))
            .or(saved.compact_at);
        let context_keep = cli
            .context_keep
            .or_else(|| {
                env("GRAY_CONTEXT_KEEP").and_then(|s| crate::setup::parse_context_window(&s))
            })
            .or(saved.context_keep);

        let exec_prefix = nonempty(env("GRAY_EXEC_PREFIX").as_deref()).or(saved.exec_prefix);

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

        let lean_env = env("GRAY_LEAN").map(|v| v.trim().to_string());
        let lean = if cli.lean || lean_env.as_deref() == Some("1") {
            true
        } else if lean_env.as_deref() == Some("0") {
            false
        } else {
            saved.lean.unwrap_or(true)
        };

        let credential_source = env("GRAY_CREDENTIAL_SOURCE")
            .map(|v| v.trim().to_string())
            .unwrap_or_else(|| {
                nonempty(Some(saved.credential_source.as_str())).unwrap_or_default()
            });
        let auth_ref = env("GRAY_AUTH_REF")
            .map(|v| v.trim().to_string())
            .unwrap_or_else(|| nonempty(Some(saved.auth_ref.as_str())).unwrap_or_default());

        let parse_parts = |raw: &str| -> std::collections::BTreeMap<String, String> {
            raw.split(',')
                .filter_map(|kv| {
                    kv.split_once('=')
                        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                })
                .filter(|(k, v)| !k.is_empty() && !v.is_empty())
                .collect()
        };
        let model_parts = cli
            .model_parts
            .as_deref()
            .map(parse_parts)
            .filter(|m| !m.is_empty())
            .or_else(|| {
                env("GRAY_MODEL_PARTS")
                    .map(|v| parse_parts(&v))
                    .filter(|m| !m.is_empty())
            })
            .unwrap_or_else(|| {
                if model == saved_model && !saved.model_parts.is_empty() {
                    saved.model_parts.clone()
                } else {
                    model
                        .as_deref()
                        .and_then(|m| {
                            saved
                                .parts_memory
                                .get(&crate::setup::effort_memory_key(&provider_id, &base_url, m))
                                .cloned()
                        })
                        .unwrap_or_default()
                }
            });

        let config = Self {
            model,
            base_url,
            api_key,
            provider_id,
            credential_source,
            auth_ref,
            thinking_effort,
            show_reasoning,
            fast_mode,
            model_parts,
            temperature,
            top_p,
            context_window,
            context_reserve,
            context_keep,
            compact_at,
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

    pub fn reasoning_hidden(&self) -> bool {
        self.thinking_effort.as_deref() == Some("off") || !self.show_reasoning.unwrap_or(true)
    }

    pub fn uses_plugin_credentials(&self) -> bool {
        self.credential_source == "plugin"
            && !self.provider_id.is_empty()
            && !self.auth_ref.is_empty()
    }
}

pub(crate) fn offline() -> bool {
    std::env::var("GRAY_OFFLINE").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
}

/// Children spawned under this process — plugin sidecars, `gray -p`
/// subagents, agent-run CLIs — inherit the session's provider rather than
/// racing the last-writer-wins saved config. A non-plugin selection clears
/// the vars so a stale inherited pin can't leak into grandchildren.
pub(crate) fn export_provider_env(config: &Config) {
    unsafe {
        if config.uses_plugin_credentials() {
            std::env::set_var("GRAY_PROVIDER_ID", &config.provider_id);
            std::env::set_var("GRAY_AUTH_REF", &config.auth_ref);
            std::env::set_var("GRAY_CREDENTIAL_SOURCE", "plugin");
        } else {
            std::env::remove_var("GRAY_PROVIDER_ID");
            std::env::remove_var("GRAY_AUTH_REF");
            if std::env::var("GRAY_CREDENTIAL_SOURCE").as_deref() == Ok("plugin") {
                std::env::remove_var("GRAY_CREDENTIAL_SOURCE");
            }
        }
    }
}

#[path = "config_tests.rs"]
#[cfg(test)]
mod tests;
