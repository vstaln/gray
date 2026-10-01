//! Strict mixed credential storage for API keys, legacy OAuth, and plugins.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gray_core::credential::{CredentialEnvelope, CredentialMaterial};

use crate::setup::catalog::{self, AuthEntry};

pub type StoredCredential = CredentialEnvelope;

/// The private auth store owns mutation of `<gray-home>/auth.json`.
#[derive(Clone, Debug)]
pub struct CredentialStore {
    path: PathBuf,
}

/// A held advisory lock for one auth file.
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
        catalog::load_mixed_store_strict(&self.path)
    }

    pub fn replace(&self, next: &BTreeMap<String, AuthEntry>) -> anyhow::Result<()> {
        let _lock = self.lock()?;
        self.replace_locked(next)
    }

    /// The write half of [`Self::replace`]; the caller holds the lock.
    ///
    /// Read-modify-write must run under one lock: loading before locking let
    /// two writers overwrite each other, and the loser's credential was gone.
    fn replace_locked(&self, next: &BTreeMap<String, AuthEntry>) -> anyhow::Result<()> {
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
        store.insert(auth_ref, AuthEntry::Plugin(value));
        self.replace_locked(&store)?;
        Ok(store)
    }

    pub fn read_plugin(&self, auth_ref: &str) -> anyhow::Result<Option<StoredCredential>> {
        Ok(self.load()?.get(auth_ref).and_then(|entry| match entry {
            AuthEntry::Plugin(value) => Some(value.clone()),
            _ => None,
        }))
    }

    pub fn replace_plugin_if_bound(
        &self,
        auth_ref: &str,
        expected_binding: &str,
        next: CredentialMaterial,
    ) -> anyhow::Result<BTreeMap<String, AuthEntry>> {
        let _lock = self.lock()?;
        let mut store = self.load()?;
        let Some(AuthEntry::Plugin(current)) = store.get(auth_ref) else {
            anyhow::bail!("plugin credential is not installed");
        };
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
        store.insert(auth_ref.to_string(), AuthEntry::Plugin(replacement));
        self.replace_locked(&store)?;
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

    /// The store's advisory lock. `pub(crate)` so the catalog's own
    /// read-modify-write on the same file takes it too, instead of
    /// duplicating the flock.
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
