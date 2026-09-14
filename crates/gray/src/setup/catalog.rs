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
    /// True when the upstream serves a keyless/free tier (9router noAuth).
    #[serde(default)]
    pub no_auth: bool,
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
/// Default-tolerant on purpose: every field is optional and unknown fields
/// are ignored so known fields survive hand-edits — never add
/// `deny_unknown_fields` here (it would drop the whole file on one typo).
#[derive(Clone, Default, Serialize, Deserialize)]
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
pub const AUTH_MODE_NONE: &str = "none";

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
/// A corrupt file warns loudly (file + serde line/column) and falls back to
/// defaults — or to the surviving fields when a single value is mistyped.
pub fn load_saved_config_at(path: &Path) -> SavedConfig {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SavedConfig::default(),
        Err(e) => {
            warn_bad_config(path, &format!("cannot read ({e:#})"));
            return SavedConfig::default();
        }
    };
    match serde_json::from_str::<SavedConfig>(&body) {
        Ok(cfg) => cfg,
        Err(e) => {
            // One mistyped value must not drop the whole file (e.g. model):
            // retry field-by-field so known-good fields survive.
            let recovered = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.as_object().cloned())
                .map(|obj| partial_saved_config(&obj))
                .unwrap_or_default();
            warn_bad_config(path, &e.to_string());
            recovered
        }
    }
}

/// Visible warning for a bad config file: stderr (immediate) + log
/// (existing `gray_config` target, cf. `Config::resolve`).
fn warn_bad_config(path: &Path, detail: &dyn std::fmt::Display) {
    let msg = format!(
        "cannot load {} config ({}); using defaults",
        path.display(),
        detail
    );
    eprintln!("warning: {msg}");
    log::warn!(target: "gray_config", "{msg}");
}

/// Per-field tolerant parse: unknown fields ignored, mistyped values become
/// None instead of nuking the whole file.
fn partial_saved_config(obj: &serde_json::Map<String, serde_json::Value>) -> SavedConfig {
    SavedConfig {
        base_url: opt_field(obj, "base_url"),
        api_key: opt_field(obj, "api_key"),
        model: opt_field(obj, "model"),
        auth_mode: opt_field(obj, "auth_mode"),
        thinking_effort: opt_field(obj, "thinking_effort"),
        show_reasoning: opt_field(obj, "show_reasoning"),
        context_window: opt_field(obj, "context_window"),
        context_reserve: opt_field(obj, "context_reserve"),
        context_keep: opt_field(obj, "context_keep"),
    }
}

fn opt_field<T>(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<T>
where
    T: for<'de> Deserialize<'de>,
{
    match obj.get(key) {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => serde_json::from_value::<Option<T>>(v.clone())
            .ok()
            .flatten(),
    }
}

/// Writes the config pretty-printed so users can hand-edit it too.
/// Mode 0600: the file stores the plaintext api_key.
pub fn save_saved_config_at(path: &Path, cfg: &SavedConfig) -> anyhow::Result<()> {
    let body = serde_json::to_string_pretty(cfg)?;
    save_private_json(path, &serde_json::from_str::<serde_json::Value>(&body)?)
}

/// Single private atomic writer for credential-bearing JSON: serialize
/// first, then write to a unique 0600 tmp in the same directory, sync,
/// and rename. A crash can never leave a truncated live file, and a
/// corrupt existing file is never read as empty-then-overwritten here —
/// callers load (and refuse to clobber) before calling save.
fn save_private_json(path: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
    use std::io::Write as _;
    let body = serde_json::to_vec_pretty(value)?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".gray-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&tmp)?;
        file.write_all(&body)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Per-provider API-key store (`~/.gray/auth.json`, mode 0600), mirroring
/// opencode's credential file: `{ "<provider-id>": "<key>", ... }`.
fn auth_store_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("auth.json"))
}

/// Persisted OAuth credential store. Lives here (not `gray-extras::oauth`)
/// so API-key helpers can read through the mixed `auth.json` store without
/// depending on the out-of-default-build OAuth signin flow.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AuthEntry {
    Key(String),
    OAuth(StoredAuth),
}

// Hand-redacted Debug: these types carry plaintext keys and tokens, so the
// derived impl would leak them into any log that formats config/auth.
impl std::fmt::Debug for SavedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedConfig")
            .field("model", &self.model)
            .field("auth_mode", &self.auth_mode)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for StoredAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredAuth")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for AuthEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Key(_) => f.write_str("Key(..)"),
            Self::OAuth(a) => f.debug_tuple("OAuth").field(a).finish(),
        }
    }
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
    save_private_json(
        path,
        &serde_json::to_value(store).map_err(|e| anyhow::anyhow!("{e}"))?,
    )
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
    pub base_url: String,
    pub no_auth: bool,
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
            false,
        ),
        (
            "anthropic",
            "Anthropic",
            "(API key)",
            "https://api.anthropic.com/v1",
            false,
        ),
        (
            "google",
            "Google",
            "(Gemini API key)",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            false,
        ),
        (
            "openrouter",
            "OpenRouter",
            "(Access 300+ models)",
            "https://openrouter.ai/api/v1",
            false,
        ),
        (
            "deepseek",
            "DeepSeek",
            "",
            "https://api.deepseek.com",
            false,
        ),
        (
            "groq",
            "Groq",
            "(Fast inference)",
            "https://api.groq.com/openai/v1",
            false,
        ),
        (
            "ollama",
            "Ollama",
            "(Local http://localhost:11434)",
            "http://localhost:11434/v1",
            true,
        ),
        (
            "github-copilot",
            "GitHub Copilot",
            "",
            "https://api.githubcopilot.com",
            false,
        ),
        (
            "xai",
            "xAI (Grok)",
            "(Grok login or API key)",
            "https://api.x.ai/v1",
            false,
        ),
        (
            "mistral",
            "Mistral",
            "(API key)",
            "https://api.mistral.ai/v1",
            false,
        ),
    ];

    let mut items = Vec::new();
    let mut popular_ids = std::collections::HashSet::new();

    for (id, name, sublabel, base_url, no_auth) in popular_defs {
        popular_ids.insert(id.to_string());
        let url = catalog.get(id).map_or(base_url, |p| p.base_url.as_str());
        items.push(ConnectItem {
            id: id.to_string(),
            name: name.to_string(),
            sublabel: sublabel.to_string(),
            base_url: url.to_string(),
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
            base_url: p.base_url.clone(),
            no_auth: p.no_auth,
        });
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // UNRUN (cargo test banned under X — verified via check + clippy only).
    #[test]
    fn mistyped_field_preserves_known_fields() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"model":"anthropic/claude","context_window":"not-a-number","base_url":"https://x"}"#,
        )
        .expect("write");
        let cfg = load_saved_config_at(&path);
        assert_eq!(cfg.model.as_deref(), Some("anthropic/claude"));
        assert_eq!(cfg.base_url.as_deref(), Some("https://x"));
        assert_eq!(cfg.context_window, None);
    }

    // UNRUN (cargo test banned under X — verified via check + clippy only).
    #[test]
    fn unknown_fields_are_ignored() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"model":"m","bogus_field":123}"#).expect("write");
        let cfg = load_saved_config_at(&path);
        assert_eq!(cfg.model.as_deref(), Some("m"));
    }

    // UNRUN (cargo test banned under X — verified via check + clippy only).
    #[test]
    fn bad_json_falls_back_to_defaults() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{bad json").expect("write");
        let cfg = load_saved_config_at(&path);
        assert_eq!(cfg.model, None);
        assert_eq!(cfg.base_url, None);
    }
}
