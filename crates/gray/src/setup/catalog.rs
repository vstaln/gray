//! First-run onboarding: a searchable provider picker fed by the bundled
//! catalog (models.dev snapshot), persisting
//! to ~/.gray/config.json. Flow: nothing forced at boot; the
//! picker appears the moment credentials are actually needed.
// 3 modals (connect/model/effort) share 80% render + nav logic (662+287+163 lines); extract generic list_picker when adding fourth modal.

use std::collections::{BTreeMap, BTreeSet};
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
    /// Most recently selected model IDs first, scoped by normalized API base URL.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub recent_models: BTreeMap<String, Vec<String>>,
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
    /// Sampling temperature sent with every chat request (None = provider default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling cutoff sent with every chat request (None = provider default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// User override for model context window in tokens (e.g. 128000). When set,
    /// it takes precedence over the auto-fetched provider value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,
    /// Master switch for automatic skill context (`/skills on` | `/skills off`,
    /// default on). `Some(false)` hides the whole `<available_skills>` block
    /// from the model; explicit `/skills <name>` still loads that one skill.
    /// `None` (missing key, older configs) reads as enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills_auto: Option<bool>,
    /// Reserve tokens before auto-compact fires (effective window = window - reserve).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_reserve: Option<usize>,
    /// Tail budget kept alongside the summary after compaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_keep: Option<usize>,
    /// Skills the user turned off (absent = enabled). `/skills disable <name>`
    /// adds here, `/skills enable <name>` removes; the prompt list skips
    /// these while manual `/skills <name>` still runs.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub disabled_skills: BTreeSet<String>,
}

/// Canonical `SavedConfig.auth_mode` values (kept as strings on disk).
pub const AUTH_MODE_API_KEY: &str = "api_key";
pub const AUTH_MODE_NONE: &str = "none";

/// Resolves `$GRAY_HOME` (or `$HOME/.gray`) — shared root for gray's files.
pub fn gray_home() -> anyhow::Result<PathBuf> {
    gray_core::paths::gray_home().ok_or_else(|| {
        anyhow::anyhow!("cannot resolve home: set GRAY_HOME or the platform user profile")
    })
}

/// Path to the persisted config file.
pub fn saved_config_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("config.json"))
}

/// Names the user disabled via `/skills disable` (empty = all enabled).
/// Missing/unresolvable/corrupt config reads as empty (all on).
pub fn disabled_skill_names() -> BTreeSet<String> {
    saved_config_path()
        .map(|p| load_saved_config_at(&p).disabled_skills)
        .unwrap_or_default()
}

/// Master switch for automatic skill context (`/skills on` | `/skills off`).
/// Missing/unresolvable/corrupt config reads as enabled (default on); only an
/// explicit `Some(false)` turns auto-loading off.
pub fn skills_auto_enabled() -> bool {
    saved_config_path()
        .map(|p| load_saved_config_at(&p).skills_auto.unwrap_or(true))
        .unwrap_or(true)
}

/// Explicit-config-path seam for [`skills_auto_enabled`] (tests).
pub fn skills_auto_enabled_at(path: &Path) -> bool {
    load_saved_config_at(path).skills_auto.unwrap_or(true)
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
        recent_models: opt_field(obj, "recent_models").unwrap_or_default(),
        auth_mode: opt_field(obj, "auth_mode"),
        thinking_effort: opt_field(obj, "thinking_effort"),
        show_reasoning: opt_field(obj, "show_reasoning"),
        temperature: opt_field(obj, "temperature"),
        top_p: opt_field(obj, "top_p"),
        context_window: opt_field(obj, "context_window"),
        skills_auto: opt_field(obj, "skills_auto"),
        context_reserve: opt_field(obj, "context_reserve"),
        context_keep: opt_field(obj, "context_keep"),
        disabled_skills: opt_field(obj, "disabled_skills").unwrap_or_default(),
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
    let previous = load_saved_config_at(path);
    let mut persisted = cfg.clone();
    // Seed older configs and preserve the provider we are leaving. Do not
    // reorder history when merely saving effort/context settings.
    for selection in [&previous, cfg] {
        if let (Some(base), Some(model)) = (&selection.base_url, &selection.model) {
            let base = normalize_custom_base_url(base);
            if !base.is_empty() && !model.trim().is_empty() {
                let recent = persisted.recent_models.entry(base).or_default();
                recent.retain(|id| id != model);
                recent.insert(0, model.clone());
            }
        }
    }
    let body = serde_json::to_string_pretty(&persisted)?;
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
pub(crate) fn auth_store_path() -> anyhow::Result<PathBuf> {
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

/// Explicit-path seam for the provider-removal path (tests).
pub(crate) fn remove_auth_entry_at(path: &Path, pid: &str) -> anyhow::Result<()> {
    let mut store = load_mixed_store(path);
    store.remove(pid);
    save_mixed_store(path, &store)
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

impl ConnectItem {
    pub(crate) fn is_connected(
        &self,
        config: &crate::config::Config,
        auth: &BTreeMap<String, AuthEntry>,
    ) -> bool {
        // Custom is an action for adding an endpoint, not a saved provider.
        self.id != "custom"
            && (auth.contains_key(&self.id)
                || (normalize_custom_base_url(&config.base_url)
                    == normalize_custom_base_url(&self.base_url)
                    && (config.api_key.as_ref().is_some_and(|key| !key.is_empty())
                        || self.no_auth)))
    }
}

pub(crate) fn load_connect_auth() -> BTreeMap<String, AuthEntry> {
    auth_store_path()
        .map(|path| load_mixed_store(&path))
        .unwrap_or_default()
}

pub(crate) fn sort_connect_items(
    items: &mut [ConnectItem],
    config: &crate::config::Config,
    auth: &BTreeMap<String, AuthEntry>,
) {
    items.sort_by_key(|item| {
        if item.is_connected(config, auth) {
            0
        } else if item.id == "custom" {
            1
        } else {
            2
        }
    });
}

impl SavedConfig {
    /// Only reorder available IDs: history must not resurrect removed models
    /// or change the validation contract of the provider's live model list.
    pub(crate) fn sort_models(&self, base_url: &str, models: &mut [(String, String)]) {
        let base = normalize_custom_base_url(base_url);
        let current = self
            .base_url
            .as_deref()
            .filter(|url| normalize_custom_base_url(url) == base)
            .and(self.model.as_deref());
        let recent = self.recent_models.get(&base);
        models.sort_by_key(|(id, _)| {
            if current == Some(id.as_str()) {
                0
            } else {
                recent
                    .and_then(|ids| ids.iter().position(|m| m == id))
                    .map_or(usize::MAX, |rank| rank + 1)
            }
        });
    }
}

/// Trims a pasted custom base URL to the API root: strips whitespace,
/// trailing slashes, and route suffixes (`/chat/completions`, `/messages`,
/// `/models`) so `…/v1/chat/completions` becomes `…/v1`.
pub fn normalize_custom_base_url(raw: &str) -> String {
    let mut s = raw.trim().trim_end_matches('/').to_string();
    loop {
        let mut stripped: Option<String> = None;
        for suffix in ["/chat/completions", "/messages", "/models"] {
            if let Some(rest) = s.strip_suffix(suffix) {
                stripped = Some(rest.trim_end_matches('/').to_string());
                break;
            }
        }
        match stripped {
            Some(next) => s = next,
            None => break,
        }
    }
    s
}

/// Builds the full list of providers for the connect modal:
/// Default order: Custom, then popular, followed by all catalog providers.
/// The modal stably promotes connected providers above Custom.
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
            "commandcode",
            "CommandCode",
            "(API key)",
            "https://api.commandcode.ai/provider/v1",
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

    // Custom OpenAI/Anthropic-compatible endpoint (separator drawn under it).
    popular_ids.insert("custom".to_string());
    items.push(ConnectItem {
        id: "custom".to_string(),
        name: "Custom".to_string(),
        sublabel: "(OpenAI/Anthropic compatible)".to_string(),
        base_url: String::new(),
        no_auth: false,
    });

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

#[path = "catalog_tests.rs"]
#[cfg(test)]
mod tests;
