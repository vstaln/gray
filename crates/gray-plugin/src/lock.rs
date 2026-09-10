//! Plugin lockfile (`<home>/plugins/lock.json`).
//!
//! Records resolved sidecar plugins so future boots can reuse them.
//! Schema is always 1. Missing file is empty (not an error); corrupt
//! file is an error (callers warn). Atomic writes via tempfile + rename.
//! Never logs argv or URLs (may carry tokens).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn default_true() -> bool {
    true
}

/// One locked plugin entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockEntry {
    pub ecosystem: String,
    pub version: String,
    pub hash: String,
    pub source: String,
    pub argv: Vec<String>,
    pub adapter_version: String,
    pub installed_at: String,
    pub scope: String,
    /// Disabled plugins are skipped at boot (old locks without the field
    /// load as enabled).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Lockfile body: schema + plugins keyed by manifest name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockFile {
    pub schema: u32,
    pub plugins: BTreeMap<String, LockEntry>,
}

/// Lockfile path for a home dir: `<home>/plugins/lock.json`.
pub fn lock_path(home: &Path) -> PathBuf {
    home.join("plugins/lock.json")
}

/// Project-scope lock path: `<cwd>/.gray/plugins.json` (same `LockFile`
/// shape as the user lock). Runtime data, never committed. A project entry
/// for the same plugin name wins over the user entry (see
/// [`disabled_sidecar_argvs`]).
pub fn project_lock_path(cwd: &Path) -> PathBuf {
    cwd.join(".gray/plugins.json")
}

/// Sidecar argv lists disabled by the user lock with the project lock
/// overlaid as an enabled-flag-only delta: a project entry for the same
/// name wins on the flag. Boot skips spawning a sidecar whose argv is in
/// the result. Names without argv (builtins) never match a spawn, and
/// project-only names (no user entry, so no known argv) contribute nothing:
/// the overlay toggles, it never discovers.
pub fn disabled_sidecar_argvs(user: &LockFile, project: &LockFile) -> Vec<Vec<String>> {
    let names: std::collections::BTreeSet<&String> =
        user.plugins.keys().chain(project.plugins.keys()).collect();
    let mut out = Vec::new();
    for name in names {
        let enabled = project
            .plugins
            .get(name)
            .map(|e| e.enabled)
            .or_else(|| user.plugins.get(name).map(|e| e.enabled))
            .unwrap_or(true);
        if !enabled
            && let Some(entry) = user.plugins.get(name)
            && !entry.argv.is_empty()
        {
            out.push(entry.argv.clone());
        }
    }
    out
}

impl LockFile {
    fn empty() -> Self {
        Self {
            schema: 1,
            plugins: BTreeMap::new(),
        }
    }

    /// Load the lockfile. Missing file is empty with schema 1 (not an
    /// error); corrupt content is an error, never a panic.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(e) => Err(e.into()),
        }
    }

    /// Save atomically: write to a tempfile in the same dir, then rename.
    /// Schema is always written as 1.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let mut out = self.clone();
        out.schema = 1;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let dir: PathBuf = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        let text = serde_json::to_string_pretty(&out)?;
        use std::io::Write;
        tmp.write_all(text.as_bytes())?;
        tmp.write_all(b"\n")?;
        tmp.flush()?;
        tmp.persist(path)
            .map_err(|e| anyhow::anyhow!("persist lockfile: {e}"))?;
        Ok(())
    }
}
