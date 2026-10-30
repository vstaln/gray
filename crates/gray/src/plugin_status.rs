use std::sync::{Arc, Mutex};

use futures::future::{BoxFuture, FutureExt, Shared};
use gray_plugin::Plugin;
use gray_plugin::builder::SpawnOutcome;

#[derive(Debug, Clone)]
pub(crate) struct StartResult {
    pub name: String,
    pub error: Option<String>,
}

type BootOut = Result<Vec<Arc<dyn Plugin>>, String>;
type SharedBoot = Shared<BoxFuture<'static, BootOut>>;

static BOOT: Mutex<Option<SharedBoot>> = Mutex::new(None);

pub(crate) fn ensure_boot(handler: Option<gray_plugin::HostHandler>) -> Option<SharedBoot> {
    let mut slot = BOOT.lock().ok()?;
    if slot.is_none() {
        let boot: SharedBoot = async move {
            crate::profile::active_plugins(handler)
                .await
                .map(|(plugins, _)| plugins)
                .map_err(|e| format!("{e:#}"))
        }
        .boxed()
        .shared();
        tokio::spawn(boot.clone());
        *slot = Some(boot);
    }
    slot.clone()
}

pub(crate) fn results() -> Option<Vec<StartResult>> {
    Some(
        gray_plugin::builder::spawn_outcomes()?
            .into_iter()
            .map(|SpawnOutcome { name, error }| StartResult { name, error })
            .collect(),
    )
}

pub(crate) fn kickoff(shared: crate::composer::SharedTui, handler: gray_plugin::HostHandler) {
    let Some(boot) = ensure_boot(Some(handler)) else {
        return;
    };
    tokio::spawn(async move {
        let _ = boot.await;
        if let Ok(mut t) = shared.lock() {
            t.set_plugin_stats(stats_line());
            if !t.modal_open {
                let _ = t.draw();
            }
        }
    });
}

pub(crate) async fn take_boot() -> Option<BootOut> {
    let boot = BOOT.lock().ok()?.take()?;
    Some(boot.await)
}

pub(crate) fn drop_boot() {
    if let Ok(mut slot) = BOOT.lock() {
        slot.take();
    }
}

pub(crate) async fn booted_tool_defs(
    handler: Option<gray_plugin::HostHandler>,
    extra: Vec<Arc<dyn Plugin>>,
) -> Option<Vec<gray_core::message::ToolDef>> {
    let boot = ensure_boot(handler)?;
    let mut plugins = boot.await.ok()?;
    for p in extra {
        let name = p.manifest().name.clone();
        if let Some(pos) = plugins.iter().position(|e| e.manifest().name == name) {
            plugins.remove(pos);
        }
        plugins.push(p);
    }
    let (registry, _) = gray_plugin::builder::from_plugins(&plugins);
    Some(registry.defs())
}

fn recent_updated() -> Option<u64> {
    let home = crate::setup::gray_home().ok()?;
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("auto-update.json")).ok()?).ok()?;
    let at = v.get("at")?.as_u64()?;
    let now = gray_core::time::now_secs();
    if now.saturating_sub(at) > 24 * 3600 {
        return None;
    }
    let seen_path = home.join("auto-update-notified");
    let seen: u64 = std::fs::read_to_string(&seen_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if seen >= at {
        return None;
    }
    let _ = std::fs::write(&seen_path, at.to_string());
    let p = v.get("plugins")?;
    Some(p.get("updated")?.as_u64().unwrap_or(0) + p.get("rebuilt")?.as_u64().unwrap_or(0))
}

pub(crate) fn stats_line() -> Option<String> {
    let results = results()?;
    let plugins = results.len();
    if plugins == 0 {
        return None;
    }
    let loaded = results.iter().filter(|r| r.error.is_none()).count();
    let failed = plugins - loaded;
    let mut parts = vec![format!("{loaded}/{plugins} plugins loaded")];
    if failed > 0 {
        parts.push(format!("{failed} failed"));
    }
    match recent_updated() {
        Some(u) if u > 0 => parts.push(format!("{u} updated")),
        _ => {}
    }
    if failed > 0 {
        parts.push("/plugins to see why".to_string());
    }
    Some(parts.join(" · "))
}

pub(crate) async fn table() -> String {
    if results().is_none()
        && let Ok(cwd) = std::env::current_dir()
        && let Some(boot) = ensure_boot(Some(crate::host::default_handler(cwd)))
    {
        let _ = boot.await;
    }
    let results = results().unwrap_or_default();
    let rows = crate::plugin_cli::list_rows().unwrap_or_default();
    if rows.is_empty() {
        return "no plugins installed".to_string();
    }
    let name_w = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let ver_w = rows.iter().map(|r| r.version.len()).max().unwrap_or(0);
    rows.iter()
        .map(|r| {
            let status = if !r.on {
                "disabled".to_string()
            } else {
                match results.iter().find(|s| s.name == r.name) {
                    Some(s) => match &s.error {
                        Some(e) => format!("failed: {e}"),
                        None => "loaded".to_string(),
                    },
                    None => "not loaded".to_string(),
                }
            };
            format!("{:<name_w$} {:<ver_w$} {status}", r.name, r.version)
        })
        .collect::<Vec<_>>()
        .join("\n")
}
