use std::sync::Arc;

use gray_plugin::Plugin;

pub use gray_plugin::builder::from_plugins;

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

pub(crate) async fn active_plugins(
    handler: Option<gray_plugin::HostHandler>,
) -> anyhow::Result<(Vec<Arc<dyn Plugin>>, bool)> {
    let active =
        gray_plugin::builder::active_plugins(Vec::new(), &[], "gray.yml", handler, true).await?;
    drain_builder_warnings();
    Ok(active)
}

pub fn builtin_registry() -> gray_tools::Registry {
    from_plugins(&[]).0
}

pub async fn build_registry()
-> anyhow::Result<(gray_tools::Registry, Vec<gray_plugin::Manifest>, bool)> {
    let (plugins, fallback) = active_plugins(None).await?;
    let (registry, manifests) = from_plugins(&plugins);
    Ok((registry, manifests, fallback))
}
