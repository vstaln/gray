use std::path::PathBuf;

use serde_json::{Value, json};

const INTERVAL_SECS: u64 = 6 * 3600;

fn stamp_path() -> Option<PathBuf> {
    crate::setup::gray_home()
        .ok()
        .map(|h| h.join("auto-update.json"))
}

fn read_stamp(path: &PathBuf) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn stamp_due(path: &PathBuf) -> bool {
    let now = gray_core::spill::now_secs() as u64;
    match read_stamp(path).and_then(|v| v.get("at").and_then(Value::as_u64)) {
        None => true,
        Some(at) => now.checked_sub(at).is_none_or(|e| e >= INTERVAL_SECS),
    }
}

pub(crate) fn set_enabled(on: bool) -> anyhow::Result<()> {
    let path = crate::setup::saved_config_path()?;
    let _cfg_lock = crate::setup::lock_saved_config_at(&path).ok();
    let mut saved = crate::setup::load_saved_config_at(&path);
    saved.auto_update = if on { None } else { Some(false) };
    crate::setup::save_saved_config_at(&path, &saved)
}

pub(crate) fn status_text() -> String {
    let on = crate::setup::auto_update_enabled();
    let mut s = format!("auto-update: {}", if on { "on" } else { "off" });
    let stamp = stamp_path().and_then(|p| read_stamp(&p));
    match stamp
        .as_ref()
        .and_then(|v| v.get("at").and_then(Value::as_u64))
    {
        None => s.push_str("\nlast run: never"),
        Some(at) => {
            let ts = chrono::DateTime::from_timestamp(at as i64, 0)
                .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_else(|| at.to_string());
            let gray = stamp
                .as_ref()
                .and_then(|v| v.get("gray"))
                .and_then(Value::as_str)
                .unwrap_or("?");
            let p = stamp.as_ref().and_then(|v| v.get("plugins"));
            let num = |k: &str| {
                p.and_then(|p| p.get(k))
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            };
            s.push_str(&format!(
                "\nlast run: {ts} — gray: {gray}; plugins: {} updated, {} rebuilt, {} failed",
                num("updated"),
                num("rebuilt"),
                num("failed")
            ));
        }
    }
    s
}

pub fn auto_cli(arg: &str) -> anyhow::Result<()> {
    match arg.trim().to_ascii_lowercase().as_str() {
        "on" => {
            set_enabled(true)?;
            println!("auto-update on");
        }
        "off" => {
            set_enabled(false)?;
            println!("auto-update off — `gray update` still installs manually");
        }
        "status" | "" => println!("{}", status_text()),
        other => anyhow::bail!("unknown --auto value '{other}' (on|off|status)"),
    }
    Ok(())
}

pub(crate) enum SelfOutcome {
    Current,
    Installed(String),
    Skipped(String),
    Failed(String),
}

pub(crate) struct Outcome {
    pub gray: SelfOutcome,
    pub plugins_updated: usize,
    pub plugins_rebuilt: usize,
    pub plugins_failed: usize,
}

impl Outcome {
    pub(crate) fn notice(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let SelfOutcome::Installed(v) = &self.gray {
            parts.push(format!("gray {v} installed — /restart to load it"));
        }
        let n = self.plugins_updated + self.plugins_rebuilt;
        if n > 0 {
            parts.push(format!("{n} plugin(s) updated"));
        }
        if self.plugins_failed > 0 {
            parts.push(format!("{} plugin(s) failed", self.plugins_failed));
        }
        (!parts.is_empty()).then(|| format!("auto-update: {}", parts.join(", ")))
    }

    fn gray_stamp(&self) -> String {
        match &self.gray {
            SelfOutcome::Current => "current".to_string(),
            SelfOutcome::Installed(v) => format!("installed {v}"),
            SelfOutcome::Skipped(r) => format!("skipped: {r}"),
            SelfOutcome::Failed(e) => format!("failed: {e}"),
        }
    }
}

async fn self_step() -> SelfOutcome {
    if cfg!(windows) {
        return SelfOutcome::Skipped("windows".into());
    }
    let current = env!("CARGO_PKG_VERSION");
    let latest = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::update::latest_version(),
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => return SelfOutcome::Failed(format!("version check: {e}")),
        Err(_) => return SelfOutcome::Failed("version check timed out".into()),
    };
    if !crate::update::update_available(
        crate::update::CHANNEL,
        &latest,
        current,
        env!("GRAY_BUILD_ID"),
    ) {
        return SelfOutcome::Current;
    }
    match tokio::task::spawn_blocking(crate::update::install_unlocked).await {
        Ok(Ok(())) => SelfOutcome::Installed(latest),
        Ok(Err(e)) => SelfOutcome::Failed(format!("{e:#}")),
        Err(e) => SelfOutcome::Failed(format!("{e}")),
    }
}

fn env_opted_out() -> bool {
    std::env::var("GRAY_NO_UPDATE_CHECK").as_deref() == Ok("1")
        || std::env::var("GRAY_AUTO_UPDATE").as_deref() == Ok("0")
        || crate::config::offline()
}

pub(crate) async fn maybe_run(bare: bool) -> Option<Outcome> {
    if bare || env_opted_out() || cfg!(debug_assertions) || env!("CARGO_PKG_VERSION") == "0.0.0" {
        return None;
    }
    if !crate::setup::auto_update_enabled() {
        return None;
    }
    let stamp = stamp_path()?;
    if !stamp_due(&stamp) {
        return None;
    }
    let _lock = crate::update::try_acquire_update_lock()?;
    if !stamp_due(&stamp) {
        return None;
    }
    let gray = self_step().await;
    let (mut updated, mut rebuilt, mut failed) = (0usize, 0usize, 0usize);
    match gray_pkg::ops::update("all").await {
        Ok(rows) => {
            for r in &rows {
                match r.action {
                    gray_pkg::ops::UpdateAction::Installed(_) => updated += 1,
                    gray_pkg::ops::UpdateAction::Rebuilt => rebuilt += 1,
                    gray_pkg::ops::UpdateAction::Failed(_) => failed += 1,
                    _ => {}
                }
            }
        }
        Err(e) => {
            log::warn!("auto-update: plugin update pass failed: {e:#}");
            failed += 1;
        }
    }
    let outcome = Outcome {
        gray,
        plugins_updated: updated,
        plugins_rebuilt: rebuilt,
        plugins_failed: failed,
    };
    let body = json!({
        "at": gray_core::spill::now_secs() as u64,
        "gray": outcome.gray_stamp(),
        "plugins": {"updated": updated, "rebuilt": rebuilt, "failed": failed},
    });
    let _ = std::fs::write(&stamp, body.to_string());
    Some(outcome)
}
