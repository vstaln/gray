//! Boot wiring: ordered plugins from profile + lockfile.
//!
//! Moved out of the `gray` crate so the future gateway repo can reuse it.
//! Depends only on gray-plugin + gray-core (+ std/tokio/serde_json/anyhow).
//! Never touches gray-tools (would be a dependency cycle).
//!
//! Order: profile entries first, then lock entries. Name conflict: later
//! wins by `manifest().name` (matches `merge_manifests`). Warnings are
//! collected, never printed. Lock warnings never include argv/URLs.

use std::sync::Arc;

use crate::{lock, profile};

/// Boot outcome flag + collected warnings (never printed here).
pub struct BootReport {
    pub used_fallback: bool,
    pub warnings: Vec<String>,
}

/// Ordered active plugins from profile + lockfile.
///
/// - `profile`: `Some(path)` loads via [`profile::load_entries`]
///   (missing file = silent empty; parse error = warning, empty).
/// - `home`: lock loaded via [`lock::LockFile`] at `lock_path(home)`
///   (missing = empty; corrupt = warning, empty).
/// - Builtin names resolve through `resolve_builtin` (unknown = warning, skip).
/// - Sidecar entries spawn via `SidecarPlugin::spawn`: profile-path failure
///   is a hard `Err` naming entry index + argv; lock-path failure is a
///   warning, others still load.
/// - `used_fallback` is true when the final list is empty.
pub async fn active_plugins(
    profile: Option<&std::path::Path>,
    home: &std::path::Path,
    resolve_builtin: &dyn Fn(&str) -> Option<std::sync::Arc<dyn crate::Plugin>>,
) -> anyhow::Result<(Vec<std::sync::Arc<dyn crate::Plugin>>, BootReport)> {
    let mut warnings: Vec<String> = Vec::new();
    let mut ordered: Vec<Arc<dyn crate::Plugin>> = Vec::new();

    let profile_display = profile
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "gray.yml".to_string());

    let profile_entries: Vec<profile::PluginEntry> = match profile {
        Some(p) => {
            let s = p.as_os_str().to_string_lossy().into_owned();
            match profile::load_entries(&s) {
                Ok(entries) => entries,
                Err(e) => {
                    let missing = e
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
                    if !missing {
                        warnings.push(format!(
                            "cannot load {} profile ({e}); using builtin plugins",
                            p.display()
                        ));
                    }
                    Vec::new()
                }
            }
        }
        None => Vec::new(),
    };

    let lock_p = lock::lock_path(home);
    let lock_file: lock::LockFile = match lock::LockFile::load(&lock_p) {
        Ok(lf) => lf,
        Err(e) => {
            warnings.push(format!(
                "cannot load {} ({e:#}); ignoring",
                lock_p.display()
            ));
            lock::LockFile {
                schema: 1,
                plugins: std::collections::BTreeMap::new(),
            }
        }
    };

    for (i, e) in profile_entries.iter().enumerate() {
        match e {
            profile::PluginEntry::Builtin(n) => match resolve_builtin(n) {
                Some(p) => ordered.push(p),
                None => warnings.push(format!(
                    "unknown plugin {n:?} in {profile_display} — ignoring"
                )),
            },
            profile::PluginEntry::Sidecar(spec) => {
                use anyhow::Context;
                let label = spec.0.join(" ");
                let plugin = crate::sidecar::SidecarPlugin::spawn(spec.0.clone())
                    .await
                    .with_context(|| format!("sidecar[{i}] ({label}) failed to spawn"))?;
                ordered.push(Arc::new(plugin) as Arc<dyn crate::Plugin>);
            }
        }
    }

    for (name, entry) in lock_file.plugins.iter() {
        if entry.argv.is_empty() {
            match resolve_builtin(name) {
                Some(p) => ordered.push(p),
                None => warnings.push(format!(
                    "unknown plugin {name:?} in {} — ignoring",
                    lock_p.display()
                )),
            }
            continue;
        }
        match crate::sidecar::SidecarPlugin::spawn(entry.argv.clone()).await {
            Ok(plugin) => ordered.push(Arc::new(plugin) as Arc<dyn crate::Plugin>),
            Err(e) => warnings.push(format!(
                "lock plugin {name:?} failed to spawn ({e:#}); ignoring"
            )),
        }
    }

    let mut deduped: Vec<Arc<dyn crate::Plugin>> = Vec::new();
    for p in ordered {
        let name = p.manifest().name.clone();
        if let Some(pos) = deduped.iter().position(|e| e.manifest().name == name) {
            deduped.remove(pos);
        }
        deduped.push(p);
    }

    let used_fallback = deduped.is_empty();
    Ok((
        deduped,
        BootReport {
            used_fallback,
            warnings,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Stub {
        name: String,
    }

    #[async_trait::async_trait]
    impl crate::Plugin for Stub {
        fn manifest(&self) -> crate::Manifest {
            crate::Manifest {
                name: self.name.clone(),
                version: "0.1.0".to_string(),
                tools: vec![],
                commands: vec![],
                hooks: vec![],
                provider: None,
                protocol: None,
                capabilities: vec![],
                subcommands: vec![],
            }
        }
    }

    fn resolver(name: &str) -> Option<Arc<dyn crate::Plugin>> {
        if name == "stub" {
            Some(Arc::new(Stub {
                name: "stub".to_string(),
            }) as Arc<dyn crate::Plugin>)
        } else {
            None
        }
    }

    #[tokio::test]
    async fn gateway_shaped_consumer_boots_with_only_plugin_and_core() {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let profile_path = dir.path().join("gray.yml");
        std::fs::write(&profile_path, "plugins:\n  - stub\n  - unknown-xyz\n").unwrap();
        let (plugins, report) = active_plugins(Some(&profile_path), home.path(), &resolver)
            .await
            .unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].manifest().name, "stub");
        assert!(!report.used_fallback);
        assert!(
            report.warnings.iter().any(|w| w.contains("unknown-xyz")),
            "{:?}",
            report.warnings
        );

        let (empty, fallback_report) = active_plugins(None, home.path(), &|_: &str| None)
            .await
            .unwrap();
        assert!(empty.is_empty());
        assert!(fallback_report.used_fallback);
    }
}
