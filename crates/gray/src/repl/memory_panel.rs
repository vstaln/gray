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

/// A read-only row carrying why the listing could not be produced. The
/// panel must not show "no memory entries" while entries exist but cannot be
/// read.
fn problem_row(what: &str) -> ManagerItem {
    ManagerItem {
        name: String::new(),
        row: format!("cannot list {what}"),
        lit: false,
        enabled: false,
        read_only: true,
    }
}

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
        // A missing store is an empty scope; anything else is a real failure
        // and gets a row of its own rather than a silent empty listing.
        match store.entries(scope) {
            Ok(entries) => {
                for (key, text) in entries {
                    out.push(ManagerItem {
                        name: key.clone(),
                        row: format!("{key} \u{2014} {label} \u{b7} {}", preview(&text)),
                        lit: true,
                        enabled: false,
                        read_only: true,
                    });
                }
            }
            Err(e) => {
                out.push(ManagerItem {
                    name: String::new(),
                    row: format!("cannot read {label} memory \u{2014} {e}"),
                    lit: false,
                    enabled: false,
                    read_only: true,
                });
            }
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
            // Both steps fold into one Result: an unresolvable home and an
            // unbuildable store are the same failure to this listing.
            let store = crate::setup::gray_home()
                .and_then(|home| crate::memory::MemoryStore::new(&home, cwd));
            match store {
                Ok(store) => Some(items_for(&store)),
                Err(e) => Some(vec![problem_row(&format!("{e:#}"))]),
            }
        },
        |_| anyhow::bail!("memory entries are read-only here \u{2014} gray memory remove <key>"),
        |_, _| Ok(()),
    )
}

#[path = "memory_panel_tests.rs"]
#[cfg(test)]
mod tests;
