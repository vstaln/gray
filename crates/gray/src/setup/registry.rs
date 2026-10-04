//! What each app declares about setting it up, and the state gray computes
//! from the app's own config: key presence only, never values.

use std::path::{Path, PathBuf};

/// A destination the user may pick instead of pasting an ID.
pub const PICKER_CHANNELS: &str = "channels";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// The user must supply it (secrets are masked, never logged).
    Required,
    /// The user may supply it; an empty default is fine.
    Optional,
    /// gray fills it in silently (paths resolved against the gray home).
    Derived,
}

#[derive(Debug, Clone, Copy)]
pub struct SetupField {
    pub key: &'static str,
    pub kind: FieldKind,
    pub description: &'static str,
    /// The portal page that issues the value, when there is one.
    pub url: Option<&'static str>,
    /// Masked on input, never written to any log or model-visible string.
    pub secret: bool,
    /// `Some("channels")` offers the destination picker after verification.
    pub picker: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
pub struct SetupDecl {
    /// The app's config file, relative to the user's home directory.
    pub config_path: &'static str,
    pub fields: &'static [SetupField],
    /// argv that proves the configuration works; success means configured.
    pub verify: &'static [&'static str],
    /// Ordered actions the flow runs after a successful verify.
    pub post_steps: &'static [&'static str],
    /// argv that runs the app's daemon in the foreground.
    pub service: Option<&'static [&'static str]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppSetupState {
    Configured,
    /// Required keys absent from the config, by key name.
    NeedsSetup(Vec<&'static str>),
    /// The verify command failed. Only the flow may set this.
    Broken,
}

impl SetupField {
    pub fn is_required(&self) -> bool {
        matches!(self.kind, FieldKind::Required)
    }
}

impl SetupDecl {
    pub fn config_file(&self, home: &Path) -> PathBuf {
        home.join(self.config_path)
    }

    pub fn field(&self, key: &str) -> Option<&SetupField> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// Key-presence state. Never reads, renders, or returns a value.
    pub fn state(&self, home: &Path) -> AppSetupState {
        let missing: Vec<&'static str> = self
            .fields
            .iter()
            .filter(|f| f.is_required())
            .filter(|f| !key_present(&self.config_file(home), f.key))
            .map(|f| f.key)
            .collect();
        if missing.is_empty() {
            AppSetupState::Configured
        } else {
            AppSetupState::NeedsSetup(missing)
        }
    }

    /// Values for every derived field. `gray_home` is gray's own home (the
    /// app spawns gray sessions under it); the config file — and therefore
    /// `workdir` — lives under the *user's* home.
    pub fn derived(&self, gray_home: &Path, user_home: &Path) -> Vec<(&'static str, String)> {
        let config_dir = self
            .config_file(user_home)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| user_home.to_path_buf());
        self.fields
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::Derived))
            .filter_map(|f| {
                resolve_derived(f.key, gray_home, &config_dir)
                    .map(|value| (f.key, value.to_string_lossy().into_owned()))
            })
            .collect()
    }
}

/// The three derived keys gray knows how to fill.
fn resolve_derived(key: &str, gray_home: &Path, config_dir: &Path) -> Option<PathBuf> {
    match key {
        "gray_bin" => std::env::current_exe().ok(),
        "gray_home" => Some(gray_home.to_path_buf()),
        "workdir" => Some(config_dir.to_path_buf()),
        _ => None,
    }
}

pub(crate) fn key_present(path: &Path, key: &str) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    value.get(key).is_some()
}

#[path = "registry_tests.rs"]
#[cfg(test)]
mod tests;
