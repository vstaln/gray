//! Plugin profile: gray-surface policy over the shared builder.
//!
//! Profile→registry→hooks assembly lives once in
//! [`gray_plugin::builder`] (lowest common crate — keeps the shared builder
//! out of any single binary). This module keeps gray's
//! surface policy: bash-only tools (`tools-minimal`) plus the always-on
//! context-only [`crate::skills_tool::SkillsPlugin`]
//! (per-turn `<available_skills>` list, no tools), the transcript-safe
//! warning queue, and the `--dump-manifest` registry view.

use std::sync::Arc;

use gray_plugin::Plugin;

pub use gray_plugin::builder::{
    ToolsBasicPlugin, ToolsMinimalPlugin, ToolsSearchPlugin, from_plugins,
};

use crate::skills_tool::SkillsPlugin;

/// Builtin plugin catalog: what a `gray.yml` may name. The no-profile
/// fallback is `tools-minimal` (see [`active_plugins`]); `tools-basic` and
/// `tools-search` are opt-in.
fn gray_defaults() -> Vec<Arc<dyn Plugin>> {
    vec![
        Arc::new(ToolsMinimalPlugin) as Arc<dyn Plugin>,
        Arc::new(ToolsBasicPlugin) as Arc<dyn Plugin>,
        Arc::new(ToolsSearchPlugin) as Arc<dyn Plugin>,
        // Always on: per-turn `<available_skills>` context, no tools.
        // (The live agent path in `lib::build_agent` appends the same via
        // `extra_plugins`; this covers `--dump-manifest`/`builtin_registry`.)
        Arc::new(SkillsPlugin) as Arc<dyn Plugin>,
    ]
}

/// Profile warnings queued for transcript display. Raw `eprintln!` while the
/// composer viewport is live collides with the next draw (ghost/overlapped
/// rows), so lib code never prints — it queues here and the UI drains.
/// One lock, one Vec: each distinct message is queued once per drain cycle
/// (N is tiny; Vec scan is fine). A rebuild re-queues a still-broken profile
/// warning — correct, like a compiler re-emitting warnings.
static PROFILE_WARNINGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn queue_profile_warning(msg: String) {
    PROFILE_WARNINGS
        .lock()
        .map(|mut q| {
            if !q.contains(&msg) {
                q.push(msg);
            }
        })
        .ok();
}

/// Drains queued profile warnings (transcript/non-TUI display owns rendering).
pub fn take_profile_warnings() -> Vec<String> {
    PROFILE_WARNINGS
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

fn drain_builder_warnings() {
    for w in gray_plugin::builder::take_builder_warnings() {
        queue_profile_warning(w);
    }
}

/// Ordered active plugins: the `gray.yml` profile order, or builtins when
/// the profile is missing/unparseable/empty. Manifest-only boot (no host
/// handler); a sidecar spawn failure aborts boot with entry index + argv.
pub(crate) async fn active_plugins() -> anyhow::Result<(Vec<Arc<dyn Plugin>>, bool)> {
    let (mut plugins, fallback) = gray_plugin::builder::active_plugins(
        gray_defaults(),
        // Fallback when no profile resolves: bash shell family + skills context.
        &["tools-minimal", "skills"],
        "gray.yml",
        None,
        true,
    )
    .await?;
    drain_builder_warnings();
    // Always-on context-only `skills` plugin: appended after the profile +
    // lock set so it survives explicit `tools-minimal`-only profiles too
    // (same later-wins dedupe as the builder's `extra_plugins`;
    // mirrors `lib::build_agent`). Carries no tools — tools stay bash-only.
    if !fallback {
        let name = "skills";
        if let Some(pos) = plugins.iter().position(|e| e.manifest().name == name) {
            plugins.remove(pos);
        }
        plugins.push(Arc::new(SkillsPlugin) as Arc<dyn Plugin>);
    }
    Ok((plugins, fallback))
}

/// The default builtin registry (no profile file): `tools-minimal` plus the
/// context-only `skills` plugin (so `/context` tool estimates match the live agent).
pub fn builtin_registry() -> gray_tools::Registry {
    from_plugins(&[
        Arc::new(ToolsMinimalPlugin) as Arc<dyn Plugin>,
        Arc::new(SkillsPlugin) as Arc<dyn Plugin>,
    ])
    .0
}

/// Builds the tool registry from the `gray.yml` profile plugin order,
/// falling back to builtins when no profile file is present.
/// Returns `(registry, manifests, used_fallback)` — the flag feeds
/// `--dump-manifest`'s note.
/// A sidecar spawn failure is a hard `Err` naming the entry (caller aborts boot).
pub async fn build_registry()
-> anyhow::Result<(gray_tools::Registry, Vec<gray_plugin::Manifest>, bool)> {
    let (plugins, fallback) = active_plugins().await?;
    let (registry, manifests) = from_plugins(&plugins);
    Ok((registry, manifests, fallback))
}
