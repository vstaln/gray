use std::path::{Path, PathBuf};

use gray_core::time::now_millis;

use gray_core::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};

const SUPPORTED_SESSION_VERSION: u32 = 1;

const SESSION_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) const RESTART_PARENT_ENV: &str = "GRAY_RESTART_PARENT_PID";
const RESTART_PARENT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn tighten_file_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> Self {
        let bits = uuid::Uuid::new_v4().as_u128();
        let q = QUALIFIER_WORDS[(bits as usize) % QUALIFIER_WORDS.len()];
        let m = MIDDLE_WORDS[((bits >> 32) as usize) % MIDDLE_WORDS.len()];
        let p = PHENOMENON_WORDS[((bits >> 64) as usize) % PHENOMENON_WORDS.len()];
        Self(format!("{q}-{m}-{p}"))
    }

    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub type SessionEntryId = u64;

type CarriedMeta = (u64, Option<gray_core::event::Usage>, Option<u64>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,

    pub timestamp: u64,

    pub cwd: PathBuf,

    pub model: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    #[serde(default, skip_serializing_if = "gray_core::is_false")]
    pub compaction_boundary: bool,

    pub entry_id: u64,

    pub parent_id: Option<u64>,

    pub timestamp: u64,

    pub message: Message,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<gray_core::event::Usage>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEnd {
    #[default]
    Done,

    ToolUse,

    ToolResult,

    UserUnanswered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    #[serde(default)]
    pub last_end: SessionEnd,

    #[serde(default)]
    pub dismissed_at: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    pub id: SessionId,

    pub started_at: u64,

    pub cwd: PathBuf,

    pub first_user_text: Option<String>,

    #[serde(default)]
    pub last_user_text: Option<String>,

    #[serde(default)]
    pub last_message_at: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("session {0} not found")]
    NotFound(SessionId),

    #[error("session {0} already exists")]
    AlreadyExists(SessionId),

    #[error(
        "session '{id}' is already open in another process{}. Close the other instance before opening it here.",
        .pid.map(|p| format!(" (PID {p})")).unwrap_or_default()
    )]
    Locked { id: SessionId, pid: Option<u32> },

    #[error("corrupt entry at {}:{}: {source}", path.display(), line)]
    Corrupt {
        path: PathBuf,

        line: usize,

        #[source]
        source: serde_json::Error,
    },
}

pub type Result<T> = std::result::Result<T, SessionError>;

impl SessionError {
    pub fn locked_notice(id: &SessionId, pid: Option<u32>) -> String {
        let head = match pid {
            Some(p) => format!("session '{id}' is already open in another process (pid {p})"),
            None => format!("session '{id}' is already open in another process"),
        };
        format!("{head}\nclose the other instance, or open a different session")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    id: SessionId,
    timestamp: u64,
    cwd: PathBuf,
    model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
}

pub struct JsonlSessionStore {
    root_dir: PathBuf,
    lock: tokio::sync::Mutex<AppendTails>,
}

type AppendTails =
    std::collections::HashMap<String, (u64, Option<std::time::SystemTime>, u64, Option<u64>)>;

impl Default for JsonlSessionStore {
    fn default() -> Self {
        Self::new(default_root().unwrap_or_else(|| PathBuf::from(".gray/sessions")))
    }
}

pub(crate) fn valid_session_id(s: &str) -> bool {
    if s.is_empty()
        || s.len() > 128
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    if matches!(lower.as_str(), "con" | "prn" | "aux" | "nul") {
        return false;
    }
    if lower.len() == 4
        && (lower.starts_with("com") || lower.starts_with("lpt"))
        && lower.as_bytes()[3].is_ascii_digit()
        && lower.as_bytes()[3] != b'0'
    {
        return false;
    }
    true
}

impl JsonlSessionStore {
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
            lock: tokio::sync::Mutex::new(AppendTails::new()),
        }
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    fn fnv1a_hex(s: &str) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:016x}")
    }

    fn remembered_dir(&self) -> PathBuf {
        self.root_dir.join("remembered")
    }

    fn remember_path_for_cwd(&self, cwd: &Path) -> PathBuf {
        let key = cwd
            .canonicalize()
            .unwrap_or_else(|_| cwd.to_path_buf())
            .display()
            .to_string();
        self.remembered_dir()
            .join(format!("{}.txt", Self::fnv1a_hex(&key)))
    }

    pub async fn remember(&self, id: &SessionId, cwd: &Path) {
        if !valid_session_id(id.as_str()) {
            return;
        }
        let dir = self.remembered_dir();
        if ensure_private_dir(&dir).is_err() {
            return;
        }
        let path = self.remember_path_for_cwd(cwd);
        let stored_cwd = cwd
            .canonicalize()
            .unwrap_or_else(|_| cwd.to_path_buf())
            .display()
            .to_string();
        let tmp = dir.join(format!(".tmp-{}", Self::fnv1a_hex(id.as_str())));
        let content = format!("{stored_cwd}\n{}\n", id.as_str());
        if tokio::fs::write(&tmp, content.as_bytes()).await.is_err() {
            return;
        }
        tighten_file_mode(&tmp);
        if tokio::fs::rename(&tmp, &path).await.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
            return;
        }
        tighten_file_mode(&path);
    }

    pub async fn recall(&self, cwd: &Path) -> Option<SessionId> {
        let content = tokio::fs::read_to_string(self.remember_path_for_cwd(cwd))
            .await
            .ok()?;
        let mut lines = content.lines();
        let stored_cwd = PathBuf::from(lines.next()?.trim());
        let current = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let stored = stored_cwd
            .canonicalize()
            .unwrap_or_else(|_| stored_cwd.clone());
        if stored != current && stored_cwd != current {
            return None;
        }
        let id_str = lines.next()?.trim();
        if !valid_session_id(id_str) {
            return None;
        }
        Some(SessionId::new(id_str))
    }

    pub async fn recall_validated(&self, cwd: &Path) -> Option<SessionId> {
        let id = self.recall(cwd).await?;
        let path = self.session_path(&id).ok()?;
        match tokio::fs::metadata(&path).await {
            Ok(m) => {
                if m.is_file() {
                    Some(id)
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }

    pub async fn forget(&self, cwd: &Path) {
        let _ = tokio::fs::remove_file(self.remember_path_for_cwd(cwd)).await;
    }

    pub(crate) async fn exists_on_disk(&self, id: &SessionId) -> bool {
        let Ok(path) = self.session_path(id) else {
            return false;
        };
        tokio::fs::metadata(&path)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
    }

    pub async fn fresh_id(&self) -> SessionId {
        self.fresh_id_with(SessionId::generate).await
    }

    async fn fresh_id_with(&self, mut next_id: impl FnMut() -> SessionId) -> SessionId {
        for _ in 0..FRESH_ID_ATTEMPTS {
            let id = next_id();
            if !self.exists_on_disk(&id).await {
                return id;
            }
        }
        let base = next_id();
        let salt = (uuid::Uuid::new_v4().as_u128() & 0xffff) as u16;
        SessionId::new(format!("{}-{salt:04x}", base.as_str()))
    }

    async fn next_ids_for_append(id: &SessionId, path: &Path) -> Result<(u64, Option<u64>)> {
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(id.clone()));
            }
            Err(e) => return Err(SessionError::Io(e)),
        };

        if !content.ends_with('\n')
            && let Some(cut) = content.rfind('\n')
        {
            let file = tokio::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .await
                .map_err(SessionError::Io)?;
            file.set_len((cut + 1) as u64)
                .await
                .map_err(SessionError::Io)?;
            file.sync_all().await.map_err(SessionError::Io)?;
            log::warn!(target: "gray_session",
                "{}:{} truncated torn tail to last complete record",
                path.display(), cut + 1);
            return Self::scan_entries(id, &content[..cut + 1], path);
        }
        Self::scan_entries(id, &content, path)
    }

    fn session_path(&self, id: &SessionId) -> std::io::Result<PathBuf> {
        let s = id.as_str();
        if !valid_session_id(s) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid session identifier",
            ));
        }
        Ok(self.root_dir.join(format!("{s}.jsonl")))
    }

    fn session_lock_path(&self, id: &SessionId) -> std::io::Result<PathBuf> {
        self.session_path(id)?;
        Ok(self.root_dir.join(format!("{}.lock", id.as_str())))
    }

    async fn lock_session_file(&self, id: &SessionId) -> Result<std::fs::File> {
        let lock_path = self.session_lock_path(id)?;
        ensure_private_dir(&self.root_dir)?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&lock_path)?;
        tighten_file_mode(&lock_path);
        let deadline = std::time::Instant::now() + SESSION_LOCK_TIMEOUT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(SessionError::Io(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            format!("timed out waiting for session lock {}", lock_path.display()),
                        )));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Err(std::fs::TryLockError::Error(e)) => {
                    log::warn!(
                        "session file locking unsupported on {} ({e}); proceeding unlocked",
                        lock_path.display()
                    );
                    return Ok(file);
                }
            }
        }
    }

    fn session_open_path(&self, id: &SessionId) -> std::io::Result<PathBuf> {
        self.session_path(id)?;
        Ok(self.root_dir.join(format!("{}.open", id.as_str())))
    }

    pub async fn acquire_open(&self, id: &SessionId) -> Result<SessionOpenGuard> {
        let open_path = self.session_open_path(id)?;
        ensure_private_dir(&self.root_dir)?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&open_path)?;
        tighten_file_mode(&open_path);

        if let Some(inner) = take_registered_open_lock(&open_path) {
            return Ok(SessionOpenGuard { _inner: inner });
        }

        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                if !wait_out_restart_parent(&mut file, &open_path).await {
                    return Err(SessionError::Locked {
                        id: id.clone(),
                        pid: read_open_holder(&open_path),
                    });
                }
            }
            Err(std::fs::TryLockError::Error(e)) => {
                log::warn!(
                    "session open-lock unsupported on {} ({e}); proceeding unlocked",
                    open_path.display()
                );
            }
        }
        write_open_holder(&mut file, &open_path);
        let inner = std::sync::Arc::new(OpenLockInner { _file: file });
        OPEN_LOCKS
            .lock()
            .expect("open-lock registry poisoned")
            .insert(open_path, std::sync::Arc::downgrade(&inner));
        Ok(SessionOpenGuard { _inner: inner })
    }

    pub async fn open_lock_held(&self, id: &SessionId) -> bool {
        self.open_lock_held_sync(id)
    }

    pub(crate) fn open_lock_held_sync(&self, id: &SessionId) -> bool {
        let Ok(open_path) = self.session_open_path(id) else {
            return false;
        };
        open_lock_held_at(&open_path)
    }

    pub async fn held_open_locks(&self, ids: Vec<SessionId>) -> std::collections::HashSet<String> {
        use futures::StreamExt as _;
        let mut probes = futures::stream::iter(
            ids.into_iter()
                .filter_map(|id| self.session_open_path(&id).ok()),
        )
        .map(|path| {
            tokio::task::spawn_blocking(move || {
                if !open_lock_held_at(&path) {
                    return None;
                }
                path.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
            })
        })
        .buffer_unordered(32);
        let mut held = std::collections::HashSet::new();
        while let Some(joined) = probes.next().await {
            if let Ok(Some(stem)) = joined {
                held.insert(stem);
            }
        }
        held
    }

    pub(crate) fn open_holder_pid(&self, id: &SessionId) -> Option<u32> {
        read_open_holder(&self.session_open_path(id).ok()?)
    }

    async fn quarantine_corrupt_file(path: &Path) {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return;
        };
        let Some(parent) = path.parent() else {
            return;
        };
        let prefix = format!("{stem}.corrupt-");
        let mut max_n = 0u32;
        if let Ok(mut rd) = tokio::fs::read_dir(parent).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(n) = name.strip_prefix(&prefix).and_then(|s| s.parse().ok()) {
                    max_n = max_n.max(n);
                }
            }
        }
        if tokio::fs::rename(path, parent.join(format!("{prefix}{}", max_n + 1)))
            .await
            .is_err()
        {
            return;
        }
        if max_n + 1 > 3 {
            for n in 1..=(max_n + 1 - 3) {
                let _ = tokio::fs::remove_file(parent.join(format!("{prefix}{n}"))).await;
            }
        }
    }
}

pub fn default_root() -> Option<PathBuf> {
    gray_core::paths::gray_home().map(|home| home.join("sessions"))
}

mod words;
use words::*;
mod meta;
use meta::*;
mod origin;
pub use origin::*;
mod scan;
pub(crate) use scan::*;
mod persist;
mod title;
pub use title::*;
mod open_lock;
pub use open_lock::*;

#[path = "session_store_tests.rs"]
#[cfg(test)]
mod tests;
