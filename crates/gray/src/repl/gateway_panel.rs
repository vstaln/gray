//! `/gateway` connections panel: just the apps gray talks to (toggleable).
//!
//! The app rows come from the plugin registry, so a transport shows up
//! the day it is installed (slack, telegram, …). Setup lives in each app
//! itself — an app that declares a `setup` subcommand gets a
//! `gray <name> setup` hint on its row.
//! Daemon/cron/memory are named nowhere here: `gray gateway status`, `/cron`
//! and `/memory` are the commands that own them, and this panel repeating
//! those names was narration with nothing to do.

use super::*;
use crate::setup::{ManagerItem, ManagerSpec, format_plugin_row_parts, run_install_manager};

const GATEWAY_SPEC: ManagerSpec = ManagerSpec {
    title: "Connections",
    empty_hint: "no apps installed — gray plugin install <name>",
    error_verb: "toggle failed",
    supports_toggle: true,
    supports_remove: false,
    errors_tab: false,
    keep_stale_on_relist_error: true,
};

/// Subcommands an app declares for itself. Empty when it registered no
/// manifest (or no home resolves) — nothing is invented on its behalf.
fn declared(home: Option<&Path>, name: &str) -> Vec<String> {
    home.map(|h| crate::plugin_cli::declared_subcommands(h, name))
        .unwrap_or_default()
}

/// One toggleable row per installed app: the `/plugin` row shape plus the
/// subcommands the app declares (and a `gray <name> setup` hint when setup
/// is one of them). `home` is the gray home the manifests are read from.
pub(crate) fn app_rows_with(
    rows: &[crate::plugin_cli::ManagedRow],
    home: Option<&Path>,
) -> Vec<ManagerItem> {
    rows.iter()
        .map(|r| {
            let mut row =
                format_plugin_row_parts(&r.name, &r.version, &r.scope, &r.ecosystem, r.on);
            let mut extras = declared(home, &r.name);
            if extras.iter().any(|s| s == "setup") {
                extras.push(format!("gray {} setup", r.name));
            }
            if !extras.is_empty() {
                row.push_str(" \u{2014} ");
                row.push_str(&extras.join(" \u{b7} "));
            }
            ManagerItem {
                name: r.name.clone(),
                row,
                lit: r.on,
                enabled: r.on,
                read_only: false,
                needs_setup: false,
            }
        })
        .collect()
}

/// One toggleable row per installed app, read from the live registry.
pub(crate) fn app_rows(rows: &[crate::plugin_cli::ManagedRow]) -> Vec<ManagerItem> {
    app_rows_with(rows, crate::plugin_cli::home().ok().as_deref())
}

/// The whole panel: apps (or the install hint).
pub(crate) fn items() -> Vec<ManagerItem> {
    let rows = crate::plugin_cli::list_rows().unwrap_or_default();
    let mut out = app_rows(&rows);
    if out.is_empty() {
        out.push(ManagerItem {
            name: String::new(),
            row: GATEWAY_SPEC.empty_hint.to_string(),
            lit: false,
            enabled: false,
            read_only: true,
            needs_setup: false,
        });
    }
    out
}

/// Headless rendering of the same rows (piped stdin, tests).
pub(crate) fn format_text() -> String {
    items()
        .iter()
        .map(|i| i.row.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The connections picker. Returns whether anything was toggled.
pub(crate) fn run_gateway_modal(
    bg: Option<&crate::setup::BackgroundSnapshot>,
) -> anyhow::Result<bool> {
    run_install_manager(
        bg,
        None,
        &GATEWAY_SPEC,
        || Some(items()),
        // Removal is not offered here: /plugin owns it.
        |_| anyhow::bail!("remove apps with /plugin remove <name>"),
        crate::plugin_cli::set_managed_enabled,
    )
}

#[path = "gateway_panel_tests.rs"]
#[cfg(test)]
mod tests;
