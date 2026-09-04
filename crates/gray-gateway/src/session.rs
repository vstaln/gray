//! SessionSource + build_session_key + FileGatewayStore
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use serde::{Deserialize, Serialize};
use crate::config::Platform;
use crate::config::{ResetMode, ResetPolicy};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSource {
    pub platform: Platform,
    pub chat_id: String,
    #[serde(default="default_chat_type")] pub chat_type: String,
    pub user_id: Option<String>,
    pub thread_id: Option<String>,
    pub scope_id: Option<String>,
    pub message_id: Option<String>,
}
fn default_chat_type() -> String { "dm".to_string() }
pub fn build_session_key(src: &SessionSource, group_per_user: bool, thread_per_user: bool) -> String {
    let mut parts = vec![format!("gray:main:{}", src.platform), src.chat_type.clone()];
    if let Some(scope) = &src.scope_id { parts.push(scope.clone()); }
    parts.push(src.chat_id.clone());
    if let Some(t) = &src.thread_id { parts.push(format!("thread_{t}")); }
    let need_user = if src.chat_type == "dm" { false } else if src.thread_id.is_some() { thread_per_user } else { group_per_user };
    if need_user { if let Some(u) = src.user_id.clone() { parts.push(u); } }
    parts.join(":")
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayEntry { pub session_key: String, pub session_id: String, pub updated_at: i64 }
pub struct FileGatewayStore { path: PathBuf, map: RwLock<HashMap<String, GatewayEntry>> }
impl FileGatewayStore {
    pub fn new(path: PathBuf) -> Self {
        let map = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { path, map: RwLock::new(map) }
    }
    pub fn default_path() -> anyhow::Result<PathBuf> {
        crate::config::gray_home_dir().map(|b| b.join("gateway_sessions.json"))
    }
    fn persist(&self) {
        if let Ok(map) = self.map.read() {
            if let Ok(s) = serde_json::to_string_pretty(&*map) {
                let _ = std::fs::create_dir_all(self.path.parent().unwrap_or(&self.path));
                let _ = std::fs::write(&self.path, s);
            }
        }
    }
}
impl FileGatewayStore {
    pub fn get_or_create(&self, key: &str) -> String {
        // Hit = activity: bump updated_at so Idle measures since last use, not creation.
        let existing = { self.map.read().unwrap().get(key).map(|e| e.session_id.clone()) };
        if let Some(id) = existing {
            let now = chrono::Utc::now().timestamp();
            if let Some(entry) = self.map.write().unwrap().get_mut(key) {
                entry.updated_at = now;
            }
            self.persist();
            return id;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let entry = GatewayEntry { session_key: key.to_string(), session_id: id.clone(), updated_at: chrono::Utc::now().timestamp() };
        self.map.write().unwrap().insert(key.to_string(), entry);
        self.persist();
        id
    }
    pub fn get(&self, key: &str) -> Option<String> { self.map.read().unwrap().get(key).map(|e| e.session_id.clone()) }
    pub fn updated_at(&self, key: &str) -> Option<i64> { self.map.read().unwrap().get(key).map(|e| e.updated_at) }
    /// Check the reset policy on message routing: calls the existing
    /// [`reset`](Self::reset) when the session expired. Returns the new id.
    pub fn reset_if_due(&self, key: &str, policy: &ResetPolicy) -> Option<String> {
        let ts = self.updated_at(key)?;
        if reset_due(policy, ts, chrono::Utc::now().timestamp()) { Some(self.reset(key)) } else { None }
    }
    /// /reset: drop the mapping and mint a fresh session id.
    pub fn reset(&self, key: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let entry = GatewayEntry { session_key: key.to_string(), session_id: id.clone(), updated_at: chrono::Utc::now().timestamp() };
        self.map.write().unwrap().insert(key.to_string(), entry);
        self.persist();
        id
    }
}
pub fn shared_store() -> Arc<FileGatewayStore> {
    let path = FileGatewayStore::default_path().unwrap_or_else(|_| PathBuf::from("/tmp/gray-gateway-sessions.json"));
    Arc::new(FileGatewayStore::new(path))
}

/// Pure expiry check behind [`FileGatewayStore::reset_if_due`].
/// `none` never expires; `idle` expires after `idle_secs` without activity;
/// `daily` expires once the clock passes today's `at_hour` (UTC) boundary.
pub fn reset_due(policy: &ResetPolicy, updated_at: i64, now: i64) -> bool {
    match policy.mode {
        ResetMode::None => false,
        ResetMode::Idle => now.saturating_sub(updated_at) >= policy.idle_secs as i64,
        ResetMode::Daily => {
            use chrono::{TimeZone, Utc};
            let day = Utc.timestamp_opt(now, 0).single().map(|d| d.date_naive());
            let boundary = day.and_then(|d| d.and_hms_opt(policy.at_hour.min(23) as u32, 0, 0)).map(|b| b.and_utc().timestamp());
            match boundary {
                Some(b) => now >= b && updated_at < b,
                None => false,
            }
        }
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn key_dm_isolated() {
        let src = SessionSource{ platform: Platform::Telegram, chat_id: "123".to_string(), chat_type: "dm".to_string(), user_id: Some("u1".to_string()), thread_id: None, scope_id: None, message_id: None };
        assert_eq!(build_session_key(&src, true, false), "gray:main:telegram:dm:123");
    }

    #[test] fn key_group_per_user() {
        let src = SessionSource{ platform: Platform::Telegram, chat_id: "456".to_string(), chat_type: "group".to_string(), user_id: Some("u42".to_string()), thread_id: None, scope_id: None, message_id: None };
        // group_per_user true => user appended
        assert_eq!(build_session_key(&src, true, false), "gray:main:telegram:group:456:u42");
        // group_per_user false => no user
        assert_eq!(build_session_key(&src, false, false), "gray:main:telegram:group:456");
    }

    #[test] fn key_thread_per_user() {
        let src = SessionSource{ platform: Platform::Discord, chat_id: "chan1".to_string(), chat_type: "channel".to_string(), user_id: Some("u9".to_string()), thread_id: Some("t1".to_string()), scope_id: None, message_id: None };
        assert_eq!(build_session_key(&src, true, true), "gray:main:discord:channel:chan1:thread_t1:u9");
        assert_eq!(build_session_key(&src, true, false), "gray:main:discord:channel:chan1:thread_t1");
    }

    #[test] fn key_with_scope() {
        let src = SessionSource{ platform: Platform::Slack, chat_id: "C123".to_string(), chat_type: "channel".to_string(), user_id: Some("U1".to_string()), thread_id: None, scope_id: Some("T9".to_string()), message_id: None };
        assert_eq!(build_session_key(&src, true, false), "gray:main:slack:channel:T9:C123:U1");
        // dm ignores user even with scope
        let dm = SessionSource{ platform: Platform::Slack, chat_id: "D123".to_string(), chat_type: "dm".to_string(), user_id: Some("U1".to_string()), thread_id: None, scope_id: Some("T9".to_string()), message_id: None };
        assert_eq!(build_session_key(&dm, true, true), "gray:main:slack:dm:T9:D123");
    }

    #[test] fn store_get_or_create_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        let store = FileGatewayStore::new(path.clone());
        let src = SessionSource{ platform: Platform::Telegram, chat_id: "1".to_string(), chat_type: "dm".to_string(), user_id: None, thread_id: None, scope_id: None, message_id: None };
        let key = build_session_key(&src, true, false);
        let id1 = store.get_or_create(&key);
        let id2 = store.get_or_create(&key);
        assert_eq!(id1, id2);
        assert_eq!(store.get(&key), Some(id1.clone()));
        // persisted file exists
        assert!(path.exists());
    }

    #[test] fn idle_measures_since_last_activity() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileGatewayStore::new(dir.path().join("s.json"));
        let key = "gray:main:telegram:dm:1";
        let id1 = store.get_or_create(key);
        let t1 = store.updated_at(key).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        // Hit bumps updated_at but keeps the id (activity).
        let id2 = store.get_or_create(key);
        assert_eq!(id1, id2);
        let t2 = store.updated_at(key).unwrap();
        assert!(t2 > t1, "get_or_create hit must bump updated_at: {t1} -> {t2}");
        // Active session never expires under Idle(60s).
        let idle = crate::config::ResetPolicy { mode: crate::config::ResetMode::Idle, idle_secs: 60, at_hour: 0 };
        assert!(store.reset_if_due(key, &idle).is_none());
        // Quiet session does: backdate past the idle window.
        {
            let mut map = store.map.write().unwrap();
            if let Some(e) = map.get_mut(key) {
                e.updated_at = chrono::Utc::now().timestamp() - 3600;
            }
        }
        assert!(store.reset_if_due(key, &idle).is_some());
    }

    #[test] fn integration_truncate_and_key() {
        // ensure truncate does not affect session key (keys are not truncated)
        let long_chat = "a".repeat(5000);
        let src = SessionSource{ platform: Platform::Telegram, chat_id: long_chat.clone(), chat_type: "group".to_string(), user_id: Some("u1".to_string()), thread_id: None, scope_id: None, message_id: None };
        let key = build_session_key(&src, true, false);
        assert!(key.contains(&long_chat));
        // but outgoing message would be truncated/split
        let msg = "b".repeat(5000);
        let truncated = crate::platform::truncate_message(&msg, 4096);
        assert!(crate::platform::utf16_len(&truncated) <= 4096);
        let chunks = crate::platform::split_message(&msg, 4096);
        assert!(chunks.len() >= 2);
    }

    #[test] fn reset_none_never_expires() {
        let p = ResetPolicy::default();
        assert_eq!(p.mode, crate::config::ResetMode::None);
        assert!(!reset_due(&p, 0, i64::MAX));
        assert!(!reset_due(&p, 100, 101));
    }

    #[test] fn reset_idle_expiry() {
        let p = crate::config::ResetPolicy { mode: crate::config::ResetMode::Idle, idle_secs: 60, at_hour: 0 };
        assert!(!reset_due(&p, 1000, 1059));
        assert!(reset_due(&p, 1000, 1060));
        assert!(reset_due(&p, 1000, 2000));
    }

    #[test] fn reset_daily_expiry() {
        use chrono::TimeZone;
        let day1 = chrono::Utc.with_ymd_and_hms(2026, 8, 1, 5, 0, 0).unwrap().timestamp();
        let day1_early = chrono::Utc.with_ymd_and_hms(2026, 8, 1, 3, 0, 0).unwrap().timestamp();
        let day2 = chrono::Utc.with_ymd_and_hms(2026, 8, 2, 5, 0, 0).unwrap().timestamp();
        let p = crate::config::ResetPolicy { mode: crate::config::ResetMode::Daily, idle_secs: 0, at_hour: 4 };
        // Same-day after the hour: no reset yet.
        assert!(!reset_due(&p, day1, day1));
        // Before today's boundary: no reset either.
        assert!(!reset_due(&p, day1_early, day1_early + 60));
        // Next day past the hour with stale session: reset.
        assert!(reset_due(&p, day1, day2));
    }
}
