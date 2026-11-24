use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use gray_pkg::index::{Index, fetch_index};
use gray_plugin::lock::{LockFile, lock_path};

pub const FALLBACK_DEFAULT_PROFILE: &[&str] =
    &["account", "extras", "memory", "search", "subagents"];

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
        }) || lock.plugins.contains_key("quality-of-life");
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

fn preserve_vim_mode(home: &Path) {
    let dir = home.join("plugin-settings");
    let dst = dir.join("extras.json");
    let mut map = std::fs::read_to_string(&dst)
        .or_else(|_| std::fs::read_to_string(dir.join("quality-of-life.json")))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    if map.contains_key("vim_mode") {
        return;
    }
    map.insert("vim_mode".to_string(), serde_json::Value::Bool(true));
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(text) = serde_json::to_string_pretty(&serde_json::Value::Object(map)) {
        let _ = std::fs::write(&dst, text);
    }
}

fn needs_upgrade_before_retire(
    installed: &gray_plugin::lock::LockEntry,
    index_entry: &gray_pkg::index::Entry,
) -> bool {
    if index_entry.version.is_empty() || installed.version == index_entry.version {
        return false;
    }
    if installed.version.eq_ignore_ascii_case("dev") {
        return false;
    }
    // Only upgrade index-managed installs automatically; direct URL and local
    // path/dev installs keep whatever the user chose.
    installed.ecosystem != "url"
        && (installed.source.starts_with("http://") || installed.source.starts_with("https://"))
}

fn plan_superseded_retirements(lock: &LockFile, index: &Index) -> Vec<(String, bool, Vec<String>)> {
    // Map installed canonical superseding plugin -> (needs_upgrade, superseded plugins present).
    let mut plan = Vec::new();
    for (installed, lock_entry) in &lock.plugins {
        let canonical = gray_pkg::ops::canonical_name(installed);
        // Only retire legacy plugins when the canonical replacement itself is
        // actually installed — never let one legacy plugin retire another.
        if installed != canonical {
            continue;
        }
        let Some(entry) = index.plugins.get(installed) else {
            continue;
        };
        let mut olds = Vec::new();
        for old in &entry.supersedes {
            let candidate = old.as_str();
            if candidate != installed
                && lock.plugins.contains_key(candidate)
                && !olds.iter().any(|x: &String| x == candidate)
            {
                olds.push(candidate.to_string());
            }
        }
        if !olds.is_empty() {
            let upgrade = needs_upgrade_before_retire(lock_entry, entry);
            plan.push((installed.clone(), upgrade, olds));
        }
    }
    plan
}

async fn retire_superseded(home: &Path, index: &Index, yes: bool) -> (Vec<String>, bool) {
    let Ok(lock) = LockFile::load(&lock_path(home)) else {
        return (Vec::new(), true);
    };
    let plan = plan_superseded_retirements(&lock, index);
    let mut retired = Vec::new();
    let mut all_ok = true;
    let mut removed = std::collections::HashSet::new();
    for (canonical, upgrade, olds) in plan {
        if upgrade {
            let res = if yes {
                crate::plugin_cli::install_spec(home, &canonical, false, true, None).await
            } else {
                crate::plugin_cli::install_spec_default(home, &canonical).await
            };
            if let Err(e) = res {
                log::warn!("superseded retirement skipped: upgrading {canonical} failed: {e:#}");
                retired.push(format!(
                    "skipped retiring superseded plugins: upgrading {canonical} failed: {e:#}"
                ));
                all_ok = false;
                continue;
            }
            retired.push(format!(
                "updated {canonical} before retiring superseded plugins"
            ));
        }
        for old in olds {
            if !removed.insert(old.clone()) {
                continue;
            }
            if old == "vim" {
                preserve_vim_mode(home);
            }
            match gray_pkg::ops::remove(&old) {
                Ok(()) => {
                    let plugins_dir = home.join("plugins");
                    let _ = std::fs::remove_file(plugins_dir.join(format!("{old}-manifest.json")));
                    let _ =
                        std::fs::remove_file(plugins_dir.join(format!("{old}-completion.json")));
                    retired.push(format!("retired superseded plugin {old}"));
                }
                Err(e) => {
                    log::warn!("superseded plugin {old}: remove failed: {e:#}");
                    retired.push(format!("failed retiring superseded plugin {old}: {e:#}"));
                    all_ok = false;
                }
            }
        }
    }
    (retired, all_ok)
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
    let mut all_ok = true;
    for name in &plugins {
        if !needed(home, name) {
            continue;
        }
        let spec = gray_pkg::ops::canonical_name(name);
        let res = if yes {
            crate::plugin_cli::install_spec(home, spec, false, true, None).await
        } else {
            crate::plugin_cli::install_spec_default(home, spec).await
        };
        match res {
            Ok(()) => lines.push(format!("installed {spec}")),
            Err(e) => {
                lines.push(format!("failed {spec}: {e:#}"));
                all_ok = false;
            }
        }
    }
    if let Some(ref idx) = index {
        let (retired, retired_ok) = retire_superseded(home, idx, yes).await;
        lines.extend(retired);
        if !retired_ok {
            all_ok = false;
        }
    }
    if all_ok {
        write_marker(&marker_path(home), env!("CARGO_PKG_VERSION"));
    }
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
        let (_, retired_ok) = retire_superseded(&home, idx, false).await;
        if !retired_ok {
            all_ok = false;
        }
    }
    if all_ok && plugins.iter().all(|name| !needed(&home, name)) {
        write_marker(&marker, version);
    }
}

pub(crate) fn spawn_migrate() {
    tokio::spawn(migrate_once());
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_pkg::index::{Entry, HashSpec, Source};
    use gray_plugin::lock::LockEntry;

    fn entry(version: &str) -> Entry {
        Entry {
            ecosystem: "gray-native".into(),
            version: version.into(),
            description: String::new(),
            kind: "plugin".into(),
            source: Source {
                type_: "tarball".into(),
                url: "https://example.invalid/extras.tar.gz".into(),
            },
            hash: HashSpec::Single("sha256:deadbeef".into()),
            scope: "user".into(),
            commands: Vec::new(),
            official: true,
            verified: true,
            requires_plugins: Vec::new(),
            conflicts: Vec::new(),
            supersedes: vec!["skills".into()],
            superseded_by: Vec::new(),
            providers: Vec::new(),
        }
    }

    fn lock_entry(version: &str, source: &str) -> LockEntry {
        LockEntry {
            ecosystem: "gray-native".into(),
            version: version.into(),
            source: source.into(),
            scope: "user".into(),
            ..LockEntry::default()
        }
    }

    #[test]
    fn default_profile_folds_skills_into_extras() {
        assert!(FALLBACK_DEFAULT_PROFILE.contains(&"extras"));
        assert!(!FALLBACK_DEFAULT_PROFILE.contains(&"skills"));
    }

    #[test]
    fn upgrade_check_guards_stale_index_installs_but_not_current_or_dev() {
        let idx = entry("0.4.0");
        assert!(needs_upgrade_before_retire(
            &lock_entry("0.3.0", "https://example.invalid/extras-0.3.0.tar.gz"),
            &idx
        ));
        assert!(!needs_upgrade_before_retire(
            &lock_entry("0.4.0", "https://example.invalid/extras-0.4.0.tar.gz"),
            &idx
        ));
        assert!(!needs_upgrade_before_retire(
            &lock_entry("dev", "/home/user/gray-extras"),
            &idx
        ));
        assert!(!needs_upgrade_before_retire(
            &lock_entry("0.3.0", "/home/user/gray-extras/target/release/gray-extras"),
            &idx
        ));
        let mut url_install = lock_entry("0.0.0", "https://example.invalid/custom-extras.tar.gz");
        url_install.ecosystem = "url".into();
        assert!(!needs_upgrade_before_retire(&url_install, &idx));
    }

    #[test]
    fn retirement_plan_requires_installed_canonical_replacement() {
        let mut idx = Index {
            profiles: std::collections::BTreeMap::new(),
            plugins: std::collections::BTreeMap::new(),
        };
        let mut essentials = entry("0.4.0");
        essentials.supersedes = vec!["secrets".into(), "skills".into()];
        idx.plugins.insert("essentials".into(), essentials);

        // Without essentials installed, legacy plugins must never retire each other.
        let mut legacy_only = LockFile::default();
        legacy_only.plugins.insert(
            "skills".into(),
            lock_entry("0.2.3", "https://example.invalid/skills-0.2.3.tar.gz"),
        );
        legacy_only.plugins.insert(
            "secrets".into(),
            lock_entry("0.1.0", "https://example.invalid/secrets-0.1.0.tar.gz"),
        );
        assert!(plan_superseded_retirements(&legacy_only, &idx).is_empty());

        // With stale essentials installed, plan upgrades it before retiring legacy entries.
        let mut with_stale = legacy_only;
        with_stale.plugins.insert(
            "essentials".into(),
            lock_entry("0.3.0", "https://example.invalid/extras-0.3.0.tar.gz"),
        );
        let plan = plan_superseded_retirements(&with_stale, &idx);
        assert_eq!(
            plan,
            vec![(
                "essentials".to_string(),
                true,
                vec!["secrets".to_string(), "skills".to_string()]
            )]
        );
    }
}
