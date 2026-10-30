use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LockEntry {
    pub ecosystem: String,
    pub version: String,
    pub hash: String,
    pub source: String,
    pub argv: Vec<String>,
    pub adapter_version: String,
    pub installed_at: String,
    pub scope: String,
    pub enabled: bool,
    pub granted_capabilities: Vec<String>,
    pub capabilities_hash: Option<String>,
    pub runtime_role: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_argv: Option<Vec<String>>,

    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub global: bool,

    /// Fields this build doesn't model, kept so a rewrite by an older or
    /// newer gray never drops them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for LockEntry {
    fn default() -> Self {
        Self {
            ecosystem: String::new(),
            version: String::new(),
            hash: String::new(),
            source: String::new(),
            argv: Vec::new(),
            adapter_version: String::new(),
            installed_at: String::new(),
            scope: String::new(),
            enabled: true,
            granted_capabilities: Vec::new(),
            capabilities_hash: None,
            runtime_role: None,
            cli_argv: None,
            global: false,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LockFile {
    pub schema: u32,
    pub plugins: BTreeMap<String, LockEntry>,
    /// Entries that failed to parse, verbatim: `save` writes them back so a
    /// read-modify-write never deletes a plugin this build can't read.
    #[serde(skip)]
    invalid: BTreeMap<String, serde_json::Value>,
}

impl Default for LockFile {
    fn default() -> Self {
        Self {
            schema: 1,
            plugins: BTreeMap::new(),
            invalid: BTreeMap::new(),
        }
    }
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
    /// Drop `name`, readable or not. True when it was present.
    pub fn remove(&mut self, name: &str) -> bool {
        let parsed = self.plugins.remove(name).is_some();
        self.invalid.remove(name).is_some() || parsed
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
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Self::default(), Vec::new())),
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
        let mut invalid = BTreeMap::new();
        let mut errors = Vec::new();
        for (name, value) in raw.plugins {
            match serde_path_to_error::deserialize::<_, LockEntry>(value.clone()) {
                Ok(entry) => {
                    plugins.insert(name, entry);
                }
                Err(e) => {
                    errors.push(format!("{name}: {e}"));
                    invalid.insert(name, value);
                }
            }
        }
        Ok((
            Self {
                schema: raw.schema,
                plugins,
                invalid,
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
        let mut doc = serde_json::to_value(&out)?;
        for (name, raw) in &self.invalid {
            if !self.plugins.contains_key(name) {
                doc["plugins"][name] = raw.clone();
            }
        }
        let text = serde_json::to_string_pretty(&doc)?;
        use std::io::Write;
        tmp.write_all(text.as_bytes())?;
        tmp.write_all(b"\n")?;
        tmp.flush()?;
        tmp.persist(path)
            .map_err(|e| anyhow::anyhow!("persist lockfile: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_keeps_unreadable_and_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock.json");
        std::fs::write(
            &path,
            r#"{"schema":1,"plugins":{
                "good":{"version":"1","future_field":42},
                "bad":{"version":7},
                "gone":{"version":7}}}"#,
        )
        .unwrap();
        let (mut lock, errors) = LockFile::load_with_errors(&path).unwrap();
        assert_eq!(errors.len(), 2);
        assert!(lock.plugins["good"].enabled);
        assert!(lock.remove("gone"));
        lock.save(&path).unwrap();

        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["plugins"]["good"]["future_field"], 42);
        assert_eq!(doc["plugins"]["bad"]["version"], 7);
        assert!(doc["plugins"].get("gone").is_none());
    }
}
