//! `sessions.json`: session key → gray session id + where it last spoke.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cron::store::Origin as Route;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    #[serde(default)]
    pub session_id: Option<String>,
    /// The surface the last user message for this key came from: where
    /// autonomous output for the key goes.
    #[serde(default)]
    pub last_route: Option<Route>,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Default)]
pub struct Sessions {
    path: PathBuf,
    pub keys: BTreeMap<String, Entry>,
}

impl Sessions {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("sessions.json");
        let keys = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self { path, keys }
    }

    fn save(&self) {
        if let Err(e) = crate::cron::store::atomic_write_json(&self.path, &self.keys) {
            log::warn!("gateway: cannot save sessions.json: {e:#}");
        }
    }

    pub fn session_id(&self, key: &str) -> Option<String> {
        self.keys.get(key).and_then(|e| e.session_id.clone())
    }

    pub fn set_session_id(&mut self, key: &str, sid: &str) {
        let e = self.keys.entry(key.to_string()).or_default();
        if e.session_id.as_deref() != Some(sid) {
            e.session_id = Some(sid.to_string());
            e.updated_at = crate::cron::now_secs();
            self.save();
        }
    }

    pub fn note_route(&mut self, key: &str, route: &Route) {
        let e = self.keys.entry(key.to_string()).or_default();
        if e.last_route.as_ref() != Some(route) {
            e.last_route = Some(route.clone());
            e.updated_at = crate::cron::now_secs();
            self.save();
        }
    }

    pub fn last_route(&self, key: &str) -> Option<Route> {
        self.keys.get(key).and_then(|e| e.last_route.clone())
    }
}
