use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gray_plugin::lock::{LockFile, lock_path};

pub(crate) const MIGRATED_PLUGINS: &[&str] = &["qol"];

const RETRY: Duration = Duration::from_secs(24 * 60 * 60);

const MARKER: &str = ".qol-migration";

fn marker(home: &Path) -> PathBuf {
    home.join("plugins").join(MARKER)
}

fn needed(home: &Path, name: &str) -> bool {
    let Ok(lock) = LockFile::load(&lock_path(home)) else {
        return true;
    };
    let canonical = gray_pkg::ops::canonical_name(name);
    [name, canonical]
        .iter()
        .all(|n| !lock.plugins.contains_key(*n) && !lock.is_tombstoned(n))
}

fn attempt_due(stamp: &Path) -> bool {
    let recent = std::fs::metadata(stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < RETRY);
    !recent
}

fn mtime(path: impl AsRef<Path>) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn existing_user(home: &Path) -> bool {
    if std::fs::read_dir(home.join("sessions")).is_ok_and(|mut d| d.next().is_some()) {
        return true;
    }
    let Some(config) = mtime(home.join("config.json")) else {
        return false;
    };
    std::env::current_exe()
        .ok()
        .and_then(mtime)
        .is_some_and(|binary| config < binary)
}

fn write_marker(marker: &Path, state: &str) {
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(marker, state);
}

pub(crate) async fn migrate_once() {
    let Ok(home) = crate::plugin_cli::home() else {
        return;
    };
    let marker = marker(&home);
    if marker.exists() {
        return;
    }
    if !existing_user(&home) {
        write_marker(&marker, "fresh");
        return;
    }
    for name in MIGRATED_PLUGINS {
        if !needed(&home, name) {
            continue;
        }
        let stamp = home
            .join("plugins")
            .join(format!(".default-{name}.attempt"));
        if !attempt_due(&stamp) {
            return;
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
            Err(e) => {
                log::warn!("migrated plugin {name}: install failed: {e:#}");
                return;
            }
        }
    }
    if MIGRATED_PLUGINS.iter().all(|name| !needed(&home, name)) {
        write_marker(&marker, "installed");
    }
}

pub(crate) fn spawn_migrate() {
    tokio::spawn(migrate_once());
}
