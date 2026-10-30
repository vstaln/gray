use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gray_plugin::lock::LockFile;
use gray_plugin::{
    CompositeRowDecl, CompositeSlotDecl, CompositeSource, ModelSlot, ModelVariant, SlotOption,
};

use crate::setup::variants::{CachedModel, CachedRow};

const MAX_OPTIONS: usize = 60;

pub(crate) fn provider_token(list_key: &str) -> String {
    if let Some(pid) = list_key.strip_prefix("plugin:") {
        return pid.replacen(':', "/", 1);
    }
    if let Ok(catalog) = crate::setup::load_catalog()
        && let Some((id, _)) = catalog
            .iter()
            .find(|(_, p)| crate::setup::normalize_custom_base_url(&p.base_url) == list_key)
    {
        return id.clone();
    }
    crate::setup::catalog::custom_key_id(list_key)
        .replace(|c: char| !(c.is_ascii_alphanumeric() || c == '-'), "-")
}

fn plain_rows(rows: &[CachedRow]) -> Vec<(String, String)> {
    rows.iter()
        .filter(|r| !matches!(r, CachedRow::Full(m) if !m.slots.is_empty()))
        .map(|r| r.pair())
        .collect()
}

fn qualified_options(
    list_key: &str,
    own_plain: &[(String, String)],
    lists: &BTreeMap<String, Vec<CachedRow>>,
) -> Vec<SlotOption> {
    let mut out: Vec<SlotOption> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |token: &str, row: &(String, String)| {
        let qualified = format!("{token}:{id}", id = row.0);
        if out.len() < MAX_OPTIONS && seen.insert(qualified.clone()) {
            out.push(SlotOption {
                id: qualified,
                name: row.1.clone(),
            });
        }
    };
    let own_token = provider_token(list_key);
    for row in own_plain {
        push(&own_token, row);
    }
    for (key, rows) in lists {
        if key == list_key || !key.starts_with("plugin:") {
            continue;
        }
        let token = provider_token(key);
        for row in &plain_rows(rows) {
            push(&token, row);
        }
    }
    for (key, rows) in lists {
        if key == list_key || key.starts_with("plugin:") {
            continue;
        }
        let token = provider_token(key);
        for row in &plain_rows(rows) {
            push(&token, row);
        }
    }
    out
}

fn synthesize(
    decl: &CompositeRowDecl,
    list_key: &str,
    own: &[CachedRow],
    lists: &BTreeMap<String, Vec<CachedRow>>,
) -> Option<CachedModel> {
    if own
        .iter()
        .any(|r| matches!(r, CachedRow::Full(m) if !m.slots.is_empty()))
    {
        return None;
    }
    let own_slots: Vec<&CompositeSlotDecl> = decl
        .slots
        .iter()
        .filter(|s| s.source == CompositeSource::Own)
        .collect();
    if own_slots.len() != 1 {
        log::warn!(target: "gray_config",
            "composite row '{}' ignored: needs exactly one own-source slot, found {}",
            decl.id, own_slots.len());
        return None;
    }
    let own_key = own_slots[0].key.clone();
    let own_plain = plain_rows(own);
    if own_plain.is_empty() {
        return None;
    }
    let mut slot_options: Vec<Vec<SlotOption>> = Vec::new();
    for slot in &decl.slots {
        let options = match slot.source {
            CompositeSource::Own => own_plain
                .iter()
                .take(MAX_OPTIONS)
                .map(|(id, name)| SlotOption {
                    id: id.clone(),
                    name: name.clone(),
                })
                .collect(),
            CompositeSource::All => qualified_options(list_key, &own_plain, lists),
        };
        if options.is_empty() {
            return None;
        }
        slot_options.push(options);
    }
    let mut partial: Vec<BTreeMap<String, String>> = vec![BTreeMap::new()];
    for (slot, options) in decl.slots.iter().zip(&slot_options) {
        let mut next = Vec::new();
        for parts in &partial {
            for opt in options {
                let mut p = parts.clone();
                p.insert(slot.key.clone(), opt.id.clone());
                next.push(p);
            }
        }
        partial = next;
    }
    let variants: Vec<ModelVariant> = partial
        .into_iter()
        .map(|parts| ModelVariant {
            id: parts
                .get(&own_key)
                .cloned()
                .unwrap_or_else(|| decl.id.clone()),
            effort: None,
            fast: false,
            parts,
        })
        .collect();
    Some(CachedModel {
        id: decl.id.clone(),
        name: decl.name.clone(),
        reasoning_efforts: Vec::new(),
        variants,
        slots: decl
            .slots
            .iter()
            .zip(slot_options)
            .map(|(s, options)| ModelSlot {
                key: s.key.clone(),
                label: s.label.clone(),
                options,
            })
            .collect(),
        declared: false,
        price_cliff_tokens: None,
    })
}

/// A file's (mtime, len), or None when it can't be read — the validity
/// check for the memoized declarations below.
type Stamp = Option<(std::time::SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn decls_from_lock(
    lock_path: &Path,
    plugins_dir: &Path,
    project: &LockFile,
    read: &mut Vec<(PathBuf, Stamp)>,
) -> Vec<CompositeRowDecl> {
    read.push((lock_path.to_path_buf(), stamp(lock_path)));
    let Ok(lock) = LockFile::load(lock_path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, entry) in &lock.plugins {
        if !gray_plugin::lock::effective_enabled(&lock, project, name) || !entry.enabled {
            continue;
        }
        let path = plugins_dir.join(format!("{name}-manifest.json"));
        read.push((path.clone(), stamp(&path)));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        out.extend(gray_plugin::Manifest::from_result(&v).composite_rows);
    }
    out
}

struct DeclsMemo {
    home: PathBuf,
    project: Option<PathBuf>,
    global: Option<PathBuf>,
    /// Every lock and manifest file the scan consulted, with its stamp.
    read: Vec<(PathBuf, Stamp)>,
    decls: Vec<CompositeRowDecl>,
}

/// Parsing every enabled plugin's manifest costs one read + JSON parse per
/// plugin (~70 on a full install), and model lists are loaded per provider
/// row as the /models rail moves. The scan is reused while none of the
/// files it read have changed (re-stat'ing them is ~70 cheap syscalls).
static DECLS_MEMO: std::sync::Mutex<Option<DeclsMemo>> = std::sync::Mutex::new(None);

fn composite_decls(home: &Path) -> Vec<CompositeRowDecl> {
    let project_path = std::env::current_dir()
        .ok()
        .map(|d| gray_plugin::lock::project_lock_path(&d));
    let global = gray_core::paths::gray_user_home().filter(|g| g != home);
    let mut memo = DECLS_MEMO.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = memo.as_ref()
        && m.home == home
        && m.project == project_path
        && m.global == global
        && m.read.iter().all(|(path, was)| stamp(path) == *was)
    {
        return m.decls.clone();
    }
    let mut read = Vec::new();
    if let Some(p) = &project_path {
        read.push((p.clone(), stamp(p)));
    }
    let project = project_path
        .as_deref()
        .and_then(|p| LockFile::load(p).ok())
        .unwrap_or_default();
    let mut out = decls_from_lock(
        &gray_plugin::lock::lock_path(home),
        &home.join("plugins"),
        &project,
        &mut read,
    );
    if let Some(global) = &global {
        out.extend(decls_from_lock(
            &gray_plugin::lock::lock_path(global),
            &global.join("plugins"),
            &project,
            &mut read,
        ));
    }
    *memo = Some(DeclsMemo {
        home: home.to_path_buf(),
        project: project_path,
        global,
        read,
        decls: out.clone(),
    });
    out
}

pub(crate) fn extend_with_composites(home: &Path, list_key: &str, rows: &mut Vec<CachedRow>) {
    if composite_decls(home).is_empty() {
        return;
    }
    let mut lists = crate::setup::context::provider_model_lists(home);
    lists.insert(list_key.to_string(), rows.clone());
    extend_with_composites_in(home, &lists, list_key, rows);
}

/// [`extend_with_composites`] against already-parsed `lists`, which must
/// hold `list_key`'s own `rows` (or lack the key when they're empty).
pub(crate) fn extend_with_composites_in(
    home: &Path,
    lists: &BTreeMap<String, Vec<CachedRow>>,
    list_key: &str,
    rows: &mut Vec<CachedRow>,
) {
    for decl in &composite_decls(home) {
        if let Some(model) = synthesize(decl, list_key, rows, lists) {
            rows.push(CachedRow::Full(model));
        }
    }
}
