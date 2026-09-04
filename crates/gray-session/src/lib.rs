//! Session storage for the Gray agent.
//!
//! This crate provides session persistence and management for conversations,
//! storing each session as a JSONL file.
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

// NOTE (ponytail-audit #11): earlier `From<String>`/`From<&str>`/`AsRef<str>`
// impls were deleted — every caller uses `new`/`generate`/`as_str`.
// `Display` stays: it formats `{sid}` in status lines and `NotFound`.

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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
    /// Token usage recorded for this turn, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<gray_core::event::Usage>,
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

impl JsonlSessionStore {
    // ponytail-audit #11: returns Result — callers decide what a failed
    // session write means instead of five nested warn-and-continue arms.
    pub async fn create(&self, meta: SessionMeta) -> Result<SessionId> {
        let _guard = self.lock.lock().await;
        let id = meta.id.clone();
        let path = self.session_path(&id);

        tokio::fs::create_dir_all(&self.root_dir).await?;

        let header = Header {
            version: 1,
            id: meta.id,
            timestamp: meta.timestamp,
            cwd: meta.cwd,
            model: meta.model,
        };

        let json = serde_json::to_string(&header)?;
        let line = format!("{json}\n");
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .await?;
        use tokio::io::AsyncWriteExt;
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;

        Ok(id)
    }

    pub async fn append(&self, id: &SessionId, msg: &Message) -> Result<SessionEntryId> {
        self.append_with_usage(id, msg, None).await
    }

    pub async fn append_with_usage(
        &self,
        id: &SessionId,
        msg: &Message,
        usage: Option<gray_core::event::Usage>,
    ) -> Result<SessionEntryId> {
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
            usage,
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

    pub async fn load(&self, id: &SessionId) -> Result<(SessionMeta, Vec<SessionEntry>)> {
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

    pub async fn list(&self) -> Vec<SessionSummary> {
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

    pub async fn delete(&self, id: &SessionId) -> Result<()> {
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
    if let Some(gh) = std::env::var_os("GRAY_HOME") {
        return Some(PathBuf::from(gh).join("sessions"));
    }
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
    use tempfile::tempdir;

    #[tokio::test]
    async fn load_ignores_torn_final_line_but_preserves_prior_entries() {
        let dir = tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let id = store.create(SessionMeta::new(SessionId::new("s1"), 1, "/tmp", "test")).await.unwrap();
        store.append(&id, &Message::user("hello")).await.unwrap();
        let path = store.session_path(&id);
        let mut raw = tokio::fs::read_to_string(&path).await.unwrap();
        raw.push_str("{torn\n");
        tokio::fs::write(&path, raw).await.unwrap();
        let (_, entries) = store.load(&id).await.unwrap();
        assert_eq!(entries.len(), 1);
    }
}

