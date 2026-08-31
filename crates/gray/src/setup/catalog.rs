//! First-run onboarding: a searchable provider picker fed by the bundled
//! catalog (models.dev snapshot), persisting
//! to ~/.gray/config.json. Flow: nothing forced at boot; the
//! picker appears the moment credentials are actually needed.
// 3 modals (connect/model/effort) share 80% render + nav logic (662+287+163 lines); extract generic list_picker when adding fourth modal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{config::Config, tui::print_wrapped};

/// Provider entry from the vendored catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogProvider {
    pub name: String,
    pub base_url: String,
    /// models.dev emits either a string or a list of env var names.
    #[serde(default)]
    pub env_key: serde_json::Value,
    pub featured: bool,
    /// True when the upstream serves a keyless/free tier (9router noAuth).
    #[serde(default)]
    pub no_auth: bool,
    #[serde(default)]
    pub models: Vec<CatalogModel>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// The full catalog, keyed by provider id (`openrouter`, `deepseek`, ...).
pub type Catalog = BTreeMap<String, CatalogProvider>;

/// Bundled models.dev snapshot.
pub const PROVIDERS_JSON: &str = include_str!("../../assets/providers.json");

/// Parses the embedded catalog. Infinitely unlikely to fail (compiled in),
/// but returns a Result so callers can degrade gracefully.
pub fn load_catalog() -> anyhow::Result<Catalog> {
    Ok(serde_json::from_str(PROVIDERS_JSON)?)
}

/// First env var name from the catalog entry, for hints during key input.
fn env_hint(p: &CatalogProvider) -> String {
    match &p.env_key {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str())
            .next()
            .unwrap_or("API_KEY")
            .to_string(),
        _ => "API_KEY".to_string(),
    }
}

/// Pretty masked display for an existing key: `sk-••••Jh8a` (prettier dots, last 4 visible).
pub fn mask_key_pretty(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 4 {
        return "•".repeat(chars.len());
    }
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    if chars.len() <= 8 {
        return format!("{}{suffix}", "•".repeat(chars.len() - 4));
    }
    let prefix: String = if key.starts_with("sk-") {
        chars[..3].iter().collect()
    } else {
        chars[..2.min(chars.len())].iter().collect()
    };
    format!("{prefix}{}{suffix}", "•".repeat(4))
}

/// On-disk configuration, kept deliberately tiny.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// How the provider authenticates: "api_key" | "oauth" | "none".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
    /// Thinking / reasoning effort: "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_effort: Option<String>,
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
/// Mode 0600: the file stores the plaintext api_key.
pub fn save_saved_config_at(path: &Path, cfg: &SavedConfig) -> anyhow::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(serde_json::to_string_pretty(cfg)?.as_bytes())?;
    Ok(())
}


/// Per-provider API-key store (`~/.gray/auth.json`, mode 0600), mirroring
/// opencode's credential file: `{ "<provider-id>": "<key>", ... }`.
fn auth_store_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("auth.json"))
}

/// All stored keys keyed by provider id; missing file yields an empty map.
pub(crate) fn load_auth_keys() -> BTreeMap<String, String> {
    std::fs::read_to_string(auth_store_path().unwrap_or_else(|_| PathBuf::from("/dev/null")))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Upserts `key` under provider id `pid` (read-modify-write, 0600).
pub(crate) fn save_auth_key(pid: &str, key: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    let path = auth_store_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut keys = load_auth_keys();
    keys.insert(pid.to_string(), key.to_string());
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(serde_json::to_string_pretty(&keys)?.as_bytes())?;
    Ok(())
}

/// Provider item displayed in the "Connect a provider" modal.
#[derive(Debug, Clone)]
pub struct ConnectItem {
    pub id: String,
    pub name: String,
    pub sublabel: String,
    pub category: &'static str,
    pub base_url: String,
    pub env_key: String,
    pub no_auth: bool,
}

/// Builds the full list of providers for the connect modal:
/// Popular section on top, followed by all catalog providers under Providers.
pub fn build_connect_items(catalog: &Catalog) -> Vec<ConnectItem> {
    let popular_defs = [
        ("openai", "OpenAI", "(ChatGPT Plus/Pro or API key)", "https://api.openai.com/v1", "OPENAI_API_KEY", false),
        ("anthropic", "Anthropic", "(API key)", "https://api.anthropic.com/v1", "ANTHROPIC_API_KEY", false),
        ("google", "Google", "(Gemini API key)", "https://generativelanguage.googleapis.com/v1beta/openai", "GEMINI_API_KEY", false),
        ("openrouter", "OpenRouter", "(Access 300+ models)", "https://openrouter.ai/api/v1", "OPENROUTER_API_KEY", false),
        ("deepseek", "DeepSeek", "", "https://api.deepseek.com", "DEEPSEEK_API_KEY", false),
        ("groq", "Groq", "(Fast inference)", "https://api.groq.com/openai/v1", "GROQ_API_KEY", false),
        ("ollama", "Ollama", "(Local http://localhost:11434)", "http://localhost:11434/v1", "", true),
        ("github-copilot", "GitHub Copilot", "", "https://api.githubcopilot.com", "COPILOT_API_KEY", false),
        ("xai", "xAI (Grok)", "(Grok API key)", "https://api.x.ai/v1", "XAI_API_KEY", false),
        ("mistral", "Mistral", "(API key)", "https://api.mistral.ai/v1", "MISTRAL_API_KEY", false),
    ];

    let mut items = Vec::new();
    let mut popular_ids = std::collections::HashSet::new();

    for (id, name, sublabel, base_url, env_k, no_auth) in popular_defs {
        popular_ids.insert(id.to_string());
        let (url, env) = if let Some(p) = catalog.get(id) {
            let e = env_hint(p);
            (p.base_url.as_str(), e)
        } else {
            (base_url, env_k.to_string())
        };
        items.push(ConnectItem {
            id: id.to_string(),
            name: name.to_string(),
            sublabel: sublabel.to_string(),
            category: "Popular",
            base_url: url.to_string(),
            env_key: env,
            no_auth,
        });
    }

    // All catalog providers in alphabetical order
    let mut catalog_entries: Vec<_> = catalog.iter().collect();
    catalog_entries.sort_by_key(|(_, p)| p.name.to_lowercase());

    for (id, p) in catalog_entries {
        if popular_ids.contains(id) {
            continue;
        }

        items.push(ConnectItem {
            id: id.clone(),
            name: p.name.clone(),
            sublabel: String::new(),
            category: "Providers",
            base_url: p.base_url.clone(),
            env_key: env_hint(p),
            no_auth: p.no_auth,
        });
    }

    items
}

