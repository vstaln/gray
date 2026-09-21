//! `/gateway` connections panel: the apps gray talks to (toggleable) plus a
//! rule and one-line pointers at the subsystems that already own a command.
//!
//! The app rows come from the merged plugin registry, so a transport shows up
//! the day it is installed (slack, telegram, …) — nothing here is per-app
//! except [`SETUP_PROBES`]. Daemon/cron/memory are *pointed at*, never
//! restated: `gray gateway status`, `/cron` and `/memory` already print
//! everything about them.

use super::*;
use crate::setup::{ManagerItem, ManagerSpec, format_plugin_row_parts, run_install_manager};

/// Catalog apps whose setup state gray can see without reading their private
/// config: the default config path's *existence*, never its contents (it
/// holds the bot token). Absent file = the app still needs its wizard.
const SETUP_PROBES: &[(&str, &str)] = &[("discord", ".config/gray-discord/config.json")];

/// Subsystems whose own command prints everything: `/gateway` names the
/// command instead of duplicating its output.
const POINTERS: &[(&str, &str)] = &[
    ("daemon", "gray gateway status · gray gateway on|off"),
    ("cron", "/cron · /cron on|off"),
    ("memory", "/memory · /memory on|off"),
];

const GATEWAY_SPEC: ManagerSpec = ManagerSpec {
    title: "Connections",
    empty_hint: "no apps installed — gray install plugin discord",
    error_verb: "toggle failed",
    supports_toggle: true,
    supports_remove: false,
    errors_tab: false,
    keep_stale_on_relist_error: true,
};

/// A rule row: the visual break between apps and the pointers.
fn separator() -> ManagerItem {
    ManagerItem {
        name: String::new(),
        row: "\u{2500}".repeat(44),
        lit: false,
        enabled: false,
        read_only: true,
    }
}

/// True when the app's default config file is absent under `home`. The path
/// is only ever tested for existence.
fn setup_missing(home: &Path, name: &str) -> bool {
    match SETUP_PROBES.iter().find(|(n, _)| *n == name) {
        Some((_, rel)) => !home.join(rel).exists(),
        None => false,
    }
}

/// Subcommands an app declares for itself. Empty when it registered no
/// manifest (or no home resolves) — nothing is invented on its behalf.
fn declared(home: Option<&Path>, name: &str) -> Vec<String> {
    home.map(|h| crate::plugin_cli::declared_subcommands(h, name))
        .unwrap_or_default()
}

/// One toggleable row per installed app: the `/plugin` row shape plus what
/// the app still needs (setup) and what it says it can do. `home` is the gray
/// home the manifests and config probes are read from.
pub(crate) fn app_rows_with(
    rows: &[crate::plugin_cli::ManagedRow],
    home: Option<&Path>,
) -> Vec<ManagerItem> {
    rows.iter()
        .map(|r| {
            let mut row =
                format_plugin_row_parts(&r.name, &r.version, &r.scope, &r.ecosystem, r.on);
            let mut extras: Vec<String> = Vec::new();
            if let Some(home) = home
                && setup_missing(home, &r.name)
            {
                extras.push(format!("needs setup \u{2014} gray {} setup", r.name));
            }
            extras.extend(declared(home, &r.name));
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
            }
        })
        .collect()
}

/// One toggleable row per installed app, read from the live registry.
pub(crate) fn app_rows(rows: &[crate::plugin_cli::ManagedRow]) -> Vec<ManagerItem> {
    app_rows_with(rows, crate::plugin_cli::home().ok().as_deref())
}

/// The whole panel: apps (or the install hint), the rule, the pointers.
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
        });
    }
    out.push(separator());
    for (label, detail) in POINTERS {
        out.push(ManagerItem {
            name: String::new(),
            row: format!("{label} \u{2014} {detail}"),
            lit: false,
            enabled: false,
            read_only: true,
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
