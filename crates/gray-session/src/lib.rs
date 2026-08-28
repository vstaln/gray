//! Session storage for the Gray agent.
//!
//! This crate provides session persistence and management for conversations,
//! storing each session as a JSONL file (legacy) and — phase 1 — a SQLite
//! store with FTS5 (new).
//!
//! # Architecture & Logging Choice
//! This crate uses the lightweight [`log`] facade (not `tracing`) as it is a leaf
//! library with no spans or asynchronous task hierarchies of its own. Warnings
//! (`log::warn!`) are emitted only on skipped or corrupt data.


use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gray_core::{Message, Role};
use serde::{Deserialize, Serialize};

/// A unique session identifier.
///
/// # Why UUID v4
/// Session IDs must be globally unique, filesystem-safe, coordination-free identifiers;
/// v4 gives 122 random bits with negligible collision probability from a maintained stdlib-grade
/// crate — no counter state or clock needed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    /// Generates a new random session ID using UUID v4.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Creates a session ID from an existing string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns a string slice of the session ID.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SessionId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Monotonically increasing identifier for an entry within a session.
pub type SessionEntryId = u64;

/// Metadata associated with a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Unique identifier for the session.
    pub id: SessionId,
    /// Unix timestamp in milliseconds when the session was created.
    pub timestamp: u64,
    /// Current working directory when the session was started.
    pub cwd: PathBuf,
    /// Model name or identifier used for the session.
    pub model: String,
}

impl SessionMeta {
    /// Creates new session metadata with the given parameters.
    pub fn new(
        id: SessionId,
        timestamp: u64,
        cwd: impl Into<PathBuf>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            id,
            timestamp,
            cwd: cwd.into(),
            model: model.into(),
        }
    }
}

/// A recorded turn or event entry within a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    /// Monotonic sequence ID of this entry in the session.
    pub entry_id: u64,
    /// ID of the parent entry in the session tree or history, or `None` for the root entry.
    pub parent_id: Option<u64>,
    /// Unix timestamp in milliseconds when this entry was created.
    pub timestamp: u64,
    /// The conversation turn message stored in this entry.
    pub message: Message,
}

/// Summary overview of a session for listing operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Unique identifier of the session.
    pub id: SessionId,
    /// Unix timestamp in milliseconds when the session started.
    pub started_at: u64,
    /// Current working directory of the session.
    pub cwd: PathBuf,
    /// The text content of the first user message in the session, if present.
    pub first_user_text: Option<String>,
}

/// Errors that can occur during session storage operations.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Underlying I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization or deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The requested session was not found.
    #[error("session {0} not found")]
    NotFound(SessionId),

    /// A corrupt or malformed entry was encountered in a session file.
    #[error("corrupt entry at {}:{}", path.display(), line)]
    Corrupt {
        /// File path where the corruption occurred.
        path: PathBuf,
        /// 1-based line number of the corrupt entry.
        line: usize,
        /// Underlying JSON parsing error.
        #[source]
        source: serde_json::Error,
    },
}

/// Type alias for results from session operations.
pub type Result<T> = std::result::Result<T, SessionError>;

/// Asynchronous session persistence store interface.
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Creates a new session file initialized with the metadata header.
    async fn create(&self, meta: SessionMeta) -> SessionId;

    /// Appends a new message entry to an existing session.
    async fn append(&self, id: &SessionId, msg: &Message) -> Result<SessionEntryId>;

    /// Loads the metadata and all entries for a given session.
    async fn load(&self, id: &SessionId) -> Result<(SessionMeta, Vec<SessionEntry>)>;

    /// Lists summaries of all sessions in the store sorted by start time ascending.
    async fn list(&self) -> Vec<SessionSummary>;

    /// Deletes a session by ID. Idempotent: returns `Ok(())` even if the session file does not exist.
    async fn delete(&self, id: &SessionId) -> Result<()>;
}

/// Header metadata stored as the first line of a session `.jsonl` file.
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    id: SessionId,
    timestamp: u64,
    cwd: PathBuf,
    model: String,
}

/// A JSONL file-backed session store.
///
/// Each session is stored as a single `.jsonl` file at `<root>/<id>.jsonl`.
/// Line 0 contains the JSON header object (`{"version":1, ...}`), and each subsequent line
/// contains a serialized [`SessionEntry`].
pub struct JsonlSessionStore {
    root_dir: PathBuf,
    lock: tokio::sync::Mutex<()>,
}

impl Default for JsonlSessionStore {
    /// Store rooted at the default directory (`~/.gray/sessions`), falling back
    /// to `.gray/sessions` under the current directory when `$HOME` is unset.
    fn default() -> Self {
        Self::new(default_root().unwrap_or_else(|| PathBuf::from(".gray/sessions")))
    }
}

impl JsonlSessionStore {
    /// Creates a new JSONL session store rooted at the given directory path.
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Returns a reference to the root directory path of the store.
    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    fn session_path(&self, id: &SessionId) -> PathBuf {
        self.root_dir.join(format!("{}.jsonl", id.as_str()))
    }
}

#[async_trait::async_trait]
impl SessionStore for JsonlSessionStore {
    async fn create(&self, meta: SessionMeta) -> SessionId {
        let _guard = self.lock.lock().await;
        let id = meta.id.clone();
        let path = self.session_path(&id);

        if let Err(e) = tokio::fs::create_dir_all(&self.root_dir).await {
            log::warn!(
                "failed to create session root directory {}: {}",
                self.root_dir.display(),
                e
            );
            return id;
        }

        let header = Header {
            version: 1,
            id: meta.id,
            timestamp: meta.timestamp,
            cwd: meta.cwd,
            model: meta.model,
        };

        match serde_json::to_string(&header) {
            Ok(json) => {
                let open_res = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&path)
                    .await;
                match open_res {
                    Ok(mut file) => {
                        use tokio::io::AsyncWriteExt;
                        let line = format!("{}\n", json);
                        if let Err(e) = file.write_all(line.as_bytes()).await {
                            log::warn!(
                                "failed to write session header to {}: {}",
                                path.display(),
                                e
                            );
                        } else if let Err(e) = file.flush().await {
                            log::warn!(
                                "failed to flush session header to {}: {}",
                                path.display(),
                                e
                            );
                        } else if let Err(e) = file.sync_all().await {
                            log::warn!(
                                "failed to sync session header to {}: {}",
                                path.display(),
                                e
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "failed to open session file {}: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
            Err(e) => {
                log::warn!("failed to serialize session header: {}", e);
            }
        }

        id
    }

    async fn append(&self, id: &SessionId, msg: &Message) -> Result<SessionEntryId> {
        let _guard = self.lock.lock().await;
        let path = self.session_path(id);

        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(id.clone()));
            }
            Err(e) => return Err(SessionError::Io(e)),
        };

        let mut lines = content.lines().filter(|l| !l.trim().is_empty());
        if lines.next().is_none() {
            return Err(SessionError::NotFound(id.clone()));
        }

        let mut max_id: Option<u64> = None;
        let mut last_id: Option<u64> = None;

        for line in lines {
            if let Ok(entry) = serde_json::from_str::<SessionEntry>(line) {
                max_id = Some(max_id.map_or(entry.entry_id, |m| m.max(entry.entry_id)));
                last_id = Some(entry.entry_id);
            }
        }

        let next_id = max_id.map_or(0, |m| m + 1);
        let parent_id = last_id;

        let entry = SessionEntry {
            entry_id: next_id,
            parent_id,
            timestamp: now_millis(),
            message: msg.clone(),
        };

        let json = serde_json::to_string(&entry)?;
        let line = format!("{}\n", json);

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .create(false)
            .open(&path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SessionError::NotFound(id.clone())
                } else {
                    SessionError::Io(e)
                }
            })?;

        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;

        Ok(next_id)
    }

    async fn load(&self, id: &SessionId) -> Result<(SessionMeta, Vec<SessionEntry>)> {
        let path = self.session_path(id);
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(id.clone()));
            }
            Err(e) => return Err(SessionError::Io(e)),
        };

        let all_lines: Vec<(usize, &str)> = content
            .lines()
            .enumerate()
            .map(|(idx, line)| (idx + 1, line))
            .filter(|(_, line)| !line.trim().is_empty())
            .collect();

        // Empty or whitespace-only file: parse an empty string so the JSON
        // error surfaces as a Corrupt failure at line 1.
        let (header_line_num, header_str) = all_lines
            .first()
            .map(|&(n, s)| (n, s))
            .unwrap_or((1, ""));
        let header: Header = serde_json::from_str(header_str).map_err(|e| SessionError::Corrupt {
            path: path.clone(),
            line: header_line_num,
            source: e,
        })?;

        let meta = SessionMeta {
            id: header.id,
            timestamp: header.timestamp,
            cwd: header.cwd,
            model: header.model,
        };

        let entry_lines = &all_lines[1..];
        let mut entries = Vec::with_capacity(entry_lines.len());

        for (idx, (line_num, line_str)) in entry_lines.iter().enumerate() {
            let is_final_line = idx == entry_lines.len() - 1;
            match serde_json::from_str::<SessionEntry>(line_str) {
                Ok(entry) => {
                    entries.push(entry);
                }
                Err(e) => {
                    if is_final_line {
                        log::warn!(
                            "ignoring corrupt or torn final line in {}:{}: {}",
                            path.display(),
                            line_num,
                            e
                        );
                    } else {
                        return Err(SessionError::Corrupt {
                            path: path.clone(),
                            line: *line_num,
                            source: e,
                        });
                    }
                }
            }
        }

        Ok((meta, entries))
    }

    async fn list(&self) -> Vec<SessionSummary> {
        let mut read_dir = match tokio::fs::read_dir(&self.root_dir).await {
            Ok(rd) => rd,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!(
                        "failed to read session directory {}: {}",
                        self.root_dir.display(),
                        e
                    );
                }
                return Vec::new();
            }
        };

        let mut summaries = Vec::new();
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }

            let content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("failed to read session file {}: {}", path.display(), e);
                    continue;
                }
            };

            let mut lines = content.lines().filter(|l| !l.trim().is_empty());
            let header_str = match lines.next() {
                Some(h) => h,
                None => {
                    log::warn!("skipping empty session file: {}", path.display());
                    continue;
                }
            };

            let header: Header = match serde_json::from_str(header_str) {
                Ok(h) => h,
                Err(e) => {
                    log::warn!("skipping corrupt header in {}: {}", path.display(), e);
                    continue;
                }
            };

            let mut first_user_text = None;
            for line in lines {
                if let Ok(entry) = serde_json::from_str::<SessionEntry>(line)
                    && entry.message.role == Role::User
                {
                    let text = entry.message.text_content();
                    if !text.is_empty() {
                        first_user_text = Some(text);
                    }
                    break;
                }
            }

            summaries.push(SessionSummary {
                id: header.id,
                started_at: header.timestamp,
                cwd: header.cwd,
                first_user_text,
            });
        }

        summaries.sort_by_key(|s| s.started_at);
        summaries
    }

    async fn delete(&self, id: &SessionId) -> Result<()> {
        let _guard = self.lock.lock().await;
        let path = self.session_path(id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SessionError::Io(e)),
        }
    }
}

/// Returns the default session directory (`~/.gray/sessions`), or `None` if `$HOME` is not set.
pub fn default_root() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".gray").join("sessions"))
}

/// Helper function to return current time in milliseconds since Unix epoch.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gray_core::ContentBlock;
    use serde_json::json;

    #[tokio::test]
    async fn round_trip_create_append_load() {
        let tmp = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(tmp.path());

        let id = SessionId::generate();
        let cwd = tmp.path().to_path_buf();
        let model = "test-model".to_string();
        let meta = SessionMeta {
            id: id.clone(),
            timestamp: 1_700_000_000_000,
            cwd: cwd.clone(),
            model: model.clone(),
        };

        let created_id = store.create(meta.clone()).await;
        assert_eq!(created_id, id);

        let user_msg = Message::user("hello");
        let assistant_msg = Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::text("hi"),
                ContentBlock::tool_use("call_1", "bash", json!({ "command": "echo hi" })),
            ],
        };

        let id0 = store.append(&id, &user_msg).await.unwrap();
        assert_eq!(id0, 0);

        let id1 = store.append(&id, &assistant_msg).await.unwrap();
        assert_eq!(id1, 1);

        let (loaded_meta, entries) = store.load(&id).await.unwrap();
        assert_eq!(loaded_meta.id, meta.id);
        assert_eq!(loaded_meta.timestamp, meta.timestamp);
        assert_eq!(loaded_meta.cwd, meta.cwd);
        assert_eq!(loaded_meta.model, meta.model);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].entry_id, 0);
        assert_eq!(entries[0].parent_id, None);
        assert_eq!(entries[0].message, user_msg);

        assert_eq!(entries[1].entry_id, 1);
        assert_eq!(entries[1].parent_id, Some(0));
        assert_eq!(entries[1].message, assistant_msg);
    }

    #[tokio::test]
    async fn list_summaries_returns_first_user_text() {
        let tmp = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(tmp.path());

        let id1 = SessionId::generate();
        let meta1 = SessionMeta {
            id: id1.clone(),
            timestamp: 1000,
            cwd: tmp.path().to_path_buf(),
            model: "model-1".to_string(),
        };
        store.create(meta1).await;
        store
            .append(&id1, &Message::user("first prompt"))
            .await
            .unwrap();
        store
            .append(&id1, &Message::assistant("assistant reply"))
            .await
            .unwrap();

        let id2 = SessionId::generate();
        let meta2 = SessionMeta {
            id: id2.clone(),
            timestamp: 2000,
            cwd: tmp.path().to_path_buf(),
            model: "model-2".to_string(),
        };
        store.create(meta2).await;
        store
            .append(&id2, &Message::assistant("assistant start without user"))
            .await
            .unwrap();

        let summaries = store.list().await;
        assert_eq!(summaries.len(), 2);

        assert_eq!(summaries[0].id, id1);
        assert_eq!(summaries[0].started_at, 1000);
        assert_eq!(
            summaries[0].first_user_text,
            Some("first prompt".to_string())
        );

        assert_eq!(summaries[1].id, id2);
        assert_eq!(summaries[1].started_at, 2000);
        assert_eq!(summaries[1].first_user_text, None);
    }

    #[tokio::test]
    async fn delete_removes_session() {
        let tmp = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(tmp.path());

        let id = SessionId::generate();
        let meta = SessionMeta {
            id: id.clone(),
            timestamp: 1000,
            cwd: tmp.path().to_path_buf(),
            model: "test-model".to_string(),
        };

        store.create(meta).await;
        store.append(&id, &Message::user("test")).await.unwrap();

        store.delete(&id).await.unwrap();

        let load_res = store.load(&id).await;
        assert!(matches!(load_res, Err(SessionError::NotFound(_))));

        // Second delete is idempotent and returns Ok
        let second_delete = store.delete(&id).await;
        assert!(second_delete.is_ok());
    }

    #[tokio::test]
    async fn resume_append_continues_monotonic_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let id = SessionId::generate();

        {
            let store = JsonlSessionStore::new(tmp.path());
            let meta = SessionMeta {
                id: id.clone(),
                timestamp: 1000,
                cwd: tmp.path().to_path_buf(),
                model: "test-model".to_string(),
            };
            store.create(meta).await;
            let id0 = store.append(&id, &Message::user("msg 0")).await.unwrap();
            let id1 = store
                .append(&id, &Message::assistant("msg 1"))
                .await
                .unwrap();
            assert_eq!(id0, 0);
            assert_eq!(id1, 1);
        }

        // New store instance pointing to same root directory
        let new_store = JsonlSessionStore::new(tmp.path());
        let (meta, entries) = new_store.load(&id).await.unwrap();
        assert_eq!(meta.id, id);
        assert_eq!(entries.len(), 2);

        let id2 = new_store
            .append(&id, &Message::user("msg 2"))
            .await
            .unwrap();
        assert_eq!(id2, 2);

        let (_, entries_after) = new_store.load(&id).await.unwrap();
        assert_eq!(entries_after.len(), 3);
        assert_eq!(entries_after[2].entry_id, 2);
        assert_eq!(entries_after[2].parent_id, Some(1));
        assert_eq!(entries_after[2].message, Message::user("msg 2"));
    }

    #[tokio::test]
    async fn torn_last_line_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(tmp.path());

        let id = SessionId::generate();
        let meta = SessionMeta {
            id: id.clone(),
            timestamp: 1000,
            cwd: tmp.path().to_path_buf(),
            model: "test-model".to_string(),
        };

        store.create(meta).await;
        store.append(&id, &Message::user("hello")).await.unwrap();

        // Manually append garbage bytes directly to the file without newline
        let file_path = tmp.path().join(format!("{}.jsonl", id.as_str()));
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file_path)
            .unwrap();
        f.write_all(b"{\"id\":2,\"parent\"").unwrap();
        f.flush().unwrap();
        drop(f);

        let (loaded_meta, entries) = store.load(&id).await.unwrap();
        assert_eq!(loaded_meta.id, id);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry_id, 0);
        assert_eq!(entries[0].message, Message::user("hello"));
    }

    #[tokio::test]
    async fn session_id_generate_is_uuid_v4() {
        let id = SessionId::generate();
        assert_eq!(id.as_str().len(), 36);
        let parsed = uuid::Uuid::parse_str(id.as_str()).expect("must parse as uuid");
        assert_eq!(parsed.get_version(), Some(uuid::Version::Random));
    }

    #[tokio::test]
    async fn corrupt_non_final_line_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let store = JsonlSessionStore::new(tmp.path());

        let id = SessionId::generate();
        let meta = SessionMeta {
            id: id.clone(),
            timestamp: 1000,
            cwd: tmp.path().to_path_buf(),
            model: "test-model".to_string(),
        };

        store.create(meta).await;
        store.append(&id, &Message::user("hello")).await.unwrap();

        // Manually append a corrupt line followed by a valid line
        let file_path = tmp.path().join(format!("{}.jsonl", id.as_str()));
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file_path)
            .unwrap();
        f.write_all(b"corrupt line here\n").unwrap();
        let valid_entry = SessionEntry {
            entry_id: 1,
            parent_id: Some(0),
            timestamp: 1001,
            message: Message::assistant("world"),
        };
        let valid_json = serde_json::to_string(&valid_entry).unwrap();
        f.write_all(format!("{}\n", valid_json).as_bytes())
            .unwrap();
        f.flush().unwrap();
        drop(f);

        let res = store.load(&id).await;
        assert!(matches!(res, Err(SessionError::Corrupt { line: 3, .. })));
    }
}
