use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gray_pkg::index::{Index, fetch_index};
use gray_plugin::lock::{LockFile, lock_path};

pub const FALLBACK_DEFAULT_PROFILE: &[&str] = &[
    "account",
    "quality-of-life",
    "memory",
    "search",
    "skills",
    "subagents",
];

const RETRY: Duration = Duration::from_secs(24 * 60 * 60);
const PROFILE_FILE: &str = "profile";
const SYNC_MARKER: &str = ".profile-sync-ver";

fn profile_path(home: &Path) -> PathBuf {
    home.join(PROFILE_FILE)
}

fn marker_path(home: &Path) -> PathBuf {
    home.join("plugins").join(SYNC_MARKER)
}

pub fn read_profile(home: &Path) -> String {
    if let Ok(raw) = std::fs::read_to_string(profile_path(home)) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Ok(lock) = LockFile::load(&lock_path(home)) {
        let has_default = FALLBACK_DEFAULT_PROFILE.iter().any(|name| {
            let canonical = gray_pkg::ops::canonical_name(name);
            lock.plugins.contains_key(*name) || lock.plugins.contains_key(canonical)
        });
        if has_default {
            return "default".to_string();
        }
    }
    "bare".to_string()
}

pub fn write_profile(home: &Path, profile: &str) -> anyhow::Result<()> {
    let path = profile_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{profile}\n"))?;
    Ok(())
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

fn write_marker(path: &Path, state: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, state);
}

async fn load_profile_plugins(profile: &str) -> (Vec<String>, Option<Index>) {
    if matches!(profile, "bare" | "custom") {
        return (Vec::new(), None);
    }
    let fetched = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok();
    let index = match fetched {
        Some(client) => fetch_index(&client).await.ok(),
        None => None,
    };
    if let Some(ref idx) = index
        && let Some(list) = idx.profiles.get(profile)
    {
        return (list.clone(), index);
    }
    let fallback = if profile == "default" {
        FALLBACK_DEFAULT_PROFILE
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    } else {
        Vec::new()
    };
    (fallback, index)
}

fn retire_superseded(home: &Path, index: &Index) -> Vec<String> {
    let Ok(lock) = LockFile::load(&lock_path(home)) else {
        return Vec::new();
    };
    let mut to_remove = Vec::new();
    for installed in lock.plugins.keys() {
        let canonical = gray_pkg::ops::canonical_name(installed);
        let entry = index
            .plugins
            .get(installed)
            .or_else(|| index.plugins.get(canonical));
        if let Some(entry) = entry {
            for old in &entry.supersedes {
                let old_canon = gray_pkg::ops::canonical_name(old);
                for candidate in [old.as_str(), old_canon] {
                    if lock.plugins.contains_key(candidate)
                        && !to_remove.iter().any(|x: &String| x == candidate)
                    {
                        to_remove.push(candidate.to_string());
                    }
                }
            }
        }
    }
    let mut retired = Vec::new();
    for old in to_remove {
        match gray_pkg::ops::remove(&old) {
            Ok(()) => retired.push(format!("retired superseded plugin {old}")),
            Err(e) => log::warn!("superseded plugin {old}: remove failed: {e:#}"),
        }
    }
    retired
}

pub async fn sync_profile(
    home: &Path,
    profile_override: Option<&str>,
    yes: bool,
) -> anyhow::Result<Vec<String>> {
    let profile = match profile_override {
        Some(p) => {
            let trimmed = p.trim();
            anyhow::ensure!(!trimmed.is_empty(), "profile name cannot be empty");
            write_profile(home, trimmed)?;
            trimmed.to_string()
        }
        None => read_profile(home),
    };
    if matches!(profile.as_str(), "bare" | "custom") {
        return Ok(vec![format!("profile {profile}: nothing to sync")]);
    }
    let (plugins, index) = load_profile_plugins(&profile).await;
    let mut lines = Vec::new();
    for name in &plugins {
        if !needed(home, name) {
            continue;
        }
        let spec = gray_pkg::ops::canonical_name(name);
        let res = if yes {
            crate::plugin_cli::install_spec(home, spec, false, true).await
        } else {
            crate::plugin_cli::install_spec_default(home, spec).await
        };
        match res {
            Ok(()) => lines.push(format!("installed {spec}")),
            Err(e) => lines.push(format!("failed {spec}: {e:#}")),
        }
    }
    if let Some(ref idx) = index {
        lines.extend(retire_superseded(home, idx));
    }
    write_marker(&marker_path(home), env!("CARGO_PKG_VERSION"));
    Ok(lines)
}

pub(crate) async fn migrate_once() {
    let Ok(home) = crate::plugin_cli::home() else {
        return;
    };
    let profile = read_profile(&home);
    if matches!(profile.as_str(), "bare" | "custom") {
        return;
    }
    let marker = marker_path(&home);
    let version = env!("CARGO_PKG_VERSION");
    if std::fs::read_to_string(&marker)
        .ok()
        .is_some_and(|v| v.trim() == version)
    {
        return;
    }
    let (plugins, index) = load_profile_plugins(&profile).await;
    let mut all_ok = true;
    for name in &plugins {
        if !needed(&home, name) {
            continue;
        }
        let stamp = home
            .join("plugins")
            .join(format!(".default-{name}.attempt"));
        if !attempt_due(&stamp) {
            all_ok = false;
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
            Err(e) => {
                log::warn!("profile plugin {name}: install failed: {e:#}");
                all_ok = false;
            }
        }
    }
    if let Some(ref idx) = index {
        let _ = retire_superseded(&home, idx);
    }
    if all_ok && plugins.iter().all(|name| !needed(&home, name)) {
        write_marker(&marker, version);
    }
}

pub(crate) fn spawn_migrate() {
    tokio::spawn(migrate_once());
}
