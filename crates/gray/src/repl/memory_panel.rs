//! `/memory` entries panel: what gray has curated, read-only.
//!
//! Forgetting stays `gray memory remove <key>` — a picker must not make
//! deletion a keystroke, and `gray memory audit` is the advisory path.

use super::*;
use crate::setup::{ManagerItem, ManagerSpec, run_install_manager};

const MEMORY_SPEC: ManagerSpec = ManagerSpec {
    title: "Memory",
    empty_hint: "no memory entries \u{2014} gray memory set <key> <text>",
    error_verb: "read failed",
    supports_toggle: false,
    supports_remove: false,
    errors_tab: false,
    keep_stale_on_relist_error: false,
};

/// First line of an entry, trimmed to the modal's row budget.
fn preview(text: &str) -> String {
    const MAX: usize = 60;
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= MAX {
        return line.to_string();
    }
    let keep: String = line.chars().take(MAX - 1).collect();
    format!("{keep}\u{2026}")
}

/// One row per curated entry, user scope first, then this project's.
pub(crate) fn items_for(store: &crate::memory::MemoryStore) -> Vec<ManagerItem> {
    let mut out = Vec::new();
    for (scope, label) in [
        (crate::memory::Scope::User, "user"),
        (crate::memory::Scope::Project, "project"),
    ] {
        for (key, text) in store.entries(scope).unwrap_or_default() {
            out.push(ManagerItem {
                name: key.clone(),
                row: format!("{key} \u{2014} {label} \u{b7} {}", preview(&text)),
                lit: true,
                enabled: false,
                read_only: true,
            });
        }
    }
    out
}

/// The entries picker. Read-only: the return value is always false.
pub(crate) fn run_memory_modal(
    bg: Option<&crate::setup::BackgroundSnapshot>,
    cwd: &Path,
) -> anyhow::Result<bool> {
    run_install_manager(
        bg,
        &MEMORY_SPEC,
        || {
            let home = crate::setup::gray_home().ok()?;
            let store = crate::memory::MemoryStore::new(&home, cwd).ok()?;
            Some(items_for(&store))
        },
        |_| anyhow::bail!("memory entries are read-only here \u{2014} gray memory remove <key>"),
        |_, _| Ok(()),
    )
}

#[path = "memory_panel_tests.rs"]
#[cfg(test)]
mod tests;
