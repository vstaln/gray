//! SessionSource + build_session_key + FileGatewayStore
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use serde::{Deserialize, Serialize};
use crate::config::Platform;
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
pub trait GatewaySessionStore: Send+Sync {
    fn get_or_create(&self, key: &str, src: &SessionSource) -> String;
    fn get(&self, key: &str) -> Option<String>;
}
pub struct FileGatewayStore { path: PathBuf, map: RwLock<HashMap<String, GatewayEntry>> }
impl FileGatewayStore {
    pub fn new(path: PathBuf) -> Self {
        let map = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { path, map: RwLock::new(map) }
    }
    pub fn default_path() -> anyhow::Result<PathBuf> {
        let base = std::env::var("GRAY_HOME").or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray"))).map_err(|_| anyhow::anyhow!("no home"))?;
        Ok(PathBuf::from(base).join("gateway_sessions.json"))
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
impl GatewaySessionStore for FileGatewayStore {
    fn get_or_create(&self, key: &str, _src: &SessionSource) -> String {
        if let Some(e) = self.map.read().unwrap().get(key) { return e.session_id.clone(); }
        let id = uuid::Uuid::new_v4().to_string();
        let entry = GatewayEntry { session_key: key.to_string(), session_id: id.clone(), updated_at: chrono::Utc::now().timestamp() };
        self.map.write().unwrap().insert(key.to_string(), entry);
        self.persist();
        id
    }
    fn get(&self, key: &str) -> Option<String> { self.map.read().unwrap().get(key).map(|e| e.session_id.clone()) }
}
pub fn shared_store() -> Arc<dyn GatewaySessionStore> {
    let path = FileGatewayStore::default_path().unwrap_or_else(|_| PathBuf::from("/tmp/gray-gateway-sessions.json"));
    Arc::new(FileGatewayStore::new(path))
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn key_dm_isolated() {
        let src = SessionSource{ platform: Platform::Telegram, chat_id: "123".to_string(), chat_type: "dm".to_string(), user_id: Some("u1".to_string()), thread_id: None, scope_id: None, message_id: None };
        assert_eq!(build_session_key(&src, true, false), "gray:main:telegram:dm:123");
    }
}
