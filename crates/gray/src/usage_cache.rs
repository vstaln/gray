use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const VERSION: u32 = 1;
const KEEP_SECS: i64 = 30 * 86_400;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Window {
    pub(crate) provider: String,
    #[serde(default)]
    pub(crate) account: Option<String>,
    pub(crate) window: String,
    #[serde(default)]
    pub(crate) last_seen_pct: Option<f64>,
    #[serde(default)]
    pub(crate) resets_at: Option<i64>,
    #[serde(default)]
    pub(crate) limit: Option<i64>,
    #[serde(default)]
    pub(crate) seen_at: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    windows: Vec<Window>,
}

pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn path() -> Option<PathBuf> {
    gray_core::paths::gray_home().map(|home| home.join("usage-cache.json"))
}

pub(crate) fn load() -> Vec<Window> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    serde_json::from_slice::<Store>(&raw)
        .map(|store| store.windows)
        .unwrap_or_default()
}

fn save(windows: &[Window]) {
    let Some(path) = path() else {
        return;
    };
    let now = now();
    let kept: Vec<Window> = windows
        .iter()
        .filter(|w| now - w.seen_at < KEEP_SECS)
        .cloned()
        .collect();
    let store = Store {
        version: VERSION,
        windows: kept,
    };
    let Ok(bytes) = serde_json::to_vec_pretty(&store) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, &bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

pub(crate) fn provider_key(raw: &str) -> &str {
    raw.rsplit(':')
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(raw)
}

fn upsert(windows: &mut Vec<Window>, next: Window) {
    if let Some(slot) = windows.iter_mut().find(|w| {
        w.provider == next.provider && w.account == next.account && w.window == next.window
    }) {
        if let Some(pct) = next.last_seen_pct {
            slot.last_seen_pct = Some(pct);
            if pct < 100.0 {
                slot.limit = None;
            }
        }
        if next.resets_at.is_some() {
            slot.resets_at = next.resets_at;
        }
        if next.limit.is_some() {
            slot.limit = next.limit;
        }
        slot.seen_at = next.seen_at;
        return;
    }
    windows.push(next);
}

pub(crate) fn note_quota(provider: &str, message: &str, resets_in: Option<std::time::Duration>) {
    let now = now();
    let window = window_of(message);
    let resets_at = resets_in.map(|wait| now + wait.as_secs() as i64);
    let mut windows = load();
    upsert(
        &mut windows,
        Window {
            provider: provider_key(provider).to_string(),
            account: None,
            window,
            last_seen_pct: None,
            resets_at,
            limit: Some(now),
            seen_at: now,
        },
    );
    save(&windows);
}

fn window_token(message: &str) -> Option<String> {
    let token = message
        .split_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(token, _)| token.trim().to_lowercase())?;
    let plain = !token.is_empty()
        && token.len() <= 24
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    plain.then_some(token)
}

fn window_of(message: &str) -> String {
    window_token(message).unwrap_or_else(|| "quota".to_string())
}
