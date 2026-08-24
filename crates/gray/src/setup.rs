//! First-run onboarding: an interactive wizard that collects provider, API key,
//! and model, then persists them to `~/.gray/config.json`.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Config, DEFAULT_BASE_URL};

/// A known OpenAI-compatible provider, pre-wired with its base URL.
pub struct ProviderPreset {
    pub label: &'static str,
    pub base_url: &'static str,
    pub suggested_model: &'static str,
}

/// The short list shown by the wizard. Custom endpoints go through option 4.
pub const PROVIDER_PRESETS: [ProviderPreset; 3] = [
    ProviderPreset {
        label: "OpenRouter",
        base_url: "https://openrouter.ai/api/v1",
        suggested_model: "anthropic/claude-sonnet-4",
    },
    ProviderPreset {
        label: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        suggested_model: "deepseek-chat",
    },
    ProviderPreset {
        label: "Groq",
        base_url: "https://api.groq.com/openai/v1",
        suggested_model: "llama-3.3-70b-versatile",
    },
];

/// On-disk configuration, kept deliberately tiny.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Resolves `$GRAY_HOME` (or `$HOME/.gray`) — shared root for gray's files.
pub fn gray_home() -> anyhow::Result<PathBuf> {
    let base = std::env::var("GRAY_HOME")
        .or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray")))
        .map_err(|_| anyhow::anyhow!("cannot resolve home: set HOME or GRAY_HOME"))?;
    Ok(PathBuf::from(base))
}

/// Path to the persisted config file.
pub fn saved_config_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("config.json"))
}

/// Loads the saved config; a missing file yields an all-None struct.
pub fn load_saved_config_at(path: &Path) -> SavedConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Writes the config pretty-printed so users can hand-edit it too.
pub fn save_saved_config_at(path: &Path, cfg: &SavedConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(cfg)?)?;
    Ok(())
}

fn read_line(prompt: &str) -> anyhow::Result<String> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Runs the interactive first-run wizard, mutating `config` in place and
/// persisting the result. Only called when model or key are still unset.
pub fn run_setup(config: &mut Config) -> anyhow::Result<()> {
    println!("welcome to gray — three quick questions (saved to ~/.gray/config.json)");
    println!();

    // 1. Provider
    println!("which provider?");
    for (i, p) in PROVIDER_PRESETS.iter().enumerate() {
        println!("  {}. {}", i + 1, p.label);
    }
    println!("  {}. custom endpoint", PROVIDER_PRESETS.len() + 1);
    let choice = read_line("provider [1]: ")?;
    let idx: usize = match choice.parse() {
        Ok(n) if (1..=PROVIDER_PRESETS.len() + 1).contains(&n) => n - 1,
        _ => 0,
    };

    // 2. Base URL (fixed for presets, asked for custom)
    let (base_url, suggested) = match PROVIDER_PRESETS.get(idx) {
        Some(p) => (p.base_url.to_string(), p.suggested_model.to_string()),
        None => {
            let url = read_line("base url (e.g. http://localhost:11434/v1): ")?;
            let url = if url.is_empty() { DEFAULT_BASE_URL.to_string() } else { url };
            let suggested = read_line("suggested model id (optional): ")?;
            (url, suggested)
        }
    };

    // 3. API key (pre-filled from environment if already exported)
    let env_key = config.api_key.clone().unwrap_or_default();
    let key_hint = if env_key.is_empty() {
        String::new()
    } else {
        format!(" [{}…{}]", &env_key[..3.min(env_key.len())], &env_key[env_key.len().saturating_sub(4)..])
    };
    let key_in = read_line(&format!("api key{key_hint}: "))?;
    let api_key = if key_in.is_empty() { env_key } else { key_in };

    // 4. Model
    let model_in = read_line(&format!("model [{}]: ", suggested))?;
    let model = if model_in.is_empty() { suggested } else { model_in };

    // Persist + apply in memory
    let saved = SavedConfig {
        base_url: Some(base_url.clone()),
        api_key: Some(api_key.clone()),
        model: Some(model.clone()),
    };
    let path = saved_config_path()?;
    save_saved_config_at(&path, &saved)?;

    config.model = Some(model);
    config.api_key = Some(api_key);
    config.base_url = base_url;

    println!();
    println!("saved. edit {} anytime, or re-run /setup.", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_config_round_trips_through_json() {
        let dir = std::env::temp_dir().join(format!("gray-setup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.json");

        assert!(load_saved_config_at(&path).model.is_none()); // missing file = defaults

        let cfg = SavedConfig {
            base_url: Some("https://api.deepseek.com/v1".into()),
            api_key: Some("sk-test".into()),
            model: Some("deepseek-chat".into()),
        };
        save_saved_config_at(&path, &cfg).unwrap();

        let loaded = load_saved_config_at(&path);
        assert_eq!(loaded.api_key.as_deref(), Some("sk-test"));
        assert_eq!(loaded.model.as_deref(), Some("deepseek-chat"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
