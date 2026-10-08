//! `outbox/`: replies the agent produced that a surface has not confirmed.
//!
//! At-least-once custody (OpenClaw delivery queue, Hermes obligation
//! ledger): an adapter `pull`s intents for its platform, which leases them
//! for [`LEASE_SECS`]; it posts, then `ack`s. An intent whose lease runs out
//! is offered again; after [`MAX_ATTEMPTS`] it moves to `outbox/dead/` and the
//! activity log says so. Nothing produced is silently lost, including across
//! a gateway restart: the files are the queue.

use std::path::{Path, PathBuf};

use super::event::Kind;
use crate::cron::store::Origin as Route;

pub const LEASE_SECS: i64 = 60;
pub const MAX_ATTEMPTS: u32 = 8;
/// Platform for output with no surface to go to: `gray gateway inbox` reads it.
pub const LOCAL: &str = "local";

/// Serializes lease/ack read-modify-writes between socket tasks and the loop.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Intent {
    pub id: String,
    pub created_at: i64,
    pub platform: String,
    #[serde(default)]
    pub route: Option<Route>,
    pub key: String,
    pub kind: Kind,
    pub text: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub leased_until: i64,
}

impl Intent {
    pub fn new(key: &str, kind: Kind, route: Option<Route>, text: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            created_at: crate::cron::now_secs(),
            platform: route
                .as_ref()
                .map(|r| r.platform.clone())
                .unwrap_or_else(|| LOCAL.to_string()),
            route,
            key: key.to_string(),
            kind,
            text: text.to_string(),
            attempts: 0,
            leased_until: 0,
        }
    }
}

fn outbox(dir: &Path) -> PathBuf {
    dir.join("outbox")
}

pub fn enqueue(dir: &Path, intent: &Intent) -> anyhow::Result<()> {
    let path = outbox(dir).join(super::event::spool_name(&intent.id));
    crate::cron::store::atomic_write_json(&path, intent)
}

fn load_all(dir: &Path) -> Vec<(PathBuf, Intent)> {
    super::event::spool_files(&outbox(dir))
        .into_iter()
        .filter_map(|p| {
            let i = serde_json::from_slice::<Intent>(&std::fs::read(&p).ok()?).ok()?;
            Some((p, i))
        })
        .collect()
}

/// Lease up to `limit` deliverable intents for `platform`, oldest first.
/// Intents past their last attempt are buried instead of offered.
pub fn pull(dir: &Path, platform: &str, now: i64, limit: usize) -> Vec<Intent> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for (path, mut intent) in load_all(dir) {
        if out.len() >= limit {
            break;
        }
        if intent.platform != platform || intent.leased_until > now {
            continue;
        }
        if intent.attempts >= MAX_ATTEMPTS {
            bury(dir, &path, &intent);
            continue;
        }
        intent.attempts += 1;
        intent.leased_until = now + LEASE_SECS;
        if crate::cron::store::atomic_write_json(&path, &intent).is_ok() {
            out.push(intent);
        }
    }
    out
}

fn bury(dir: &Path, path: &Path, intent: &Intent) {
    let dead = outbox(dir).join("dead");
    let _ = std::fs::create_dir_all(&dead);
    if let Some(name) = path.file_name() {
        let _ = std::fs::rename(path, dead.join(name));
    }
    super::activity::log(
        dir,
        "undelivered",
        serde_json::json!({"key": intent.key, "platform": intent.platform,
            "attempts": intent.attempts, "text": intent.text}),
    );
}

/// Confirm delivery: the intents are removed. Returns how many matched.
pub fn ack(dir: &Path, ids: &[String]) -> usize {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut n = 0;
    for (path, intent) in load_all(dir) {
        if ids.contains(&intent.id) && std::fs::remove_file(&path).is_ok() {
            n += 1;
            super::activity::log(
                dir,
                "delivered",
                serde_json::json!({"key": intent.key, "platform": intent.platform,
                    "kind": intent.kind.as_str()}),
            );
        }
    }
    n
}

#[path = "outbox_tests.rs"]
#[cfg(test)]
mod tests;
