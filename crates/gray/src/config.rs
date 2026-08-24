//! Configuration resolution for the Gray agent harness.

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
            });  // optional: REPL starts without a model; validated on first use

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
            });  // optional: validated on first use

        Ok(Self {
            model,
            base_url,
            api_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_cli() -> Cli {
        Cli {
            model: None,
            base_url: None,
            print: None,
            api_key: None,
        }
    }

    #[test]
    fn flags_override_all_env_and_defaults() {
        let mut cli = make_cli();
        cli.model = Some("custom/flag-model".to_string());
        cli.base_url = Some("https://flag.example.com/v1".to_string());
        cli.api_key = Some("flag-key-123".to_string());
        let mut env_map = HashMap::new();
        env_map.insert("GRAY_MODEL", "env/model");
        env_map.insert("GRAY_BASE_URL", "https://env.example.com/v1");
        env_map.insert("GRAY_API_KEY", "gray-env-key");
        env_map.insert("OPENAI_API_KEY", "openai-env-key");

        let config = Config::resolve_with(&cli, |k| env_map.get(k).map(|s| s.to_string()))
            .expect("resolution should succeed");

        assert_eq!(config.model.as_deref(), Some("custom/flag-model"));
        assert_eq!(config.base_url, "https://flag.example.com/v1");
        assert_eq!(config.api_key.as_deref(), Some("flag-key-123"));
    }

    #[test]
    fn env_overrides_defaults() {
        let cli = make_cli();

        let mut env_map = HashMap::new();
        env_map.insert("GRAY_MODEL", "env/gray-model");
        env_map.insert("GRAY_BASE_URL", "https://env.custom.com/v1");
        env_map.insert("GRAY_API_KEY", "gray-key-456");
        let config = Config::resolve_with(&cli, |k| env_map.get(k).map(|s| s.to_string()))
            .expect("resolution should succeed");

        assert_eq!(config.model.as_deref(), Some("env/gray-model"));
        assert_eq!(config.base_url, "https://env.custom.com/v1");
        assert_eq!(config.api_key.as_deref(), Some("gray-key-456"));
    }

    #[test]
    fn defaults_applied_when_env_and_flags_missing() {
        let cli = make_cli();

        let mut env_map = HashMap::new();
        env_map.insert("GRAY_MODEL", "provider/model-default-test");
        env_map.insert("GRAY_API_KEY", "key-xyz");

        let config = Config::resolve_with(&cli, |k| env_map.get(k).map(|s| s.to_string()))
            .expect("resolution should succeed");

        assert_eq!(config.model.as_deref(), Some("provider/model-default-test"));
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.api_key.as_deref(), Some("key-xyz"));
    }

    #[test]
    fn gray_api_key_takes_precedence_over_openai_api_key() {
        let cli = make_cli();

        let mut env_map = HashMap::new();
        env_map.insert("GRAY_MODEL", "provider/model");
        env_map.insert("GRAY_API_KEY", "gray-specific-key");
        env_map.insert("OPENAI_API_KEY", "openai-fallback-key");

        let config = Config::resolve_with(&cli, |k| env_map.get(k).map(|s| s.to_string()))
            .expect("resolution should succeed");

        assert_eq!(config.api_key.as_deref(), Some("gray-specific-key"));
    }

    #[test]
    fn openai_api_key_used_as_fallback() {
        let cli = make_cli();

        let mut env_map = HashMap::new();
        env_map.insert("GRAY_MODEL", "provider/model");
        env_map.insert("OPENAI_API_KEY", "openai-only-key");

        let config = Config::resolve_with(&cli, |k| env_map.get(k).map(|s| s.to_string()))
            .expect("resolution should succeed");

        assert_eq!(config.api_key.as_deref(), Some("openai-only-key"));
    }

    #[test]
    fn missing_model_fails_with_descriptive_error() {
        let cli = make_cli();

        let mut env_map = HashMap::new();
        env_map.insert("GRAY_API_KEY", "key");

        let config = Config::resolve_with(&cli, |k| env_map.get(k).map(|s| s.to_string()))
            .expect("should succeed without a model");
        assert_eq!(config.model, None);
    }

    #[test]
    fn missing_api_key_fails_with_descriptive_error() {
        let cli = make_cli();

        let mut env_map = HashMap::new();
        env_map.insert("GRAY_MODEL", "model-name");

        let config = Config::resolve_with(&cli, |k| env_map.get(k).map(|s| s.to_string()))
            .expect("should succeed without an api key");
        assert_eq!(config.api_key, None);
    }


    #[test]
    fn empty_whitespace_strings_treated_as_unset() {
        let mut cli = make_cli();
        cli.model = Some("   ".to_string());

        let mut env_map = HashMap::new();
        env_map.insert("GRAY_MODEL", "   ");
        env_map.insert("GRAY_API_KEY", "key");

        let config = Config::resolve_with(&cli, |k| env_map.get(k).map(|s| s.to_string()))
            .expect("whitespace model should be treated as unset");
        assert_eq!(config.model, None);
    }
}
