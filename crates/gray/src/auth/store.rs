use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gray_core::credential::{CredentialEnvelope, CredentialMaterial};

use crate::setup::catalog::{self, AuthEntry, PluginAccount, PluginAuthPool};

pub type StoredCredential = CredentialEnvelope;

#[derive(Clone, Debug)]
pub struct CredentialStore {
    path: PathBuf,
}

pub struct AuthLock {
    _file: File,
}

impl CredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> anyhow::Result<BTreeMap<String, AuthEntry>> {
        validate_path(&self.path)?;
        let mut map = catalog::load_mixed_store_strict(&self.path)?;
        // v1 plugin entries migrate to single-account pools in memory;
        // the next write persists the v2 shape (with a .v1.bak backup —
        // see `backup_on_migrate`).
        for entry in map.values_mut() {
            if let AuthEntry::Plugin(env) = entry.clone() {
                *entry = AuthEntry::PluginPool(PluginAuthPool::from_envelope(&env));
            }
        }
        Ok(map)
    }

    /// Rewrite any on-disk v1 plugin entries as v2 pools. Backs the file
    /// up to `auth.json.v1.bak` first; a second call is a no-op.
    pub fn migrate_to_v2(&self) -> anyhow::Result<bool> {
        let _lock = self.lock()?;
        let raw = match std::fs::read_to_string(&self.path) {
            Ok(body) => body,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if !raw.contains("\"plugin:") {
            return Ok(false);
        }
        // Only act when the raw JSON still holds single-envelope entries:
        // a `plugin:*` object with `credential` but no `accounts` is v1.
        let Ok(values) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&raw) else {
            return Ok(false);
        };
        let has_v1 = values.iter().any(|(k, v)| {
            k.starts_with("plugin:")
                && v.is_object()
                && v.get("credential").is_some()
                && v.get("accounts").is_none()
        });
        if !has_v1 {
            return Ok(false);
        }
        self.backup_v1()?;
        let map = self.load()?;
        self.replace_locked(&map)?;
        Ok(true)
    }

    /// `auth.json` → `auth.json.v1.bak`, once (an existing backup is kept).
    fn backup_v1(&self) -> anyhow::Result<()> {
        let bak = self.path.with_file_name("auth.json.v1.bak");
        if !bak.exists() {
            std::fs::copy(&self.path, &bak)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&bak, std::fs::Permissions::from_mode(0o600));
            }
        }
        Ok(())
    }

    pub fn replace(&self, next: &BTreeMap<String, AuthEntry>) -> anyhow::Result<()> {
        let _lock = self.lock()?;
        self.replace_locked(next)
    }

    pub(crate) fn replace_locked(&self, next: &BTreeMap<String, AuthEntry>) -> anyhow::Result<()> {
        // First write that persists a migrated pool: keep a backup of the
        // v1 file so nothing is lost.
        if next.values().any(|e| matches!(e, AuthEntry::PluginPool(_)))
            && let Ok(raw) = std::fs::read_to_string(&self.path)
            && raw.contains("\"plugin:")
        {
            if let Ok(values) = serde_json::from_str::<BTreeMap<String, serde_json::Value>>(&raw)
                && values.iter().any(|(k, v)| {
                    k.starts_with("plugin:")
                        && v.get("credential").is_some()
                        && v.get("accounts").is_none()
                })
            {
                let _ = self.backup_v1();
            }
        }
        let value = serde_json::to_value(next)?;
        if serde_json::to_vec(&value)?.len() > MAX_AUTH_BYTES {
            anyhow::bail!("auth store exceeds the maximum size");
        }
        validate_path(&self.path)?;
        catalog::save_private_json(&self.path, &value)
    }

    pub fn put_plugin(
        &self,
        value: StoredCredential,
    ) -> anyhow::Result<BTreeMap<String, AuthEntry>> {
        let auth_ref = plugin_auth_ref(&value)?;
        let _lock = self.lock()?;
        let mut store = self.load()?;
        match store.get_mut(&auth_ref) {
            // Re-login refreshes the pinned (or sole) account's material
            // instead of clobbering sibling accounts.
            Some(AuthEntry::PluginPool(pool)) => {
                let label = pool.selected.clone().unwrap_or_else(|| "default".into());
                match pool.accounts.iter_mut().find(|a| a.label == label) {
                    Some(a) => {
                        a.profile_binding = value.profile_binding.clone();
                        a.credential = value.credential.clone();
                        a.cooldown_until = 0;
                        a.last_error = None;
                    }
                    None => pool.accounts.push(PluginAccount {
                        label,
                        profile_binding: value.profile_binding.clone(),
                        credential: value.credential.clone(),
                        email: None,
                        added_at: now_secs(),
                        cooldown_until: 0,
                        last_error: None,
                        request_count: 0,
                        last_used_at: 0,
                    }),
                }
            }
            _ => {
                store.insert(auth_ref, AuthEntry::Plugin(value));
            }
        }
        self.replace_locked(&store)?;
        Ok(store)
    }

    /// Add (or replace) a labeled account under a plugin auth_ref. The
    /// first account added becomes the pin, matching the plugin pools.
    pub fn put_plugin_account(
        &self,
        value: StoredCredential,
        label: &str,
    ) -> anyhow::Result<BTreeMap<String, AuthEntry>> {
        let auth_ref = plugin_auth_ref(&value)?;
        anyhow::ensure!(!label.is_empty(), "account label is empty");
        let _lock = self.lock()?;
        let mut store = self.load()?;
        let account = PluginAccount {
            label: label.to_string(),
            profile_binding: value.profile_binding.clone(),
            credential: value.credential.clone(),
            email: None,
            added_at: now_secs(),
            cooldown_until: 0,
            last_error: None,
            request_count: 0,
            last_used_at: 0,
        };
        match store.get_mut(&auth_ref) {
            Some(AuthEntry::PluginPool(pool)) => {
                match pool.accounts.iter_mut().find(|a| a.label == label) {
                    Some(a) => {
                        a.profile_binding = account.profile_binding.clone();
                        a.credential = account.credential.clone();
                        a.cooldown_until = 0;
                        a.last_error = None;
                    }
                    None => pool.accounts.push(account),
                }
                if pool.selected.is_none() {
                    pool.selected = Some(label.to_string());
                }
            }
            _ => {
                store.insert(
                    auth_ref,
                    AuthEntry::PluginPool(PluginAuthPool {
                        version: 2,
                        plugin: value.plugin.clone(),
                        provider: value.provider.clone(),
                        auth_method: value.auth_method.clone(),
                        selected: Some(label.to_string()),
                        strategy: "fill_first".to_string(),
                        accounts: vec![account],
                    }),
                );
            }
        }
        self.replace_locked(&store)?;
        Ok(store)
    }

    /// The credential a caller sees: the pool's serving pick, or the
    /// first account when every one is cooling (a stale credential is a
    /// better answer than none — the caller's request fails upstream with
    /// the real verdict, and connected-state reads stay honest).
    pub fn read_plugin(&self, auth_ref: &str) -> anyhow::Result<Option<StoredCredential>> {
        Ok(self.load()?.get(auth_ref).and_then(|entry| match entry {
            AuthEntry::Plugin(value) => Some(value.clone()),
            AuthEntry::PluginPool(pool) => pick_account(pool)
                .map(|(_, a)| a)
                .or_else(|| pool.accounts.first())
                .map(|a| pool.envelope_for(a)),
            _ => None,
        }))
    }

    /// The account a request should run as: `Some((label, envelope))` —
    /// the pinned or strategy-picked usable account. `None` when there is
    /// no entry, the pool is empty, or every account is cooling (use
    /// [`Self::pool_retry_hint`] for the error message).
    pub fn serving_plugin(
        &self,
        auth_ref: &str,
    ) -> anyhow::Result<Option<(String, StoredCredential)>> {
        let store = self.load()?;
        let Some(entry) = store.get(auth_ref) else {
            return Ok(None);
        };
        Ok(match entry {
            AuthEntry::Plugin(value) => Some(("default".to_string(), value.clone())),
            AuthEntry::PluginPool(pool) => {
                pick_account(pool).map(|(label, a)| (label.to_string(), pool.envelope_for(a)))
            }
            _ => None,
        })
    }

    /// Account rows for the `/accounts` panel.
    pub fn plugin_account_rows(&self, auth_ref: &str) -> anyhow::Result<Vec<PluginAccountRow>> {
        let store = self.load()?;
        let now = now_secs();
        Ok(match store.get(auth_ref) {
            Some(AuthEntry::PluginPool(pool)) => pool
                .accounts
                .iter()
                .map(|a| PluginAccountRow {
                    label: a.label.clone(),
                    email: a.email.clone(),
                    cooling: a.cooldown_until > now,
                    cooldown_until: a.cooldown_until,
                    last_error: a.last_error.clone(),
                    request_count: a.request_count,
                    last_used_at: a.last_used_at,
                    pinned: pool.selected.as_deref() == Some(a.label.as_str()),
                })
                .collect(),
            Some(AuthEntry::Plugin(_)) => Vec::new(),
            _ => Vec::new(),
        })
    }

    /// The pool's strategy/selected for display: `(strategy, selected)`.
    pub fn plugin_pool_meta(
        &self,
        auth_ref: &str,
    ) -> anyhow::Result<Option<(String, Option<String>)>> {
        Ok(match self.load()?.get(auth_ref) {
            Some(AuthEntry::PluginPool(pool)) => Some((
                if pool.strategy.is_empty() {
                    "fill_first".to_string()
                } else {
                    pool.strategy.clone()
                },
                pool.selected.clone(),
            )),
            _ => None,
        })
    }

    /// Human "reset after …" when every account is cooling.
    pub fn pool_retry_hint(&self, auth_ref: &str) -> anyhow::Result<Option<String>> {
        let store = self.load()?;
        let now = now_secs();
        Ok(match store.get(auth_ref) {
            Some(AuthEntry::PluginPool(pool)) if !pool.accounts.is_empty() => {
                let earliest = pool
                    .accounts
                    .iter()
                    .map(|a| a.cooldown_until)
                    .filter(|u| *u > now)
                    .min();
                earliest.map(|u| {
                    let secs = u.saturating_sub(now);
                    format!("reset after {}m {}s", secs / 60, secs % 60)
                })
            }
            _ => None,
        })
    }

    /// Pin (`Some(label)`) or clear (`None`) the account a pool serves.
    pub fn select_plugin_account(&self, auth_ref: &str, label: Option<&str>) -> anyhow::Result<()> {
        let _lock = self.lock()?;
        let mut store = self.load()?;
        let Some(AuthEntry::PluginPool(pool)) = store.get_mut(auth_ref) else {
            anyhow::bail!("no plugin credential pool for {auth_ref}");
        };
        if let Some(label) = label {
            anyhow::ensure!(
                pool.accounts.iter().any(|a| a.label == label),
                "no account {label:?} in {auth_ref}"
            );
            pool.selected = Some(label.to_string());
        } else {
            pool.selected = None;
        }
        self.replace_locked(&store)
    }

    pub fn set_plugin_strategy(&self, auth_ref: &str, strategy: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(strategy, "fill_first" | "least_used"),
            "strategy must be fill_first or least_used"
        );
        let _lock = self.lock()?;
        let mut store = self.load()?;
        let Some(AuthEntry::PluginPool(pool)) = store.get_mut(auth_ref) else {
            anyhow::bail!("no plugin credential pool for {auth_ref}");
        };
        pool.strategy = strategy.to_string();
        self.replace_locked(&store)
    }

    pub fn remove_plugin_account(&self, auth_ref: &str, label: &str) -> anyhow::Result<()> {
        let _lock = self.lock()?;
        let mut store = self.load()?;
        let Some(AuthEntry::PluginPool(pool)) = store.get_mut(auth_ref) else {
            anyhow::bail!("no plugin credential pool for {auth_ref}");
        };
        let before = pool.accounts.len();
        pool.accounts.retain(|a| a.label != label);
        anyhow::ensure!(pool.accounts.len() < before, "no account {label:?}");
        if pool.selected.as_deref() == Some(label) {
            pool.selected = None;
        }
        self.replace_locked(&store)
    }

    /// Account-scoped verdicts cool the account; anything else leaves it.
    pub fn note_plugin_failure(
        &self,
        auth_ref: &str,
        label: &str,
        status: Option<u16>,
        detail: &str,
    ) -> anyhow::Result<()> {
        let ttl = match status {
            Some(401) => COOL_401_SECS,
            Some(429) => COOL_429_SECS,
            _ => return Ok(()),
        };
        let _lock = self.lock()?;
        let mut store = self.load()?;
        let Some(AuthEntry::PluginPool(pool)) = store.get_mut(auth_ref) else {
            return Ok(());
        };
        let sole = pool.accounts.len() == 1;
        let Some(a) = pool.accounts.iter_mut().find(|a| a.label == label) else {
            return Ok(());
        };
        let ttl = if sole {
            ttl.min(SOLE_CAP_SECS)
        } else {
            ttl.min(MAX_RESET_SECS)
        };
        a.cooldown_until = now_secs() + ttl.max(1);
        a.last_error = Some(detail.chars().take(200).collect());
        self.replace_locked(&store)
    }

    /// A served request cleared: bump counters, drop any recorded failure.
    pub fn note_plugin_success(&self, auth_ref: &str, label: &str) -> anyhow::Result<()> {
        let _lock = self.lock()?;
        let mut store = self.load()?;
        let Some(AuthEntry::PluginPool(pool)) = store.get_mut(auth_ref) else {
            return Ok(());
        };
        let Some(a) = pool.accounts.iter_mut().find(|a| a.label == label) else {
            return Ok(());
        };
        a.request_count += 1;
        a.last_used_at = now_secs();
        a.cooldown_until = 0;
        a.last_error = None;
        self.replace_locked(&store)
    }

    pub fn replace_plugin_if_bound(
        &self,
        auth_ref: &str,
        expected_binding: &str,
        next: CredentialMaterial,
        account_label: Option<String>,
    ) -> anyhow::Result<BTreeMap<String, AuthEntry>> {
        let _lock = self.lock()?;
        let mut store = self.load()?;
        let label = account_label.unwrap_or_else(|| match store.get(auth_ref) {
            Some(AuthEntry::PluginPool(pool)) => pool
                .selected
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            _ => "default".to_string(),
        });
        let Some(entry) = store.get_mut(auth_ref) else {
            anyhow::bail!("plugin credential is not installed");
        };
        match entry {
            AuthEntry::Plugin(current) => {
                if current.profile_binding != expected_binding {
                    anyhow::bail!("plugin credential profile binding changed");
                }
                let replacement = StoredCredential {
                    version: current.version,
                    plugin: current.plugin.clone(),
                    provider: current.provider.clone(),
                    auth_method: current.auth_method.clone(),
                    profile_binding: current.profile_binding.clone(),
                    credential: next,
                };
                *entry = AuthEntry::Plugin(replacement);
            }
            AuthEntry::PluginPool(pool) => {
                let Some(a) = pool.accounts.iter_mut().find(|a| a.label == label) else {
                    anyhow::bail!("plugin account {label:?} is not installed");
                };
                if a.profile_binding != expected_binding {
                    anyhow::bail!("plugin credential profile binding changed");
                }
                a.credential = next;
            }
            _ => anyhow::bail!("plugin credential is not installed"),
        }
        self.replace_locked(&store)?;
        let store = store.clone();
        Ok(store)
    }

    pub fn remove(&self, key: &str) -> anyhow::Result<bool> {
        let _lock = self.lock()?;
        let mut store = self.load()?;
        let removed = store.remove(key).is_some();
        if removed {
            self.replace_locked(&store)?;
        }
        Ok(removed)
    }

    pub fn remove_plugin_owner(&self, plugin: &str) -> anyhow::Result<usize> {
        let _lock = self.lock()?;
        let mut store = self.load()?;
        let prefix = format!("plugin:{plugin}:");
        let keys: Vec<String> = store
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        for key in &keys {
            store.remove(key);
        }
        if !keys.is_empty() {
            self.replace_locked(&store)?;
        }
        Ok(keys.len())
    }

    pub(crate) fn lock(&self) -> anyhow::Result<AuthLock> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        #[cfg(unix)]
        let existed = parent.symlink_metadata().is_ok();
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        if !existed {
            std::fs::set_permissions(parent, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
        }
        validate_parent(parent)?;
        let lock_path = self.path.with_extension("json.lock");
        validate_path(&lock_path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&lock_path)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(AuthLock { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(std::fs::TryLockError::WouldBlock) => anyhow::bail!("auth store lock timeout"),
                Err(std::fs::TryLockError::Error(e)) => {
                    anyhow::bail!("auth store lock unavailable: {e}")
                }
            }
        }
    }
}

pub fn load_plugin_credential(
    path: &Path,
    auth_ref: &str,
) -> anyhow::Result<Option<StoredCredential>> {
    CredentialStore::new(path.to_path_buf()).read_plugin(auth_ref)
}

pub fn save_plugin_credential(path: &Path, value: StoredCredential) -> anyhow::Result<()> {
    CredentialStore::new(path.to_path_buf())
        .put_plugin(value)
        .map(|_| ())
}

pub fn remove_plugin_owner(path: &Path, plugin: &str) -> anyhow::Result<usize> {
    CredentialStore::new(path.to_path_buf()).remove_plugin_owner(plugin)
}

const MAX_AUTH_BYTES: usize = 4 * 1024 * 1024;

fn plugin_auth_ref(value: &StoredCredential) -> anyhow::Result<String> {
    let auth_ref = format!(
        "plugin:{}:{}:{}",
        value.plugin, value.provider, value.auth_method
    );
    if value.plugin.is_empty()
        || value.provider.is_empty()
        || value.auth_method.is_empty()
        || value.plugin.contains(':')
        || value.provider.contains(':')
        || value.auth_method.contains(':')
    {
        anyhow::bail!("invalid plugin credential identity");
    }
    Ok(auth_ref)
}

fn validate_path(path: &Path) -> anyhow::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    validate_parent(parent)?;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("auth path is not a regular file");
    }
    if metadata.len() > MAX_AUTH_BYTES as u64 {
        anyhow::bail!("auth store exceeds the maximum size");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!("auth store permissions are too open");
        }
        if metadata.nlink() != 1 {
            anyhow::bail!("auth store has multiple hard links");
        }
    }
    Ok(())
}

fn validate_parent(parent: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(parent)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("auth parent is not a directory");
    }
    Ok(())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

/// One pool row for the `/accounts` panel.
#[derive(Clone, Debug)]
pub struct PluginAccountRow {
    pub label: String,
    pub email: Option<String>,
    pub cooling: bool,
    pub cooldown_until: u64,
    pub last_error: Option<String>,
    pub request_count: u64,
    pub last_used_at: u64,
    pub pinned: bool,
}

/// Cooldown TTLs mirror the plugin-owned pools: a 401 is a dead login
/// worth retrying occasionally; a 429 is quota. A lone account never
/// sits out longer than [`SOLE_CAP_SECS`], so a transient verdict can't
/// bench the only login for an hour.
const COOL_401_SECS: u64 = 5 * 60;
const COOL_429_SECS: u64 = 60 * 60;
const SOLE_CAP_SECS: u64 = 60;
const MAX_RESET_SECS: u64 = 8 * 24 * 3600;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The usable pick: pinned wins when it's not cooling, then `least_used`
/// (oldest use) or `fill_first` (pool order). `None` when the pool is
/// empty or every account is cooling.
fn pick_account(pool: &PluginAuthPool) -> Option<(&str, &PluginAccount)> {
    if pool.accounts.is_empty() {
        return None;
    }
    let now = now_secs();
    let usable = |a: &PluginAccount| a.cooldown_until <= now;
    if let Some(sel) = pool.selected.as_deref()
        && let Some(a) = pool.accounts.iter().find(|a| a.label == sel && usable(a))
    {
        return Some((sel, a));
    }
    let mut it = pool.accounts.iter().filter(|a| usable(a));
    if pool.strategy == "least_used" {
        return it
            .min_by_key(|a| a.last_used_at)
            .map(|a| (a.label.as_str(), a));
    }
    it.next().map(|a| (a.label.as_str(), a))
}
