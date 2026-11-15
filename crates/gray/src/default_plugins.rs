use std::path::Path;
use std::time::{Duration, SystemTime};

use gray_plugin::lock::{LockFile, lock_path};

pub(crate) const DEFAULT_PLUGINS: &[&str] = &["qol"];

const RETRY: Duration = Duration::from_secs(24 * 60 * 60);

fn missing(home: &Path) -> Vec<&'static str> {
    let Ok(lock) = LockFile::load(&lock_path(home)) else {
        return Vec::new();
    };
    DEFAULT_PLUGINS
        .iter()
        .copied()
        .filter(|name| {
            let canonical = gray_pkg::ops::canonical_name(name);
            [*name, canonical]
                .iter()
                .all(|n| !lock.plugins.contains_key(*n) && !lock.is_tombstoned(n))
        })
        .collect()
}

fn attempt_due(stamp: &Path) -> bool {
    let recent = std::fs::metadata(stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < RETRY);
    !recent
}

pub(crate) async fn ensure_installed() {
    let Ok(home) = crate::plugin_cli::home() else {
        return;
    };
    for name in missing(&home) {
        let stamp = home
            .join("plugins")
            .join(format!(".default-{name}.attempt"));
        if !attempt_due(&stamp) {
            continue;
        }
        if let Some(parent) = stamp.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&stamp, gray_core::time::now_secs().to_string());
        let spec = gray_pkg::ops::canonical_name(name);
        match crate::plugin_cli::install_spec_default(&home, spec).await {
            Ok(()) => {
                let _ = std::fs::remove_file(&stamp);
            }
            Err(e) => log::warn!("default plugin {name}: install failed: {e:#}"),
        }
    }
}

pub(crate) fn spawn_ensure_installed() {
    tokio::spawn(ensure_installed());
}
