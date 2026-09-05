//! Plugin profile: gray-surface policy over the shared builder.
//!
//! Profile→registry→hooks assembly lives once in
//! [`gray_plugin::builder`] (lowest common crate — the `gray → gray-gateway`
//! edge forbids a `gray`-owned shared builder). This module keeps gray's
//! surface policy: the [`SkillTool`] default baked into `tools-basic`, the
//! transcript-safe warning queue, and the `--dump-manifest` registry view.

use std::sync::Arc;

use gray_plugin::Plugin;

pub use gray_plugin::builder::{ToolsBasicPlugin, ToolsSearchPlugin, from_plugins};

use crate::skills_tool::SkillTool;

/// Gray-surface defaults: shared builtins with the `skill` tool.
fn gray_defaults() -> Vec<Arc<dyn Plugin>> {
    vec![
        Arc::new(ToolsBasicPlugin {
            extra: vec![Arc::new(SkillTool)],
        }) as Arc<dyn Plugin>,
        Arc::new(ToolsSearchPlugin) as Arc<dyn Plugin>,
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
    let out =
        gray_plugin::builder::active_plugins(gray_defaults(), "gray.yml", None, true).await?;
    drain_builder_warnings();
    Ok(out)
}

/// The default builtin registry (no profile file).
pub fn builtin_registry() -> gray_tools::Registry {
    from_plugins(&gray_defaults()).0
}

/// Builds the tool registry from the `gray.yml` profile plugin order,
/// falling back to builtins when no profile file is present.
/// Returns `(registry, manifests, used_fallback)` — the flag feeds
/// `--dump-manifest`'s note.
/// A sidecar spawn failure is a hard `Err` naming the entry (caller aborts boot).
pub async fn build_registry() -> anyhow::Result<(
    gray_tools::Registry,
    Vec<gray_plugin::Manifest>,
    bool,
)> {
    let (plugins, fallback) = active_plugins().await?;
    let (registry, manifests) = from_plugins(&plugins);
    Ok((registry, manifests, fallback))
}
