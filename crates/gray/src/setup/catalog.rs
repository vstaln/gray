use std::collections::BTreeMap;
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
    crate::setup::presets::preset_catalog()
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
    pub context_reserve: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_keep: Option<usize>,

    #[serde(rename = "compact_at_tokens", skip_serializing_if = "Option::is_none")]
    pub compact_at: Option<crate::setup::CompactAt>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub exec_prefix: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_auto: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_update: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub lean: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_compaction: Option<bool>,

    /// Resend cost (USD) at which a cold cache asks for a second Enter; 0 is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cold_confirm_usd: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_inline_bytes: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_warming: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolbox: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_title: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_progress: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_attribution: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction_model: Option<String>,

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

pub fn intro_enabled() -> bool {
    subsystem_enabled(|s| s.intro)
}

pub fn auto_update_enabled() -> bool {
    subsystem_enabled(|s| s.auto_update)
}

fn subsystem_enabled(get: impl Fn(&SavedConfig) -> Option<bool>) -> bool {
    saved_config_path()
        .map(|p| load_saved_config_at(&p))
        .map(|s| get(&s).unwrap_or(true))
        .unwrap_or(true)
}

pub(crate) fn saved_config_locked() -> SavedConfig {
    saved_config_path()
        .map(|path| saved_config_locked_at(&path))
        .unwrap_or_default()
}

pub(crate) fn saved_config_locked_at(path: &Path) -> SavedConfig {
    let _cfg_lock = lock_saved_config_at(path)
        .inspect_err(|e| log::warn!("cannot lock {} ({e:#})", path.display()))
        .ok();
    load_saved_config_at(path)
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
        context_reserve: opt_field(obj, "context_reserve"),
        context_keep: opt_field(obj, "context_keep"),
        compact_at: opt_field(obj, "compact_at_tokens"),
        exec_prefix: opt_field(obj, "exec_prefix"),
        memory_auto: opt_field(obj, "memory_auto"),
        auto_update: opt_field(obj, "auto_update"),
        lean: opt_field(obj, "lean"),
        background_compaction: opt_field(obj, "background_compaction"),
        cold_confirm_usd: opt_field(obj, "cold_confirm_usd"),
        tool_inline_bytes: opt_field(obj, "tool_inline_bytes"),
        cache_warming: opt_field(obj, "cache_warming"),
        toolbox: opt_field(obj, "kit").or_else(|| opt_field(obj, "toolbox")),
        terminal_title: opt_field(obj, "terminal_title"),
        terminal_progress: opt_field(obj, "terminal_progress"),
        commit_attribution: opt_field(obj, "commit_attribution"),
        compaction_model: opt_field(obj, "compaction_model"),
        intro: opt_field(obj, "intro"),
        theme: opt_field(obj, "theme"),
        status_line: opt_field(obj, "status_line"),
        provider_id: string_field(obj, "provider_id"),
        credential_source: string_field(obj, "credential_source"),
        auth_ref: string_field(obj, "auth_ref"),
        effort_memory: opt_field(obj, "effort_memory").unwrap_or_default(),
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

/// One labeled API key in a [`KeyPool`]. `label` is the row a pool panel
/// shows; `key` is the secret itself.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PooledKey {
    pub label: String,
    pub key: String,
    /// Unix seconds the cooldown ends; 0 = not cooling.
    #[serde(default)]
    pub cooldown_until: u64,
    #[serde(default)]
    pub last_used_at: u64,
}

impl std::fmt::Debug for PooledKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledKey")
            .field("label", &self.label)
            .field("cooldown_until", &self.cooldown_until)
            .field("last_used_at", &self.last_used_at)
            .finish_non_exhaustive()
    }
}

/// auth.json entry for an API-key provider holding several keys under the
/// provider id. Written only once a provider has a second key: a lone key
/// stays a plain [`AuthEntry::Key`] string, exactly as before.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPool {
    pub version: u8,
    #[serde(default)]
    pub selected: Option<String>,
    /// "" and "fill_first" both mean pool order; "least_used" takes the
    /// oldest use.
    #[serde(default)]
    pub strategy: String,
    pub keys: Vec<PooledKey>,
}

impl std::fmt::Debug for KeyPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyPool")
            .field("version", &self.version)
            .field("selected", &self.selected)
            .field("strategy", &self.strategy)
            .field("keys", &self.keys.len())
            .finish()
    }
}

impl KeyPool {
    /// Wrap one plain key as a one-key pool labelled `default`.
    pub fn from_key(key: &str) -> Self {
        Self {
            version: 1,
            selected: None,
            strategy: String::new(),
            keys: vec![PooledKey {
                label: "default".to_string(),
                key: key.to_string(),
                cooldown_until: 0,
                last_used_at: 0,
            }],
        }
    }

    /// The first free `key N` label, so adding never collides.
    pub fn next_label(&self) -> String {
        (1..)
            .map(|n| format!("key {n}"))
            .find(|label| self.keys.iter().all(|k| &k.label != label))
            .unwrap_or_else(|| format!("key {}", self.keys.len() + 1))
    }
}

/// One pool member, whichever pool kind it is in.
pub struct PoolSlot<'a> {
    pub label: &'a str,
    pub cooldown_until: u64,
    pub last_used_at: u64,
}

impl PoolSlot<'_> {
    fn usable(&self, now: u64) -> bool {
        self.cooldown_until <= now
    }
}

/// The serving pick over any labeled pool: the pinned member when it is
/// not cooling, else `least_used` (oldest use) or pool order. Shared by
/// plugin account pools and API key pools so both pick identically.
pub fn pick_from_pool<'a>(
    selected: Option<&str>,
    strategy: &str,
    slots: &'a [PoolSlot<'a>],
) -> Option<&'a PoolSlot<'a>> {
    if slots.is_empty() {
        return None;
    }
    let now = gray_core::time::now_secs();
    if let Some(sel) = selected
        && let Some(slot) = slots.iter().find(|s| s.label == sel && s.usable(now))
    {
        return Some(slot);
    }
    let mut it = slots.iter().filter(|s| s.usable(now));
    if strategy == "least_used" {
        return it.min_by_key(|s| s.last_used_at);
    }
    it.next()
}

impl KeyPool {
    /// The key a request should run as, by the shared pool rule.
    pub fn pick(&self) -> Option<&PooledKey> {
        let slots: Vec<PoolSlot<'_>> = self
            .keys
            .iter()
            .map(|k| PoolSlot {
                label: &k.label,
                cooldown_until: k.cooldown_until,
                last_used_at: k.last_used_at,
            })
            .collect();
        let picked = pick_from_pool(self.selected.as_deref(), &self.strategy, &slots)?;
        self.keys.iter().find(|k| k.label == picked.label)
    }

    /// The serving key, else the first when every key is cooling: a stale
    /// key is a better answer than none for read-only uses like showing
    /// "connected" or naming models.
    pub fn first_key(&self) -> Option<&str> {
        self.pick()
            .or_else(|| self.keys.first())
            .map(|k| k.key.as_str())
    }
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
    KeyPool(KeyPool),
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
            Self::KeyPool(pool) => f.debug_tuple("KeyPool").field(pool).finish(),
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
            AuthEntry::KeyPool(pool) => pool.first_key().map(|key| (k, key.to_string())),
            AuthEntry::OAuth(_) | AuthEntry::Plugin(_) | AuthEntry::PluginPool(_) => None,
        })
        .collect()
}

/// Every key a provider holds, in pool order: the one entry a plain string
/// gives, or one line per pooled key. Secrets for a pool panel and nothing
/// else.
pub fn load_auth_key_pool(pid: &str) -> Vec<PooledKey> {
    let Ok(path) = auth_store_path() else {
        return Vec::new();
    };
    match load_mixed_store(&path).remove(pid) {
        Some(AuthEntry::Key(key)) => vec![PooledKey {
            label: "default".to_string(),
            key,
            cooldown_until: 0,
            last_used_at: 0,
        }],
        Some(AuthEntry::KeyPool(pool)) => pool.keys,
        _ => Vec::new(),
    }
}

/// The auth.json id a config's API key lives under: the provider id when one
/// is set, else the `custom:<slug>` id of its base URL.
pub fn key_pool_id(config: &crate::config::Config) -> String {
    if config.provider_id.is_empty() {
        custom_key_id(&config.base_url)
    } else {
        config.provider_id.clone()
    }
}

pub(crate) fn save_auth_key(pid: &str, key: &str) -> anyhow::Result<()> {
    let path = auth_store_path()?;
    save_auth_key_at(&path, pid, key)
}

/// [`add_auth_key_at`] against the live auth store.
pub(crate) fn add_auth_key(pid: &str, key: &str) -> anyhow::Result<()> {
    let path = auth_store_path()?;
    add_auth_key_at(&path, pid, key)
}

pub(crate) fn save_auth_key_at(path: &Path, pid: &str, key: &str) -> anyhow::Result<()> {
    let _lock = crate::auth::CredentialStore::new(path.to_path_buf()).lock()?;
    let mut store = load_mixed_store_strict(path)?;
    store.insert(pid.to_string(), AuthEntry::Key(key.to_string()));
    save_mixed_store(path, &store)
}

/// Store `key` under `pid`, appending it to whatever the provider already
/// holds and pinning it. The first key keeps the plain-string shape; the
/// second turns the entry into a [`KeyPool`] with the old key labelled
/// `default`.
pub(crate) fn add_auth_key_at(path: &Path, pid: &str, key: &str) -> anyhow::Result<()> {
    let _lock = crate::auth::CredentialStore::new(path.to_path_buf()).lock()?;
    let mut store = load_mixed_store_strict(path)?;
    let mut pool = match store.remove(pid) {
        Some(AuthEntry::Key(existing)) => KeyPool::from_key(&existing),
        Some(AuthEntry::KeyPool(pool)) => pool,
        Some(other) => {
            store.insert(pid.to_string(), other);
            anyhow::bail!("{pid} holds a non-API-key credential");
        }
        None => {
            store.insert(pid.to_string(), AuthEntry::Key(key.to_string()));
            save_mixed_store(path, &store)?;
            return Ok(());
        }
    };
    if pool.keys.iter().any(|k| k.key == key) {
        pool.selected = Some(
            pool.keys
                .iter()
                .find(|k| k.key == key)
                .map(|k| k.label.clone())
                .unwrap_or_default(),
        );
        store.insert(pid.to_string(), AuthEntry::KeyPool(pool));
        return save_mixed_store(path, &store);
    }
    let label = pool.next_label();
    pool.keys.push(PooledKey {
        label: label.clone(),
        key: key.to_string(),
        cooldown_until: 0,
        last_used_at: 0,
    });
    pool.selected = Some(label);
    store.insert(pid.to_string(), AuthEntry::KeyPool(pool));
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
    use crate::providers::registry::{
        LEGACY_CHATGPT_AUTH, LEGACY_CHATGPT_PROVIDER, ProviderRegistry,
    };
    let Ok(path) = auth_store_path() else {
        return BTreeMap::new();
    };
    let auth = load_mixed_store(&path);
    if auth.contains_key(LEGACY_CHATGPT_AUTH)
        && let Ok(home) = gray_home()
        && let Some(installed) = ProviderRegistry::load_cached(&home)
            .resolve(LEGACY_CHATGPT_PROVIDER, LEGACY_CHATGPT_AUTH)
        && installed.is_bundled_chatgpt_plan()
        && let Ok(()) =
            crate::auth::CredentialStore::new(path.clone()).migrate_chatgpt_plan(&installed)
    {
        return load_mixed_store(&path);
    }
    auth
}

pub(crate) struct Detected {
    pub note: String,
    pub env: Option<(String, String)>,
}

type ProviderActivity = BTreeMap<u64, BTreeMap<String, u64>>;

fn activity_day() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400
}

fn load_provider_activity(path: &Path) -> ProviderActivity {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn provider_activity_key(id: &str, base_url: &str, plugin: bool) -> String {
    if plugin {
        return id.to_owned();
    }
    use sha2::{Digest, Sha256};
    let base = normalize_custom_base_url(base_url);
    if base.is_empty() {
        return String::new();
    }
    format!("{:x}", Sha256::digest(base.as_bytes()))
}

pub(crate) fn record_provider_turn(config: &crate::config::Config) {
    let Ok(home) = gray_home() else { return };
    let path = home.join("provider-activity.json");
    let key = provider_activity_key(
        &config.provider_id,
        &config.base_url,
        config.credential_source == "plugin",
    );
    if key.is_empty() {
        return;
    }
    let Ok(_lock) = lock_saved_config_at(&path) else {
        return;
    };
    let day = activity_day();
    let mut history = load_provider_activity(&path);
    history.retain(|date, _| (day.saturating_sub(29)..=day).contains(date));
    let count = history.entry(day).or_default().entry(key).or_default();
    *count = count.saturating_add(1);
    if let Ok(value) = serde_json::to_value(history) {
        let _ = save_private_json(&path, &value);
    }
}

pub(crate) fn provider_activity() -> BTreeMap<String, u64> {
    let Ok(home) = gray_home() else {
        return BTreeMap::new();
    };
    let day = activity_day();
    let history = load_provider_activity(&home.join("provider-activity.json"));
    let mut totals = BTreeMap::<String, u64>::new();
    for (_, counts) in history.range(day.saturating_sub(29)..=day) {
        for (key, count) in counts {
            let total = totals.entry(key.clone()).or_default();
            *total = total.saturating_add(*count);
        }
    }
    totals
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
            || crate::setup::presets::is_popular_preset(&item.id)
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
        name: "Gray".to_string(),
        sublabel: "(models hosted by Gray)".to_string(),
        base_url: gray_portal_base_url(),
        no_auth: false,
        auth: ConnectAuth::ApiKey,
    });

    crate::setup::presets::append_connect_presets(&mut items, catalog);

    items
}
