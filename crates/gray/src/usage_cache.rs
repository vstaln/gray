use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const VERSION: u32 = 1;
const KEEP_SECS: i64 = 30 * 86_400;
const LIMIT_UNKNOWN_RESET_SECS: i64 = 6 * 3_600;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Window {
    pub(crate) provider: String,
    #[serde(default)]
    pub(crate) account: Option<String>,
    pub(crate) window: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
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

pub(crate) enum WindowState {
    Limit { resets: Option<String> },
    Percent(f64),
    Unknown,
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
        if next.label.is_some() {
            slot.label = next.label;
        }
        slot.seen_at = next.seen_at;
        return;
    }
    windows.push(next);
}

pub(crate) fn record_usage(provider: &str, accounts: &Value) {
    let provider = provider_key(provider).to_string();
    let now = now();
    let mut windows = load();
    for account in accounts.as_array().into_iter().flatten() {
        let label = account["label"].as_str().map(str::to_string);
        for window in account["usage"]["windows"].as_array().into_iter().flatten() {
            let Some(id) = window["id"].as_str().or_else(|| window["label"].as_str()) else {
                continue;
            };
            let display_label = window["label"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let pct = window["used_percent"].as_f64();
            upsert(
                &mut windows,
                Window {
                    provider: provider.clone(),
                    account: label.clone(),
                    window: id.to_lowercase(),
                    label: display_label,
                    last_seen_pct: pct,
                    resets_at: window["resets_at"].as_str().and_then(iso_epoch),
                    limit: pct.filter(|pct| *pct >= 100.0).map(|_| now),
                    seen_at: now,
                },
            );
        }
    }
    save(&windows);
}

pub(crate) fn record_health(provider: &str, accounts: &Value) {
    let provider = provider_key(provider).to_string();
    let now = now();
    let mut windows = load();
    let mut touched = false;
    for account in accounts.as_array().into_iter().flatten() {
        let Some(label) = account["label"].as_str() else {
            continue;
        };
        let Some(until) = account["health"]["until"].as_i64().filter(|at| *at > now) else {
            continue;
        };
        let Some(window) = account["health"]["reason"].as_str().and_then(window_token) else {
            continue;
        };
        upsert(
            &mut windows,
            Window {
                provider: provider.clone(),
                account: Some(label.to_string()),
                window,
                label: None,
                last_seen_pct: None,
                resets_at: Some(until),
                limit: Some(now),
                seen_at: now,
            },
        );
        touched = true;
    }
    if touched {
        save(&windows);
    }
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
            label: None,
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

fn iso_epoch(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|at| at.timestamp())
}

pub(crate) fn for_row(provider: &str, account: Option<&str>, serving: bool) -> Vec<Window> {
    let provider = provider_key(provider);
    let now = now();
    let live: Vec<Window> = load()
        .into_iter()
        .filter(|w| w.provider == provider && now - w.seen_at < KEEP_SECS)
        .collect();
    let mut rows: Vec<Window> = live
        .iter()
        .filter(|w| w.account.as_deref() == account)
        .cloned()
        .collect();
    if serving {
        for window in live.iter().filter(|w| w.account.is_none()) {
            if !rows.iter().any(|r| r.window == window.window) {
                rows.push(window.clone());
            }
        }
    }
    rows
}

pub(crate) fn state(window: &Window, now: i64) -> WindowState {
    let exhausted = window.limit.is_some() || window.last_seen_pct.is_some_and(|pct| pct >= 100.0);
    if exhausted && window.resets_at.is_some_and(|at| at <= now) {
        return WindowState::Percent(0.0);
    }
    match window.resets_at {
        Some(at) if exhausted && at > now => {
            return WindowState::Limit {
                resets: Some(clock(at)),
            };
        }
        None if exhausted => {
            let seen = window.limit.unwrap_or(window.seen_at);
            if seen > 0 && now - seen < LIMIT_UNKNOWN_RESET_SECS {
                return WindowState::Limit { resets: None };
            }
        }
        _ => {}
    }
    match window.last_seen_pct {
        Some(pct) => WindowState::Percent(pct),
        None => WindowState::Unknown,
    }
}

pub(crate) fn clock(epoch: i64) -> String {
    chrono::DateTime::from_timestamp(epoch, 0)
        .map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_preserves_display_label_when_later_update_has_none() {
        let mut windows = Vec::new();
        upsert(
            &mut windows,
            Window {
                provider: "antigravity-subscription".into(),
                account: None,
                window: "3p-weekly".into(),
                label: Some("Weekly \u{b7} Claude/GPT".into()),
                last_seen_pct: Some(50.0),
                resets_at: None,
                limit: None,
                seen_at: 100,
            },
        );
        upsert(
            &mut windows,
            Window {
                provider: "antigravity-subscription".into(),
                account: None,
                window: "3p-weekly".into(),
                label: None,
                last_seen_pct: None,
                resets_at: Some(200),
                limit: Some(150),
                seen_at: 150,
            },
        );
        assert_eq!(windows.len(), 1);
        assert_eq!(
            windows[0].label.as_deref(),
            Some("Weekly \u{b7} Claude/GPT")
        );
        assert_eq!(windows[0].limit, Some(150));
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    fn exhausted(resets_at: Option<i64>) -> Window {
        Window {
            provider: "p".into(),
            account: None,
            window: "weekly".into(),
            label: None,
            last_seen_pct: Some(100.0),
            resets_at,
            limit: Some(1_000),
            seen_at: 1_000,
        }
    }

    #[test]
    fn a_window_past_its_reset_stops_reporting_limited() {
        let w = exhausted(Some(1_500));
        assert!(matches!(state(&w, 1_400), WindowState::Limit { .. }));
        assert!(matches!(state(&w, 1_600), WindowState::Percent(_)));
    }
}
