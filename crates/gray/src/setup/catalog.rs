use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::providers::registry::InstalledProvider;

use serde::{Deserialize, Serialize};

use gray_core::credential::{CredentialEnvelope, CredentialMaterial};

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogProvider {
    pub name: String,
    pub base_url: String,

    #[serde(default)]
    pub no_auth: bool,
}

pub type Catalog = BTreeMap<String, CatalogProvider>;

pub fn load_catalog() -> anyhow::Result<Catalog> {
    crate::quality_of_life::providers::preset_catalog()
}

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

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SavedConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub recent_models: BTreeMap<String, Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_effort: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_reasoning: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<bool>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_parts: BTreeMap<String, String>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts_memory: BTreeMap<String, BTreeMap<String, String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills_auto: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_reserve: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_keep: Option<usize>,

    #[serde(rename = "compact_at_tokens", skip_serializing_if = "Option::is_none")]
    pub compact_at: Option<crate::setup::CompactAt>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec_prefix: Option<String>,

    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub disabled_skills: BTreeSet<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_auto: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub cron_auto: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub gw_auto: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_update: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub lean: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_compaction: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub intro: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_line: Option<crate::statusline::StatusLineConfig>,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_id: String,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub credential_source: String,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub auth_ref: String,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub effort_memory: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "qol")]
    pub quality_of_life: Option<bool>,
}

pub const AUTH_MODE_API_KEY: &str = "api_key";
pub const AUTH_MODE_NONE: &str = "none";

pub fn gray_home() -> anyhow::Result<PathBuf> {
    gray_core::paths::gray_home().ok_or_else(|| {
        anyhow::anyhow!("cannot resolve home: set GRAY_HOME or the platform user profile")
    })
}

pub fn saved_config_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("config.json"))
}

pub fn disabled_skill_names() -> BTreeSet<String> {
    saved_config_path()
        .map(|p| load_saved_config_at(&p).disabled_skills)
        .unwrap_or_default()
}

pub fn skills_auto_enabled() -> bool {
    saved_config_path()
        .map(|p| load_saved_config_at(&p).skills_auto.unwrap_or(true))
        .unwrap_or(true)
}

pub fn skills_auto_enabled_at(path: &Path) -> bool {
    load_saved_config_at(path).skills_auto.unwrap_or(true)
}

pub fn gw_auto_enabled() -> bool {
    subsystem_enabled(|s| s.gw_auto)
}

pub fn gw_auto_enabled_at(path: &Path) -> bool {
    subsystem_enabled_at(path, |s| s.gw_auto)
}

pub fn intro_enabled() -> bool {
    subsystem_enabled(|s| s.intro)
}

pub fn auto_update_enabled() -> bool {
    subsystem_enabled(|s| s.auto_update)
}

pub fn quality_of_life_enabled() -> bool {
    subsystem_enabled(|s| s.quality_of_life)
}

pub fn quality_of_life_enabled_at(path: &Path) -> bool {
    subsystem_enabled_at(path, |s| s.quality_of_life)
}

pub fn auto_update_enabled_at(path: &Path) -> bool {
    subsystem_enabled_at(path, |s| s.auto_update)
}

fn subsystem_enabled(get: impl Fn(&SavedConfig) -> Option<bool>) -> bool {
    saved_config_path()
        .map(|p| load_saved_config_at(&p))
        .map(|s| get(&s).unwrap_or(true))
        .unwrap_or(true)
}

fn subsystem_enabled_at(path: &Path, get: impl Fn(&SavedConfig) -> Option<bool>) -> bool {
    get(&load_saved_config_at(path)).unwrap_or(true)
}

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

fn warn_bad_config(path: &Path, detail: &dyn std::fmt::Display) {
    let msg = format!(
        "cannot load {} config ({}); using defaults",
        path.display(),
        detail
    );
    eprintln!("warning: {msg}");
    log::warn!(target: "gray_config", "{msg}");
}

fn partial_saved_config(obj: &serde_json::Map<String, serde_json::Value>) -> SavedConfig {
    SavedConfig {
        base_url: opt_field(obj, "base_url"),
        api_key: opt_field(obj, "api_key"),
        model: opt_field(obj, "model"),
        recent_models: opt_field(obj, "recent_models").unwrap_or_default(),
        auth_mode: opt_field(obj, "auth_mode"),
        thinking_effort: opt_field(obj, "thinking_effort"),
        show_reasoning: opt_field(obj, "show_reasoning"),
        fast_mode: opt_field(obj, "fast_mode"),
        model_parts: opt_field(obj, "model_parts").unwrap_or_default(),
        parts_memory: opt_field(obj, "parts_memory").unwrap_or_default(),
        temperature: opt_field(obj, "temperature"),
        top_p: opt_field(obj, "top_p"),
        context_window: opt_field(obj, "context_window"),
        skills_auto: opt_field(obj, "skills_auto"),
        context_reserve: opt_field(obj, "context_reserve"),
        context_keep: opt_field(obj, "context_keep"),
        compact_at: opt_field(obj, "compact_at_tokens"),
        exec_prefix: opt_field(obj, "exec_prefix"),
        disabled_skills: opt_field(obj, "disabled_skills").unwrap_or_default(),
        memory_auto: opt_field(obj, "memory_auto"),
        cron_auto: opt_field(obj, "cron_auto"),
        gw_auto: opt_field(obj, "gw_auto"),
        auto_update: opt_field(obj, "auto_update"),
        lean: opt_field(obj, "lean"),
        background_compaction: opt_field(obj, "background_compaction"),
        intro: opt_field(obj, "intro"),
        theme: opt_field(obj, "theme"),
        status_line: opt_field(obj, "status_line"),
        provider_id: string_field(obj, "provider_id"),
        credential_source: string_field(obj, "credential_source"),
        auth_ref: string_field(obj, "auth_ref"),
        effort_memory: opt_field(obj, "effort_memory").unwrap_or_default(),
        quality_of_life: opt_field(obj, "quality_of_life").or_else(|| opt_field(obj, "qol")),
    }
}

fn string_field(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> String {
    opt_field::<String>(obj, key).unwrap_or_default()
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

const CONFIG_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub struct SavedConfigLock {
    _file: Option<std::fs::File>,
}

pub fn lock_saved_config_at(path: &Path) -> anyhow::Result<SavedConfigLock> {
    let lock_path = path.with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    let deadline = std::time::Instant::now() + CONFIG_LOCK_TIMEOUT;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(SavedConfigLock { _file: Some(f) }),
            Err(std::fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "another gray process is writing the config ({}); try again",
                        lock_path.display()
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                log::warn!(
                    "config locking unsupported on {} ({e}); proceeding unlocked",
                    lock_path.display()
                );
                return Ok(SavedConfigLock { _file: None });
            }
        }
    }
}

pub fn save_saved_config_at(path: &Path, cfg: &SavedConfig) -> anyhow::Result<()> {
    let previous = load_saved_config_at(path);
    let mut persisted = cfg.clone();

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

pub(crate) fn save_private_json(path: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
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

pub(crate) fn auth_store_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("auth.json"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectAuth {
    None,
    ApiKey,
    Plugin {
        auth_ref: String,
        profile_binding: String,
    },
}

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

/// One labeled credential in a v2 plugin pool. `accounts` in
/// [`PluginAuthPool`] is a vec: file order is the `fill_first` priority
/// order, matching the plugin-owned pools.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAccount {
    pub label: String,
    pub profile_binding: String,
    pub credential: CredentialMaterial,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default)]
    pub added_at: u64,
    /// Unix seconds the cooldown ends; 0 = not cooling.
    #[serde(default)]
    pub cooldown_until: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default)]
    pub request_count: u64,
    #[serde(default)]
    pub last_used_at: u64,
}

/// auth.json v2 plugin entry: several labeled credential envelopes under
/// one `plugin:<lock>:<provider>:<method>` auth_ref. `selected` pins an
/// account; `strategy` orders the rest (`fill_first`, `least_used`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAuthPool {
    pub version: u8,
    pub plugin: String,
    pub provider: String,
    pub auth_method: String,
    #[serde(default)]
    pub selected: Option<String>,
    #[serde(default)]
    pub strategy: String,
    pub accounts: Vec<PluginAccount>,
}

impl PluginAuthPool {
    /// Wrap a v1 single-credential entry as a one-account pool.
    pub fn from_envelope(value: &CredentialEnvelope) -> Self {
        Self {
            version: 2,
            plugin: value.plugin.clone(),
            provider: value.provider.clone(),
            auth_method: value.auth_method.clone(),
            selected: Some("default".to_string()),
            strategy: "fill_first".to_string(),
            accounts: vec![PluginAccount {
                label: "default".to_string(),
                profile_binding: value.profile_binding.clone(),
                credential: value.credential.clone(),
                email: None,
                added_at: 0,
                cooldown_until: 0,
                last_error: None,
                request_count: 0,
                last_used_at: 0,
            }],
        }
    }

    /// The pool entry as one envelope, for callers that still want a
    /// single credential (the serving pick is the account's material).
    pub fn envelope_for(&self, account: &PluginAccount) -> CredentialEnvelope {
        CredentialEnvelope {
            version: 1,
            plugin: self.plugin.clone(),
            provider: self.provider.clone(),
            auth_method: self.auth_method.clone(),
            profile_binding: account.profile_binding.clone(),
            credential: account.credential.clone(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AuthEntry {
    Key(String),
    OAuth(StoredAuth),
    Plugin(CredentialEnvelope),
    PluginPool(PluginAuthPool),
}

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
            Self::Plugin(plugin) => f.debug_tuple("Plugin").field(plugin).finish(),
            Self::PluginPool(pool) => f
                .debug_struct("PluginPool")
                .field("provider", &pool.provider)
                .field("accounts", &pool.accounts.len())
                .finish(),
        }
    }
}

pub fn load_mixed_store(path: &Path) -> BTreeMap<String, AuthEntry> {
    load_mixed_store_strict(path).unwrap_or_default()
}

pub fn load_mixed_store_strict(path: &Path) -> anyhow::Result<BTreeMap<String, AuthEntry>> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Ok(BTreeMap::new());
    };
    if let Ok(single) = serde_json::from_str::<StoredAuth>(&body) {
        let mut map = BTreeMap::new();
        map.insert(single.provider.clone(), AuthEntry::OAuth(single));
        return Ok(map);
    }
    serde_json::from_str::<BTreeMap<String, AuthEntry>>(&body).map_err(|e| {
        anyhow::anyhow!(
            "{} is not valid JSON ({}); fix or remove it before changing credentials",
            path.display(),
            e
        )
    })
}

pub fn save_mixed_store(path: &Path, store: &BTreeMap<String, AuthEntry>) -> anyhow::Result<()> {
    save_private_json(
        path,
        &serde_json::to_value(store).map_err(|e| anyhow::anyhow!("{e}"))?,
    )
}

pub fn load_auth_keys() -> BTreeMap<String, String> {
    let path = auth_store_path().unwrap_or_else(|_| PathBuf::from("/dev/null"));
    load_mixed_store(&path)
        .into_iter()
        .filter_map(|(k, v)| match v {
            AuthEntry::Key(key) => Some((k, key)),
            AuthEntry::OAuth(_) | AuthEntry::Plugin(_) | AuthEntry::PluginPool(_) => None,
        })
        .collect()
}

pub(crate) fn save_auth_key(pid: &str, key: &str) -> anyhow::Result<()> {
    let path = auth_store_path()?;
    save_auth_key_at(&path, pid, key)
}

pub(crate) fn save_auth_key_at(path: &Path, pid: &str, key: &str) -> anyhow::Result<()> {
    let _lock = crate::auth::CredentialStore::new(path.to_path_buf()).lock()?;
    let mut store = load_mixed_store_strict(path)?;
    store.insert(pid.to_string(), AuthEntry::Key(key.to_string()));
    save_mixed_store(path, &store)
}

pub(crate) fn remove_auth_entry_at(path: &Path, pid: &str) -> anyhow::Result<()> {
    let _lock = crate::auth::CredentialStore::new(path.to_path_buf()).lock()?;
    let mut store = load_mixed_store_strict(path)?;
    store.remove(pid);
    save_mixed_store(path, &store)
}

#[derive(Debug, Clone)]
pub struct ConnectItem {
    pub id: String,
    pub name: String,
    pub sublabel: String,
    pub base_url: String,
    pub no_auth: bool,
    pub auth: ConnectAuth,
}

impl ConnectItem {
    pub(crate) fn is_connected(
        &self,
        config: &crate::config::Config,
        auth: &BTreeMap<String, AuthEntry>,
    ) -> bool {
        (self.id != "custom"
            && match &self.auth {
                ConnectAuth::Plugin { auth_ref, .. } => {
                    auth.contains_key(auth_ref)
                        || (config.credential_source == "plugin" && config.provider_id == self.id)
                }
                ConnectAuth::ApiKey => auth.contains_key(&self.id),
                ConnectAuth::None => false,
            })
            || (normalize_custom_base_url(&config.base_url)
                == normalize_custom_base_url(&self.base_url)
                && (config.api_key.as_ref().is_some_and(|key| !key.is_empty()) || self.no_auth))
    }
}

pub(crate) fn load_connect_auth() -> BTreeMap<String, AuthEntry> {
    auth_store_path()
        .map(|path| load_mixed_store(&path))
        .unwrap_or_default()
}

pub(crate) struct Detected {
    pub note: String,
    pub env: Option<(String, String)>,
}

pub(crate) fn sort_connect_items(
    items: &mut [ConnectItem],
    config: &crate::config::Config,
    auth: &BTreeMap<String, AuthEntry>,
    detected: &BTreeMap<String, Detected>,
) {
    items.sort_by_key(|item| {
        if item.id == "custom" {
            0
        } else if item.is_connected(config, auth) {
            1
        } else if detected.contains_key(&item.id) {
            2
        } else if item.id.starts_with("custom:")
            || matches!(item.auth, ConnectAuth::Plugin { .. })
            || crate::quality_of_life::providers::is_popular_preset(&item.id)
        {
            3
        } else {
            4
        }
    });
}

impl SavedConfig {
    pub(crate) fn remembered_effort(&self, key: &str) -> Option<String> {
        self.effort_memory.get(key).cloned()
    }

    pub(crate) fn remember_effort(&mut self, key: &str, effort: &str) {
        self.effort_memory
            .insert(key.to_string(), effort.to_string());
        self.thinking_effort = Some(effort.to_string());
    }

    pub(crate) fn remember_parts(&mut self, key: &str, parts: &BTreeMap<String, String>) {
        if !parts.is_empty() {
            self.parts_memory.insert(key.to_string(), parts.clone());
        }
        self.model_parts = parts.clone();
    }

    pub(crate) fn sort_models(&self, base_url: &str, models: &mut [(String, String)]) {
        let base = normalize_custom_base_url(base_url);
        let current = self
            .base_url
            .as_deref()
            .filter(|url| normalize_custom_base_url(url) == base)
            .and(self.model.as_deref());
        let recent = self.recent_models.get(&base);
        models.sort_by_key(|(id, _)| {
            let composite = super::variants::row_shape(id).is_some_and(|s| s.is_composite());
            let rank = if current == Some(id.as_str()) {
                0
            } else {
                recent
                    .and_then(|ids| ids.iter().position(|m| m == id))
                    .map_or(usize::MAX, |rank| rank + 1)
            };
            (!composite, rank)
        });
    }
}

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

pub fn plugin_models_key(provider_id: &str) -> String {
    format!("plugin:{provider_id}")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomEndpoint {
    pub name: String,

    pub base_url: String,

    #[serde(default)]
    pub keyless: bool,
}

pub(crate) fn custom_endpoints_path() -> anyhow::Result<PathBuf> {
    Ok(gray_home()?.join("custom_endpoints.json"))
}

pub fn custom_key_id(base_url: &str) -> String {
    format!("custom:{}", custom_slug(base_url))
}

pub fn is_custom_id(id: &str) -> bool {
    id == "custom" || id.starts_with("custom:")
}

fn custom_slug(base_url: &str) -> String {
    let normalized = normalize_custom_base_url(base_url);
    let Ok(url) = reqwest::Url::parse(&normalized) else {
        return normalized
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':') {
                    c
                } else {
                    '-'
                }
            })
            .collect();
    };
    let mut slug = url.host_str().unwrap_or_default().to_string();
    if let Some(port) = url.port() {
        slug.push(':');
        slug.push_str(&port.to_string());
    }
    let path = url.path().trim_matches('/');
    if !path.is_empty() && !matches!(path, "v1" | "v1beta" | "api/v1") {
        slug.push('/');
        slug.push_str(&path.replace('/', "-"));
    }
    slug
}

pub fn custom_display_name(base_url: &str) -> String {
    let normalized = normalize_custom_base_url(base_url);
    for ep in &crate::setup::context::provider_meta().local_endpoints {
        if normalize_custom_base_url(&ep.url) == normalized {
            return ep.name.clone();
        }
    }
    reqwest::Url::parse(&normalized)
        .ok()
        .map(|u| match (u.host_str(), u.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            _ => "Custom".to_string(),
        })
        .unwrap_or_else(|| "Custom".to_string())
}

pub fn load_custom_endpoints() -> Vec<CustomEndpoint> {
    let path = custom_endpoints_path().unwrap_or_else(|_| PathBuf::from("/dev/null"));
    load_custom_endpoints_at(&path)
}

pub(crate) fn load_custom_endpoints_at(path: &Path) -> Vec<CustomEndpoint> {
    let Ok(body) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<CustomEndpoint>>(&body).unwrap_or_default()
}

fn save_custom_endpoints_at(path: &Path, eps: &[CustomEndpoint]) -> anyhow::Result<()> {
    save_private_json(path, &serde_json::to_value(eps)?)
}

pub(crate) fn upsert_custom_endpoint_at(
    path: &Path,
    base_url: &str,
    keyless: bool,
) -> anyhow::Result<CustomEndpoint> {
    let base = normalize_custom_base_url(base_url);
    let mut eps = load_custom_endpoints_at(path);
    let ep = if let Some(existing) = eps
        .iter_mut()
        .find(|e| normalize_custom_base_url(&e.base_url) == base)
    {
        existing.name = custom_display_name(&base);
        existing.keyless = keyless;
        existing.clone()
    } else {
        let ep = CustomEndpoint {
            name: custom_display_name(&base),
            base_url: base.clone(),
            keyless,
        };
        eps.push(ep.clone());
        ep
    };
    save_custom_endpoints_at(path, &eps)?;
    Ok(ep)
}

pub(crate) fn remove_custom_endpoint_at(path: &Path, base_url: &str) -> anyhow::Result<bool> {
    let base = normalize_custom_base_url(base_url);
    let mut eps = load_custom_endpoints_at(path);
    let before = eps.len();
    eps.retain(|e| normalize_custom_base_url(&e.base_url) != base);
    if eps.len() == before {
        return Ok(false);
    }
    save_custom_endpoints_at(path, &eps)?;
    Ok(true)
}

pub fn provider_effort_key(provider_id: &str, base_url: &str) -> String {
    if provider_id.trim().is_empty() {
        format!("url:{}", normalize_custom_base_url(base_url))
    } else {
        plugin_models_key(provider_id)
    }
}

pub fn effort_memory_key(provider_id: &str, base_url: &str, model: &str) -> String {
    let scope = provider_effort_key(provider_id, base_url);
    let model = model.trim();
    if model.is_empty() {
        scope
    } else {
        format!("{scope}|{model}")
    }
}

pub const GRAY_PORTAL_ID: &str = "gray-portal";

/// OpenAI-compatible endpoint for models hosted by the Gray registry.
pub(crate) fn gray_portal_base_url() -> String {
    std::env::var("GRAY_PORTAL_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "https://api-gray.alignment.id/api/portal/v1".to_string())
}

/// The account token written by `gray login` — doubles as the portal key.
pub(crate) fn load_registry_token() -> Option<String> {
    let path = gray_home().ok()?.join("registry-token.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

pub fn build_connect_items(
    catalog: &Catalog,
    providers: &[InstalledProvider],
    customs: &[CustomEndpoint],
) -> Vec<ConnectItem> {
    let mut items = Vec::new();
    for installed in providers {
        let sublabel = if installed.auth_method.name.trim().is_empty() {
            "(plugin provider)".to_string()
        } else {
            format!("({})", installed.auth_method.name)
        };
        items.push(ConnectItem {
            id: installed.provider_id(),
            name: installed.provider.name.clone(),
            sublabel,
            base_url: installed.provider.transport.base_url.to_string(),
            no_auth: false,
            auth: ConnectAuth::Plugin {
                auth_ref: installed.auth_ref(),
                profile_binding: installed.profile_binding.clone(),
            },
        });
    }

    for ep in customs {
        items.push(ConnectItem {
            id: custom_key_id(&ep.base_url),
            name: ep.name.clone(),
            sublabel: format!("({})", ep.base_url),
            base_url: ep.base_url.clone(),
            no_auth: ep.keyless,
            auth: if ep.keyless {
                ConnectAuth::None
            } else {
                ConnectAuth::ApiKey
            },
        });
    }

    items.push(ConnectItem {
        id: "custom".to_string(),
        name: "Custom".to_string(),
        sublabel: "(any OpenAI-compatible base URL)".to_string(),
        base_url: String::new(),
        no_auth: false,
        auth: ConnectAuth::ApiKey,
    });

    items.push(ConnectItem {
        id: GRAY_PORTAL_ID.to_string(),
        name: "Gray portal".to_string(),
        sublabel: "(models hosted by Gray)".to_string(),
        base_url: gray_portal_base_url(),
        no_auth: false,
        auth: ConnectAuth::ApiKey,
    });

    crate::quality_of_life::providers::append_connect_presets(&mut items, catalog);

    items
}

#[path = "catalog_tests.rs"]
#[cfg(test)]
mod tests;
