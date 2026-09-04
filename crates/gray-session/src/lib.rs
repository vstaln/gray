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
    /// Optional display title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Title provenance: `"user"` or `"auto"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_source: Option<String>,
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
            title: None,
            title_source: None,
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
    /// Wall-clock turn duration in milliseconds, if measured.
    /// Tokens stay in `usage` (Codex/Pi parity); time lives alongside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
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
    /// Optional display title (mirrors the header).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Title provenance: `"user"` or `"auto"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_source: Option<String>,
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

/// Strips control chars, zero-width/bidi formatting, collapses whitespace, caps at 120 chars.
pub fn sanitize_title(s: &str) -> String {
    let mut kept = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() {
            // Keep real whitespace so words don't glue; drop the rest (NUL, BEL, DEL, ...).
            if c.is_whitespace() {
                kept.push(c);
            }
        } else {
            match c {
                // Zero-width, bidi isolates/overrides, word joiner, BOM.
                '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}' | '\u{200e}'
                | '\u{200f}' | '\u{202a}' | '\u{202b}' | '\u{202c}' | '\u{202d}'
                | '\u{202e}' | '\u{2066}' | '\u{2067}' | '\u{2068}' | '\u{2069}'
                | '\u{061c}' | '\u{2060}' => {}
                _ => kept.push(c),
            }
        }
    }
    let collapsed = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > 120 {
        collapsed.chars().take(120).collect()
    } else {
        collapsed
    }
}

/// First meaningful non-empty line of `text`, word-boundary trimmed with a `…` suffix.
pub fn derive_title(text: &str) -> Option<String> {
    for line in text.lines() {
        let clean = sanitize_title(line);
        if clean.is_empty() {
            continue;
        }
        if clean.chars().count() <= 60 {
            return Some(clean);
        }
        let prefix: String = clean.chars().take(60).collect();
        let cut = prefix.rfind(' ').map(|i| prefix[..i].to_string()).unwrap_or(prefix);
        return Some(format!("{}…", cut.trim_end()));
    }
    None
}

/// Header metadata stored as the first line of a session `.jsonl` file.
#[derive(Debug, Serialize, Deserialize)]
struct Header {
    version: u32,
    id: SessionId,
    timestamp: u64,
    cwd: PathBuf,
    model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title_source: Option<String>,
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

    /// Moves a corrupt session file aside as `<stem>.corrupt-<n>`, keeping the newest 3.
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
        if tokio::fs::rename(path, parent.join(format!("{prefix}{}", max_n + 1))).await.is_err() {
            return;
        }
        if max_n + 1 > 3 {
            for n in 1..=(max_n + 1 - 3) {
                let _ = tokio::fs::remove_file(parent.join(format!("{prefix}{n}"))).await;
            }
        }
    }

    /// Resolves an id prefix: exact stem wins, else exactly-one `starts_with` match, else None.
    /// Prefixes containing `/`, `\`, or `..` are rejected (path traversal).
    pub fn resolve_session_id(&self, prefix: &str) -> Option<SessionId> {
        if prefix.contains('/') || prefix.contains('\\') || prefix.contains("..") {
            return None;
        }
        if self.root_dir.join(format!("{prefix}.jsonl")).is_file() {
            return Some(SessionId::new(prefix));
        }
        let mut hit = None;
        for entry in std::fs::read_dir(&self.root_dir).ok()?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !stem.starts_with(prefix) {
                continue;
            }
            if hit.is_some() {
                return None;
            }
            hit = Some(SessionId::new(stem));
        }
        hit
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
            title: meta.title,
            title_source: meta.title_source,
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
        self.append_with_usage_and_duration(id, msg, usage, None).await
    }

    pub async fn append_with_usage_and_duration(
        &self,
        id: &SessionId,
        msg: &Message,
        usage: Option<gray_core::event::Usage>,
        duration_ms: Option<u64>,
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
            duration_ms,
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
        let header: Header = match serde_json::from_str(header_str) {
            Ok(h) => h,
            Err(e) => {
                Self::quarantine_corrupt_file(&path).await;
                return Err(SessionError::Corrupt {
                    path: path.clone(),
                    line: header_line_num,
                    source: e,
                });
            }
        };

        let meta = SessionMeta {
            id: header.id,
            timestamp: header.timestamp,
            cwd: header.cwd,
            model: header.model,
            title: header.title,
            title_source: header.title_source,
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
                    Self::quarantine_corrupt_file(&path).await;
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
                title: header.title,
                title_source: header.title_source,
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

    /// Sets an explicit user title; always wins over auto titles.
    pub async fn set_user_title(&self, id: &SessionId, title: &str) -> Result<()> {
        self.set_title_inner(id, title, "user", true).await.map(|_| ())
    }

    /// Sets a derived title unless a user title is present. Returns whether it wrote.
    pub async fn set_auto_title(&self, id: &SessionId, title: &str) -> Result<bool> {
        self.set_title_inner(id, title, "auto", false).await
    }

    async fn set_title_inner(
        &self,
        id: &SessionId,
        title: &str,
        source: &str,
        force: bool,
    ) -> Result<bool> {
        let _guard = self.lock.lock().await;
        let path = self.session_path(id);
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(id.clone()));
            }
            Err(e) => return Err(SessionError::Io(e)),
        };
        let header_end = content.find('\n').map(|i| i + 1).unwrap_or(content.len());
        let (header_str, rest) = content.split_at(header_end);
        let mut header: Header = serde_json::from_str(header_str.trim_end()).map_err(|e| {
            SessionError::Corrupt {
                path: path.clone(),
                line: 1,
                source: e,
            }
        })?;
        if !force && matches!(header.title_source.as_deref(), Some("user")) {
            return Ok(false);
        }
        let clean = sanitize_title(title);
        if clean.is_empty() {
            header.title = None;
            header.title_source = None;
        } else {
            header.title = Some(clean);
            header.title_source = Some(source.to_string());
        }
        let mut out = serde_json::to_string(&header)?;
        out.push('\n');
        out.push_str(rest);
        tokio::fs::write(&path, out).await?;
        Ok(true)
    }

    /// Returns `base` when unused, else the first unused `base #N` (N >= 2).
    pub async fn next_title_in_lineage(&self, base: &str) -> String {
        let titles: std::collections::HashSet<String> =
            self.list().await.into_iter().filter_map(|s| s.title).collect();
        if !titles.contains(base) {
            return base.to_string();
        }
        let mut n = 2u32;
        loop {
            let candidate = format!("{base} #{n}");
            if !titles.contains(candidate.as_str()) {
                return candidate;
            }
            n += 1;
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

    #[tokio::test]
    async fn persists_turn_duration_ms() {
        let dir = tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let id = store.create(SessionMeta::new(SessionId::new("s1"), 1, "/tmp", "test")).await.unwrap();
        store
            .append_with_usage_and_duration(&id, &Message::user("hi"), Some(gray_core::event::Usage::new(10, 5)), Some(6250))
            .await
            .unwrap();
        let (_, entries) = store.load(&id).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].duration_ms, Some(6250));
    }

    #[tokio::test]
    async fn legacy_entry_without_duration_loads_as_none() {
        let dir = tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let id = store.create(SessionMeta::new(SessionId::new("s1"), 1, "/tmp", "test")).await.unwrap();
        let path = store.session_path(&id);
        let mut raw = tokio::fs::read_to_string(&path).await.unwrap();
        // Legacy entry shape: no duration_ms field.
        raw.push_str(r#"{"entry_id":0,"parent_id":null,"timestamp":1,"message":{"role":"user","content":[{"type":"text","text":"hi"}]},"usage":null}"#);
        raw.push('\n');
        tokio::fs::write(&path, raw).await.unwrap();
        let (_, entries) = store.load(&id).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].duration_ms, None);
    }

    #[test]
    fn sanitize_title_collapses_and_strips() {
        assert_eq!(sanitize_title("  hello   world  "), "hello world");
        assert_eq!(sanitize_title("a\u{200b}b\u{202e}c\x07d"), "abcd");
        assert_eq!(sanitize_title("   "), "");
        assert_eq!(sanitize_title(&"x".repeat(200)).chars().count(), 120);
    }

    #[test]
    fn derive_title_picks_first_meaningful_line() {
        assert_eq!(derive_title(""), None);
        assert_eq!(derive_title("  \n \n"), None);
        assert_eq!(
            derive_title("hello world\nsecond"),
            Some("hello world".to_string())
        );
        assert_eq!(
            derive_title("\n\n  Real Title  \nsecond"),
            Some("Real Title".to_string())
        );
        let t = derive_title(&"word ".repeat(30)).unwrap();
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= 61);
    }

    #[tokio::test]
    async fn auto_title_yields_to_user_title() {
        let dir = tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let id = store
            .create(SessionMeta::new(SessionId::new("t1"), 1, "/tmp", "m"))
            .await
            .unwrap();
        assert!(store.set_auto_title(&id, "Auto One").await.unwrap());
        let (meta, _) = store.load(&id).await.unwrap();
        assert_eq!(meta.title.as_deref(), Some("Auto One"));
        assert_eq!(meta.title_source.as_deref(), Some("auto"));
        assert!(store.set_auto_title(&id, "Auto Two").await.unwrap());
        store.set_user_title(&id, "Mine").await.unwrap();
        assert!(!store.set_auto_title(&id, "Auto Three").await.unwrap());
        let (meta, _) = store.load(&id).await.unwrap();
        assert_eq!(meta.title.as_deref(), Some("Mine"));
        assert_eq!(meta.title_source.as_deref(), Some("user"));
    }

    #[test]
    fn resolve_session_id_exact_prefix_and_ambiguous() {
        let dir = tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        for id in ["abc", "abcdef11", "abcdef22", "xyz999"] {
            std::fs::write(dir.path().join(format!("{id}.jsonl")), "{}\n").unwrap();
        }
        // Exact stem wins even though others share the prefix.
        assert_eq!(store.resolve_session_id("abc").unwrap().as_str(), "abc");
        // Two matches, no exact hit: ambiguous.
        assert!(store.resolve_session_id("abcdef").is_none());
        // Exactly one prefix match resolves.
        assert_eq!(store.resolve_session_id("abcdef1").unwrap().as_str(), "abcdef11");
        assert_eq!(store.resolve_session_id("xyz").unwrap().as_str(), "xyz999");
        assert!(store.resolve_session_id("nope").is_none());
    }

    #[test]
    fn resolve_session_id_rejects_traversal() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("sessions");
        std::fs::create_dir_all(&root).unwrap();
        let store = JsonlSessionStore::new(&root);
        // File outside the root: reachable pre-fix via the `is_file` join.
        std::fs::write(dir.path().join("evil.jsonl"), "{}\n").unwrap();
        assert!(store.resolve_session_id("../evil").is_none());
        assert!(store.resolve_session_id("..").is_none());
        // Exact stem containing `..` must still be rejected.
        std::fs::write(root.join("a..b.jsonl"), "{}\n").unwrap();
        assert!(store.resolve_session_id("a..b").is_none());
        assert!(store.resolve_session_id("a/b").is_none());
        assert!(store.resolve_session_id("a\\b").is_none());
    }

    #[tokio::test]
    async fn next_title_in_lineage_finds_first_unused() {
        let dir = tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        assert_eq!(store.next_title_in_lineage("Base").await, "Base");
        let a = store
            .create(SessionMeta::new(SessionId::new("a"), 1, "/tmp", "m"))
            .await
            .unwrap();
        store.set_user_title(&a, "Base").await.unwrap();
        assert_eq!(store.next_title_in_lineage("Base").await, "Base #2");
        let b = store
            .create(SessionMeta::new(SessionId::new("b"), 2, "/tmp", "m"))
            .await
            .unwrap();
        store.set_user_title(&b, "Base #2").await.unwrap();
        assert_eq!(store.next_title_in_lineage("Base").await, "Base #3");
    }

    #[tokio::test]
    async fn corrupt_header_is_quarantined_on_load() {
        let dir = tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let id = store
            .create(SessionMeta::new(SessionId::new("bad1"), 1, "/tmp", "m"))
            .await
            .unwrap();
        let path = store.session_path(&id);
        tokio::fs::write(&path, "not json\n").await.unwrap();
        let err = store.load(&id).await.unwrap_err();
        assert!(matches!(err, SessionError::Corrupt { .. }));
        assert!(!path.exists());
        assert!(dir.path().join("bad1.corrupt-1").exists());
        assert!(store.list().await.is_empty());
    }

    #[tokio::test]
    async fn corrupt_header_is_quarantined_on_list_and_keeps_newest_3() {
        let dir = tempdir().unwrap();
        let store = JsonlSessionStore::new(dir.path());
        let good = store
            .create(SessionMeta::new(SessionId::new("good"), 1, "/tmp", "m"))
            .await
            .unwrap();
        let bad = SessionId::new("bad2");
        let bad_path = store.session_path(&bad);
        for _ in 0..5 {
            tokio::fs::write(&bad_path, "not json\n").await.unwrap();
            let summaries = store.list().await;
            assert_eq!(summaries.len(), 1);
            assert_eq!(summaries[0].id, good);
        }
        let mut kept: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("bad2.corrupt-"))
            .collect();
        kept.sort();
        assert_eq!(kept, vec!["bad2.corrupt-3", "bad2.corrupt-4", "bad2.corrupt-5"]);
        assert!(!bad_path.exists());
    }
}

