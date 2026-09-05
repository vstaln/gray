//! First-run onboarding: a searchable provider picker fed by the bundled
//! catalog (models.dev snapshot), persisting
//! to ~/.gray/config.json. Flow: nothing forced at boot; the
//! picker appears the moment credentials are actually needed.
// 3 modals (connect/model/effort) share 80% render + nav logic (662+287+163 lines); extract generic list_picker when adding fourth modal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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
    /// Show reasoning text in the transcript (effort "off" always hides).
    /// None (default) = shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_reasoning: Option<bool>,
    /// User override for model context window in tokens (e.g. 128000). When set,
    /// it takes precedence over the auto-fetched provider value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,
    /// Reserve tokens before auto-compact fires (effective window = window - reserve).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_reserve: Option<usize>,
    /// Tail budget kept alongside the summary after compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_keep: Option<usize>,
}

/// Canonical `SavedConfig.auth_mode` values (kept as strings on disk).
pub const AUTH_MODE_API_KEY: &str = "api_key";
pub const AUTH_MODE_OAUTH: &str = "oauth";
pub const AUTH_MODE_NONE: &str = "none";

/// Unknown/missing modes behave as today: API key.
pub fn normalize_auth_mode(mode: Option<&str>) -> &'static str {
    match mode {
        Some("oauth") => AUTH_MODE_OAUTH,
        Some("none") => AUTH_MODE_NONE,
        _ => AUTH_MODE_API_KEY,
    }
}

/// Provider ids with an OAuth login flow (`oauth.rs` implements both).
/// Table, not an if-ladder.
pub const OAUTH_CAPABLE: &[&str] = &["openai", "xai"];

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

/// Persisted OAuth credential store. Lives here (not `gray-extras::oauth`)
/// so API-key helpers can read through the mixed `auth.json` store without
/// depending on the out-of-default-build OAuth signin flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredAuth {
    pub provider: String,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    pub expires_at: i64,
    #[serde(default)]
    pub email: Option<String>,
}

/// One `auth.json` entry: a plaintext API key or an OAuth credential. The
/// file is a mixed map `{pid: String | StoredAuth}` (plus a legacy
/// single-object form); key helpers and OAuth saves share it so neither
/// writer clobbers the other's shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AuthEntry {
    Key(String),
    OAuth(StoredAuth),
}

pub fn load_mixed_store(path: &Path) -> BTreeMap<String, AuthEntry> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    if let Ok(single) = serde_json::from_str::<StoredAuth>(&body) {
        let mut map = BTreeMap::new();
        map.insert(single.provider.clone(), AuthEntry::OAuth(single));
        return map;
    }
    serde_json::from_str::<BTreeMap<String, AuthEntry>>(&body).unwrap_or_default()
}

pub fn save_mixed_store(path: &Path, store: &BTreeMap<String, AuthEntry>) -> anyhow::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }

    let json = serde_json::to_string_pretty(&store)?;
    file.write_all(json.as_bytes())?;
    file.flush()?;
    Ok(())
}

/// All stored keys keyed by provider id; missing file yields an empty map.
/// Reads through the mixed auth store so OAuth objects in the same file are
/// left untouched (see [`AuthEntry`]).
pub fn load_auth_keys() -> BTreeMap<String, String> {
    let path = auth_store_path().unwrap_or_else(|_| PathBuf::from("/dev/null"));
    load_mixed_store(&path)
        .into_iter()
        .filter_map(|(k, v)| match v {
            AuthEntry::Key(key) => Some((k, key)),
            AuthEntry::OAuth(_) => None,
        })
        .collect()
}

/// Upserts `key` under provider id `pid` (read-modify-write, 0600),
/// preserving any OAuth entries in the same file.
pub(crate) fn save_auth_key(pid: &str, key: &str) -> anyhow::Result<()> {
    let path = auth_store_path()?;
    let mut store = load_mixed_store(&path);
    store.insert(pid.to_string(), AuthEntry::Key(key.to_string()));
    save_mixed_store(&path, &store)
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
    /// True when the provider also offers browser OAuth login.
    pub oauth_capable: bool,
}

/// Builds the full list of providers for the connect modal:
/// Popular section on top, followed by all catalog providers under Providers.
pub fn build_connect_items(catalog: &Catalog) -> Vec<ConnectItem> {
    let popular_defs = [
        (
            "openai",
            "OpenAI",
            "(ChatGPT login or API key)",
            "https://api.openai.com/v1",
            "OPENAI_API_KEY",
            false,
        ),
        (
            "anthropic",
            "Anthropic",
            "(API key)",
            "https://api.anthropic.com/v1",
            "ANTHROPIC_API_KEY",
            false,
        ),
        (
            "google",
            "Google",
            "(Gemini API key)",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "GEMINI_API_KEY",
            false,
        ),
        (
            "openrouter",
            "OpenRouter",
            "(Access 300+ models)",
            "https://openrouter.ai/api/v1",
            "OPENROUTER_API_KEY",
            false,
        ),
        (
            "deepseek",
            "DeepSeek",
            "",
            "https://api.deepseek.com",
            "DEEPSEEK_API_KEY",
            false,
        ),
        (
            "groq",
            "Groq",
            "(Fast inference)",
            "https://api.groq.com/openai/v1",
            "GROQ_API_KEY",
            false,
        ),
        (
            "ollama",
            "Ollama",
            "(Local http://localhost:11434)",
            "http://localhost:11434/v1",
            "",
            true,
        ),
        (
            "github-copilot",
            "GitHub Copilot",
            "",
            "https://api.githubcopilot.com",
            "COPILOT_API_KEY",
            false,
        ),
        (
            "xai",
            "xAI (Grok)",
            "(Grok login or API key)",
            "https://api.x.ai/v1",
            "XAI_API_KEY",
            false,
        ),
        (
            "mistral",
            "Mistral",
            "(API key)",
            "https://api.mistral.ai/v1",
            "MISTRAL_API_KEY",
            false,
        ),
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
            oauth_capable: OAUTH_CAPABLE.contains(&id),
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
            oauth_capable: OAUTH_CAPABLE.contains(&id.as_str()),
        });
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_openai_and_xai_offer_oauth() {
        let catalog = load_catalog().expect("bundled catalog parses");
        let items = build_connect_items(&catalog);
        let dual: Vec<_> = items
            .iter()
            .filter(|i| i.oauth_capable)
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(dual, vec!["openai", "xai"], "{dual:?}");
    }

    #[test]
    fn normalize_auth_mode_defaults_to_api_key() {
        assert_eq!(normalize_auth_mode(None), AUTH_MODE_API_KEY);
        assert_eq!(normalize_auth_mode(Some("oauth")), AUTH_MODE_OAUTH);
        assert_eq!(normalize_auth_mode(Some("none")), AUTH_MODE_NONE);
        assert_eq!(normalize_auth_mode(Some("bogus")), AUTH_MODE_API_KEY);
    }

    #[test]
    fn saving_key_preserves_oauth_objects() {
        // Mirror of the oauth-side clobber regression: key saves must not
        // wipe OAuth entries sharing auth.json.
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("auth.json");
        let oauth = StoredAuth {
            provider: "xai".to_string(),
            access_token: "tok".to_string(),
            refresh_token: String::new(),
            expires_at: 9_999_999_999,
            email: None,
        };
        let mut store = load_mixed_store(&path);
        store.insert(oauth.provider.clone(), AuthEntry::OAuth(oauth));
        save_mixed_store(&path, &store).expect("oauth save");
        let mut store = load_mixed_store(&path);
        store.insert(
            "openrouter".to_string(),
            AuthEntry::Key("sk-or-1".to_string()),
        );
        save_mixed_store(&path, &store).expect("key save");
        let reloaded = load_mixed_store(&path);
        assert!(
            matches!(reloaded.get("xai"), Some(AuthEntry::OAuth(_))),
            "{reloaded:?}"
        );
        // And the key-only view exposes just the key.
        let keys: BTreeMap<String, String> = reloaded
            .into_iter()
            .filter_map(|(k, v)| match v {
                AuthEntry::Key(key) => Some((k, key)),
                AuthEntry::OAuth(_) => None,
            })
            .collect();
        assert_eq!(keys.get("openrouter").map(String::as_str), Some("sk-or-1"));
        assert!(!keys.contains_key("xai"));
    }
}
