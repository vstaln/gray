//! Writing an app's config the way the app itself would: private (dir 0700,
//! file 0600), atomic (temp + rename), and never echoing a secret.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Map, Value};

use super::registry::SetupDecl;

/// What the user typed, keyed by the declaration's field names. Debug prints
/// declared secrets as `<secret>` so a panic message, a log line, or a test
/// failure can never leak one.
#[derive(Clone, Default)]
pub struct Supplied {
    values: HashMap<String, String>,
    /// Answers the app stores as a JSON array of strings (`allowed_users`).
    lists: HashMap<String, Vec<String>>,
    secret_keys: Vec<String>,
}

impl Supplied {
    pub fn insert(&mut self, key: &str, value: String, secret: bool) {
        if secret && !self.secret_keys.iter().any(|k| k == key) {
            self.secret_keys.push(key.to_string());
        }
        self.lists.remove(key);
        self.values.insert(key.to_string(), value);
    }

    /// An answer written as a JSON array; replaces a plain answer for `key`.
    pub fn insert_list(&mut self, key: &str, items: Vec<String>) {
        self.values.remove(key);
        self.lists.insert(key.to_string(), items);
    }

    pub fn list(&self, key: &str) -> Option<&[String]> {
        self.lists.get(key).map(Vec::as_slice)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.lists.is_empty()
    }
}

impl std::fmt::Debug for Supplied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut shown: Vec<(&String, &String)> = self.values.iter().collect();
        shown.sort_by(|a, b| a.0.cmp(b.0));
        let mut out = f.debug_map();
        for (key, value) in shown {
            if self.secret_keys.iter().any(|k| k == key) {
                out.entry(key, &"<secret>");
            } else {
                out.entry(key, value);
            }
        }
        let mut lists: Vec<(&String, &Vec<String>)> = self.lists.iter().collect();
        lists.sort_by(|a, b| a.0.cmp(b.0));
        for (key, items) in lists {
            out.entry(key, items);
        }
        out.finish()
    }
}

/// Merge the supplied answers plus the declaration's derived paths into the
/// app's config and write it privately and atomically. Unknown keys already
/// in the file survive; an unparseable file is replaced (the app's own
/// loader rejects it anyway).
pub fn write_config(
    config_path: &Path,
    decl: &SetupDecl,
    supplied: &Supplied,
    gray_home: &Path,
    user_home: &Path,
) -> anyhow::Result<()> {
    let mut data = read_object(config_path);
    for (key, value) in &supplied.values {
        data.insert(key.clone(), Value::String(value.clone()));
    }
    for (key, items) in &supplied.lists {
        let items = items.iter().cloned().map(Value::String).collect();
        data.insert(key.clone(), Value::Array(items));
    }
    for (key, value) in decl.derived(gray_home, user_home) {
        data.insert(key.to_string(), Value::String(value));
    }
    atomic_write(config_path, &Value::Object(data))
}

pub(crate) fn read_object(path: &Path) -> Map<String, Value> {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        },
        Err(_) => Map::new(),
    }
}

fn atomic_write(path: &Path, data: &Value) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| anyhow::anyhow!("cannot create the config directory: {e}"))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow::anyhow!("cannot create the config directory: {e}"))?;

    // The file half is the repo's single private atomic writer: unique 0600
    // tmp created with its mode already set, fsynced, renamed. A copy here
    // created the token at umask mode and chmod'ed it afterwards.
    super::catalog::save_private_json(path, data)
}

#[path = "write_config_tests.rs"]
#[cfg(test)]
mod tests;
