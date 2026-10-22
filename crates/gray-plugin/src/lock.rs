use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockEntry {
    pub ecosystem: String,
    pub version: String,
    pub hash: String,
    pub source: String,
    pub argv: Vec<String>,
    pub adapter_version: String,
    pub installed_at: String,
    pub scope: String,

    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub granted_capabilities: Vec<String>,

    #[serde(default)]
    pub capabilities_hash: Option<String>,

    #[serde(default)]
    pub runtime_role: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_argv: Option<Vec<String>>,

    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub global: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockFile {
    pub schema: u32,
    pub plugins: BTreeMap<String, LockEntry>,
}

pub fn lock_path(home: &Path) -> PathBuf {
    home.join("plugins/lock.json")
}

pub fn project_lock_path(cwd: &Path) -> PathBuf {
    cwd.join(".gray/plugins.json")
}

pub fn effective_enabled(user: &LockFile, project: &LockFile, name: &str) -> bool {
    project
        .plugins
        .get(name)
        .map(|e| e.enabled)
        .or_else(|| user.plugins.get(name).map(|e| e.enabled))
        .unwrap_or(true)
}

pub fn disabled_sidecar_argvs(user: &LockFile, project: &LockFile) -> Vec<Vec<String>> {
    let names: std::collections::BTreeSet<&String> =
        user.plugins.keys().chain(project.plugins.keys()).collect();
    let mut out = Vec::new();
    for name in names {
        if !effective_enabled(user, project, name)
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

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Self::load_with_errors(path).map(|(lf, _)| lf)
    }

    /// One malformed entry must not zero out the whole registry: `plugins` is
    /// parsed per entry and each failure comes back as a "name: reason"
    /// string for the caller to surface; the plugin is skipped.
    pub fn load_with_errors(path: &Path) -> anyhow::Result<(Self, Vec<String>)> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Self::empty(), Vec::new())),
            Err(e) => Err(e.into()),
        }
    }

    fn parse(text: &str) -> anyhow::Result<(Self, Vec<String>)> {
        #[derive(serde::Deserialize)]
        struct Raw {
            schema: u32,
            #[serde(default)]
            plugins: BTreeMap<String, serde_json::Value>,
        }
        let raw: Raw = serde_json::from_str(text)?;
        let mut plugins = BTreeMap::new();
        let mut errors = Vec::new();
        for (name, value) in raw.plugins {
            match serde_path_to_error::deserialize::<_, LockEntry>(value) {
                Ok(entry) => {
                    plugins.insert(name, entry);
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        Ok((
            Self {
                schema: raw.schema,
                plugins,
            },
            errors,
        ))
    }

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
