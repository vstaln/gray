//! Session storage for the Gray agent.
//!
//! This module provides session persistence and management for conversations,
//! storing each session as a JSONL file.
//!
//! # Architecture & Logging Choice
//! This crate uses the lightweight [`log`] facade (not `tracing`) as it is a leaf
//! library with no spans or asynchronous task hierarchies of its own. Warnings
//! (`log::warn!`) are emitted only on skipped or corrupt data.
//!
//! # Locking: cross-process file lock first, in-memory mutex second
//! The store mutex is per-instance. Every read-modify-append/replace path
//! (`append`, `append_compaction_replacement`) additionally
//! holds a per-session cross-process exclusive lock (`<root>/<id>.lock` via
//! `std::fs::File::lock`) for the whole critical section. Lock order is always
//! file-lock -> in-memory mutex; never the reverse. The lock file is opened
//! (created 0600 on unix) then `try_lock` is retried up to 30s; the
//! open file handle is kept alive until the end of the method — closing it
//! releases the flock, including on process death. `create` uses atomic
//! `create_new` and needs no lock; `load`/`list` are lock-free reads (a torn
//! final line is ignored on load, appends refuse a damaged tail).
//!
//! # Storage privacy: private resumable storage vs redacted export (ONE rule)
//! The store is private resumable storage: `<root>` is 0700 and every
//! `.jsonl`/`.lock`/tmp file is 0600 on unix (best-effort chmod after
//! `create_dir_all`; Windows ACLs are not hardened — std has no portable
//! owner-only flag). The store persists exactly what callers hand it; it does
//! NOT redact.
//!
//! Divergence (intentional, documented here — do not "centralize" by changing
//! replay fidelity):
//! - `gray::print` (`save_session`, `append_new_messages`) pre-scrubs via
//!   `gray_core::redaction::redact_message` — secret-scoped: secret-bearing
//!   blocks are redacted (paths in the same block go too), secret-free blocks
//!   persist verbatim for resume fidelity.
//! - REPL (`repl::session::persist_turn_messages`, compaction paths,
//!   `acp_cmds`) persists RAW messages
//!   for exact replay fidelity (tool args/results, secrets needed to reproduce
//!   the turn). Safety comes from the 0700/0600 store, not from scrubbing.
//!
//!   Redacted export (logs, receipts, `redact_for_disclosure`) is a separate
//!   disclosure path and must never be confused with resumable storage.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gray_core::{Message, Role};
use serde::{Deserialize, Serialize};

/// On-disk session format version this build reads/writes.
const SUPPORTED_SESSION_VERSION: u32 = 1;
/// How long a writer waits for a held per-session file lock before failing.
const SESSION_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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

// NOTE: earlier `From<String>`/`From<&str>`/`AsRef<str>`
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

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEntry {
    /// Compaction boundary marker: when true, all prior entries are superseded
    /// and reload replays only the entries after the last marker. Old files
    /// omit it (`default` = false), so they keep loading whole.
    #[serde(default, skip_serializing_if = "is_false")]
    pub compaction_boundary: bool,
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
    /// Tokens stay in `usage`; time lives alongside it.
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
    /// The text content of the most recent user message in the session, if
    /// present — what the resume list previews, so a long session reads as
    /// what it was last about rather than how it opened.
    #[serde(default)]
    pub last_user_text: Option<String>,
    /// Unix timestamp in milliseconds of the most recent entry in the session
    /// (any role), i.e. when the session was last active. Falls back to
    /// [`started_at`](Self::started_at) for a session that has no entries yet.
    #[serde(default)]
    pub last_message_at: u64,
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

    /// A session with this ID already exists (create never truncates).
    #[error("session {0} already exists")]
    AlreadyExists(SessionId),

    /// A corrupt or malformed entry was encountered in a session file.
    /// `source` carries the detail (JSON syntax or structural validation:
    /// unsupported version, id mismatch, duplicate id, bad parent chain), so
    /// it is part of the display — `to_string()` alone must stay clear.
    #[error("corrupt entry at {}:{}: {source}", path.display(), line)]
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

/// One policy for session identifiers: 1..=128 bytes of ASCII alphanumerics,
/// `-`, `_` — and never a Windows reserved device basename (case-insensitive):
/// `CON.jsonl` opens the console device, not a file.
fn valid_session_id(s: &str) -> bool {
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

    /// FNV-1a 64-bit hex of `s` (no new deps): stable pointer-file names
    /// for remembered workspace sessions.
    fn fnv1a_hex(s: &str) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:016x}")
    }

    /// Directory holding one remembered-session pointer per cwd hash.
    /// A directory (not `.jsonl` files) so [`Self::list`] never scans it.
    fn remembered_dir(&self) -> PathBuf {
        self.root_dir.join("remembered")
    }

    /// Pointer path for `cwd`: hash of the canonical path when it resolves,
    /// else the raw path (same rule on write and read, so they agree).
    fn remember_path_for_cwd(&self, cwd: &Path) -> PathBuf {
        let key = cwd
            .canonicalize()
            .unwrap_or_else(|_| cwd.to_path_buf())
            .display()
            .to_string();
        self.remembered_dir()
            .join(format!("{}.txt", Self::fnv1a_hex(&key)))
    }

    /// Best-effort write of the remembered session for `cwd` (`cwd\nid`,
    /// 0600, atomic tmp+rename). Never fails the caller: a missing pointer
    /// just means the next `-c` pays one full scan.
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

    /// Recalled session id for `cwd`, or `None` when no/invalid pointer.
    /// Verifies the stored cwd still matches (hash collisions, moved dirs)
    /// and the id is well-formed — never trusts the file blindly.
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

    /// Recalled id that still has a session file on disk (`None` after
    /// prune/delete — the caller then falls back to the list scan).
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

    /// `(next_id, parent_id)` for append paths: reads the file and runs the
    /// same [`Self::scan_entries`] validation the load path uses, so a torn
    /// tail / corrupt entry is refused with its real line number.
    // full rescan per append (O(n²) per session); restore a tail
    // probe + length cursor if append latency ever shows up.
    async fn next_ids_for_append(id: &SessionId, path: &Path) -> Result<(u64, Option<u64>)> {
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(SessionError::NotFound(id.clone()));
            }
            Err(e) => return Err(SessionError::Io(e)),
        };
        Self::scan_entries(id, &content, path)
    }

    /// Storage path for `id`, validated at the boundary: only ASCII
    /// alphanumerics, `-`, `_` (covers UUIDs and safe legacy IDs). Anything
    /// else — `../`, absolute paths, NUL — is rejected before touching the
    /// filesystem.
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

    /// Sibling lock token for `id` (`<root>/<id>.lock`). Validates the id via
    /// [`Self::session_path`] first so traversal ids never reach the fs.
    fn session_lock_path(&self, id: &SessionId) -> std::io::Result<PathBuf> {
        self.session_path(id)?;
        Ok(self.root_dir.join(format!("{}.lock", id.as_str())))
    }

    /// Acquires the per-session cross-process exclusive lock, creating the
    /// root (0700) and lock file (0600) as needed. File lock first, memory
    /// mutex second — callers must hold the returned `File` alive for the
    /// whole read-modify-write section, then take `self.lock`. Retries
    /// `WouldBlock` until [`SESSION_LOCK_TIMEOUT`]; any other lock error
    /// degrades to unlocked-with-warning (availability over mutual exclusion
    /// on filesystems without flock).
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

/// Bytes read from each end of a session file to build its summary. The
/// first user message and the newest entry both live near an end; the
/// middle of a conversation cannot change what the row says.
const SUMMARY_END_BYTES: u64 = 16 * 1024;

/// Extra bytes allowed to find the line boundary a cut landed in, so a
/// slice never starts or ends mid-line.
const BOUNDARY_SCAN_BYTES: u64 = 64 * 1024;

/// The two ends of a session file, cut on line boundaries: `head` starts
/// at byte 0 and ends just after a newline; `tail` starts just after a
/// newline and runs to EOF. No line is ever split, so every line handed to
/// the JSON parser is whole. For a file too small to have two ends the head
/// is the whole file and the tail is empty.
struct SessionSlices {
    head: String,
    tail: String,
}

/// Read both ends of a session file. Synchronous by design: it runs
/// inside one blocking-pool task per file, so plain `std::fs` keeps the
/// whole file to five cheap reads with no per-read task dispatch.
fn read_session_ends(path: &Path) -> Option<SessionSlices> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok().or_else(|| {
        log::warn!("failed to open session file {}", path.display());
        None
    })?;
    let len = file.metadata().ok()?.len();
    if len == 0 {
        return None;
    }

    // Head: from byte 0, extended past the first newline at or after the
    // cut so the last head line is complete.
    let head_len = SUMMARY_END_BYTES.min(len);
    let mut head_bytes = vec![0u8; head_len as usize];
    file.read_exact(&mut head_bytes).ok()?;
    if head_len < len && !head_bytes.last().is_some_and(|b| *b == b'\n') {
        let mut scan = vec![0u8; BOUNDARY_SCAN_BYTES as usize];
        let mut got = 0usize;
        while got < scan.len() {
            let n = file.read(&mut scan[got..]).ok()?;
            if n == 0 {
                break;
            }
            got += n;
            if scan[..got].contains(&b'\n') {
                break;
            }
        }
        match scan[..got].iter().position(|b| *b == b'\n') {
            Some(at) => head_bytes.extend_from_slice(&scan[..=at]),
            // One line longer than the whole scan budget: the header is
            // unterminated in what we read, so read the file rather than
            // guess (a mid-write file stays a mid-write file).
            None => return read_session_whole(path),
        }
    }
    let head = String::from_utf8_lossy(&head_bytes).into_owned();

    // Tail: the last SUMMARY_END_BYTES, advanced to the next newline so it
    // begins on a line boundary.
    if len <= head_bytes.len() as u64 + SUMMARY_END_BYTES {
        return Some(SessionSlices {
            head,
            tail: String::new(),
        });
    }
    file.seek(SeekFrom::Start(len - SUMMARY_END_BYTES)).ok()?;
    let mut tail_bytes = Vec::with_capacity(SUMMARY_END_BYTES as usize);
    file.read_to_end(&mut tail_bytes).ok()?;
    let text = String::from_utf8_lossy(&tail_bytes).into_owned();
    let tail = match text.find('\n') {
        Some(at) => text[at + 1..].to_string(),
        None => return read_session_whole(path),
    };
    Some(SessionSlices { head, tail })
}

fn read_session_whole(path: &Path) -> Option<SessionSlices> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("failed to read session file {}: {}", path.display(), e);
            return None;
        }
    };
    Some(SessionSlices {
        head: content,
        tail: String::new(),
    })
}

impl JsonlSessionStore {
    // Returns Result — callers decide what a failed
    // session write means instead of five nested warn-and-continue arms.
    pub async fn create(&self, meta: SessionMeta) -> Result<SessionId> {
        let _guard = self.lock.lock().await;
        let id = meta.id.clone();
        let path = self.session_path(&id)?;

        ensure_private_dir(&self.root_dir)?;

        let header = Header {
            version: SUPPORTED_SESSION_VERSION,
            id: meta.id,
            timestamp: meta.timestamp,
            cwd: meta.cwd,
            model: meta.model,
        };

        let json = serde_json::to_string(&header)?;
        let line = format!("{json}\n");
        // create_new: an existing ID is AlreadyExists, never a silent
        // truncation of live history. Mode 0600 on unix at creation.
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        match options.open(&path).await {
            Ok(mut file) => {
                use tokio::io::AsyncWriteExt;
                file.write_all(line.as_bytes()).await?;
                file.flush().await?;
                file.sync_all().await?;
                tighten_file_mode(&path);
                self.remember(&id, &header.cwd).await;
                Ok(id)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(SessionError::AlreadyExists(id))
            }
            Err(e) => Err(SessionError::Io(e)),
        }
    }

    /// Refuse a damaged tail instead of appending past it: without the
    /// trailing newline the last record is torn, and appending would
    /// merge with it (or strand it as interior corruption). Repair via
    /// the quarantine flow, then append. Returns `(next_id, parent_id)`.
    fn scan_entries(id: &SessionId, content: &str, path: &Path) -> Result<(u64, Option<u64>)> {
        let mut lines = content.lines().filter(|l| !l.trim().is_empty());
        if lines.next().is_none() {
            return Err(SessionError::NotFound(id.clone()));
        }
        if !content.ends_with('\n') {
            return Err(SessionError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session has an incomplete tail; repair before appending",
            )));
        }
        let mut max_id: Option<u64> = None;
        let mut last_id: Option<u64> = None;
        for (n, line) in lines.enumerate() {
            let entry = serde_json::from_str::<SessionEntry>(line).map_err(|source| {
                SessionError::Corrupt {
                    path: path.to_path_buf(),
                    // +2: header line + 1-based.
                    line: n + 2,
                    source,
                }
            })?;
            max_id = Some(max_id.map_or(entry.entry_id, |m| m.max(entry.entry_id)));
            last_id = Some(entry.entry_id);
        }
        Ok((max_id.map_or(0, |m| m + 1), last_id))
    }

    pub async fn append(&self, id: &SessionId, msg: &Message) -> Result<SessionEntryId> {
        self.append_with_usage_and_duration(id, msg, None, None)
            .await
    }

    pub async fn append_with_usage_and_duration(
        &self,
        id: &SessionId,
        msg: &Message,
        usage: Option<gray_core::event::Usage>,
        duration_ms: Option<u64>,
    ) -> Result<SessionEntryId> {
        // Lock order file -> memory (see module docs); `_lock_file` stays
        // alive for the whole read-modify-append.
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;

        let (next_id, parent_id) = Self::next_ids_for_append(id, &path).await?;

        let entry = SessionEntry {
            compaction_boundary: false,
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
        tighten_file_mode(&path);

        Ok(next_id)
    }

    /// Appends a compaction replacement: one boundary marker plus the active
    /// (post-compact) messages, in a single locked batch. Reload replays only
    /// what follows the last marker, so the pre-compact history stays on disk
    /// for recovery without re-entering the active transcript (previously the
    /// replacement was appended bare, and reload replayed original +
    /// replacement duplicated with compaction silently undone).
    pub async fn append_compaction_replacement(
        &self,
        id: &SessionId,
        replacement: &[Message],
    ) -> Result<()> {
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;

        let (mut next_id, mut parent_id) = Self::next_ids_for_append(id, &path).await?;

        let mut out = String::new();
        // Marker first, then the replacement, one entry per line.
        let mut msgs = Vec::with_capacity(replacement.len() + 1);
        msgs.push((
            true,
            Message::system(
                "compaction boundary: entries before this point are superseded; replay starts after it",
            ),
        ));
        for msg in replacement {
            msgs.push((false, msg.clone()));
        }
        for (boundary, msg) in msgs {
            let entry = SessionEntry {
                compaction_boundary: boundary,
                entry_id: next_id,
                parent_id,
                timestamp: now_millis(),
                message: msg,
                usage: None,
                duration_ms: None,
            };
            let json = serde_json::to_string(&entry)?;
            out.push_str(&json);
            out.push('\n');
            parent_id = Some(next_id);
            next_id += 1;
        }

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

        file.write_all(out.as_bytes()).await?;
        file.flush().await?;
        file.sync_all().await?;
        tighten_file_mode(&path);

        Ok(())
    }

    /// Semantic validation over the full on-disk entry list (before the
    /// compaction drain): unsupported versions, filename-vs-header ID
    /// mismatches, duplicate entry IDs, and invalid parent chains (missing
    /// parents, forward references). A cycle always contains a forward
    /// reference, so the parent-precedes-entry check rejects it here.
    /// Returns `Corrupt` with a custom message so existing `matches!(Corrupt)`
    /// callers (e.g. strict resume) keep reporting it as corruption; the file
    /// is NOT quarantined here — rejection only. Old version-1 linear files
    /// pass unchanged.
    fn validate_loaded_graph(
        path: &Path,
        expected_id: &SessionId,
        header_line_num: usize,
        header: &Header,
        entries: &[SessionEntry],
        entry_line_nums: &[usize],
    ) -> Result<()> {
        use serde::de::Error as _;
        if header.version != SUPPORTED_SESSION_VERSION {
            return Err(SessionError::Corrupt {
                path: path.to_path_buf(),
                line: header_line_num,
                source: serde_json::Error::custom(format!(
                    "unsupported session version {} (expected {})",
                    header.version, SUPPORTED_SESSION_VERSION
                )),
            });
        }
        if header.id != *expected_id {
            return Err(SessionError::Corrupt {
                path: path.to_path_buf(),
                line: header_line_num,
                source: serde_json::Error::custom(format!(
                    "session id mismatch: filename '{}' != header id '{}'",
                    expected_id.as_str(),
                    header.id.as_str()
                )),
            });
        }
        let mut seen: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            if let Some(prev) = seen.insert(entry.entry_id, i) {
                let _ = prev;
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line: entry_line_nums.get(i).copied().unwrap_or(0),
                    source: serde_json::Error::custom(format!(
                        "duplicate entry id {}",
                        entry.entry_id
                    )),
                });
            }
        }
        let index_of = |eid: u64| seen.get(&eid).copied();
        for (i, entry) in entries.iter().enumerate() {
            let Some(parent) = entry.parent_id else {
                continue;
            };
            let line = entry_line_nums.get(i).copied().unwrap_or(0);
            if parent == entry.entry_id {
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line,
                    source: serde_json::Error::custom(format!(
                        "parent cycle: entry {} is its own parent",
                        entry.entry_id
                    )),
                });
            }
            let Some(pi) = index_of(parent) else {
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line,
                    source: serde_json::Error::custom(format!(
                        "missing parent {} for entry {}",
                        parent, entry.entry_id
                    )),
                });
            };
            if pi >= i {
                return Err(SessionError::Corrupt {
                    path: path.to_path_buf(),
                    line,
                    source: serde_json::Error::custom(format!(
                        "parent {} of entry {} must precede it (forward reference)",
                        parent, entry.entry_id
                    )),
                });
            }
        }
        Ok(())
    }

    /// Remove superseded on-disk history from the hot file without losing it.
    /// Streams records into a replacement, retaining the complete original in
    /// archive. Only valid linear histories are rewritten; branches/torn files
    /// remain untouched for the normal loader's validation/recovery rules.
    pub async fn maintain(&self, id: &SessionId) -> Result<bool> {
        use std::io::{BufRead, Seek, SeekFrom, Write};
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        if file.metadata()?.len() < 8 * 1024 * 1024 {
            return Ok(false);
        }
        let mut reader = std::io::BufReader::new(file);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let header: Header = match serde_json::from_str(&line) {
            Ok(header) => header,
            Err(_) => return Ok(false),
        };
        if header.version != SUPPORTED_SESSION_VERSION || header.id != *id {
            return Ok(false);
        }
        let mut replacement = tempfile::NamedTempFile::new_in(&self.root_dir)?;
        serde_json::to_writer(&mut replacement, &header)?;
        writeln!(replacement)?;
        let header_end = replacement.stream_position()?;
        let mut previous = None;
        let mut next = 0u64;
        let mut boundary = false;
        loop {
            line.clear();
            // A single enormous record cannot allocate unbounded memory here.
            // Return untouched so maintenance never truncates valid user data.
            let n = std::io::Read::take(&mut reader, 16 * 1024 * 1024 + 1).read_line(&mut line)?;
            if n == 0 {
                break;
            }
            if !line.ends_with('\n') || n > 16 * 1024 * 1024 {
                return Ok(false);
            }
            if line.trim().is_empty() {
                continue;
            }
            let mut entry: SessionEntry = match serde_json::from_str(&line) {
                Ok(entry) => entry,
                Err(_) => return Ok(false),
            };
            if entry.parent_id != previous || previous.is_some_and(|p| entry.entry_id <= p) {
                return Ok(false);
            }
            previous = Some(entry.entry_id);
            if entry.compaction_boundary {
                replacement.as_file().set_len(header_end)?;
                replacement.seek(SeekFrom::Start(header_end))?;
                next = 0;
                boundary = true;
                continue;
            }
            // Until a compaction marker is found these records cannot be part
            // of the replacement. Do not serialize megabytes only to truncate
            // them at the marker (the original is archived intact below).
            if !boundary {
                continue;
            }
            entry.entry_id = next;
            entry.parent_id = next.checked_sub(1);
            next += 1;
            serde_json::to_writer(&mut replacement, &entry)?;
            writeln!(replacement)?;
        }
        if !boundary {
            return Ok(false);
        }
        replacement.as_file().sync_all()?;
        self.archive_original(id, &path)?;
        replacement
            .persist(&path)
            .map_err(|e| SessionError::Io(e.error))?;
        Ok(true)
    }

    /// Keep the complete original beside the rewritten file. Both writers of a
    /// shorter session -- compaction and `/undo` -- are destructive by design,
    /// and this archive is what makes either recoverable by hand.
    fn archive_original(&self, id: &SessionId, path: &Path) -> Result<()> {
        let archive_dir = self.root_dir.join("archive");
        ensure_private_dir(&archive_dir)?;
        let mut archive = tempfile::Builder::new()
            .prefix(&format!("{}-", id.as_str()))
            .suffix(".jsonl")
            .tempfile_in(&archive_dir)?;
        std::io::copy(&mut std::fs::File::open(path)?, &mut archive)?;
        archive.as_file().sync_all()?;
        let _ = archive.keep().map_err(|e| SessionError::Io(e.error))?;
        Ok(())
    }

    /// Rewind the session to its first `keep` *active* entries and return the
    /// entries that were dropped (empty when there is nothing to drop).
    ///
    /// The caller rewinds the *conversation*, never the disk: files the model
    /// wrote are git's business, not this method's. The complete original
    /// lands in `archive/`, so a rewind is recoverable by hand.
    ///
    /// Records are copied verbatim rather than re-serialized: truncating a tail
    /// never invalidates the ids the surviving records already carry (the chain
    /// only ever points backwards), and verbatim keeps fields this build does
    /// not know about. Everything up to the last compaction marker stays, since
    /// dropping that marker would resurrect the history its summary replaced.
    pub async fn rewind(&self, id: &SessionId, keep: usize) -> Result<Vec<SessionEntry>> {
        use std::io::Write;
        // `load` replays from the last compaction marker, so `active` is
        // exactly what a resume would see: the count the caller means.
        let (_meta, active) = self.load(id).await?;
        if keep >= active.len() {
            return Ok(Vec::new());
        }
        let dropped = active[keep..].to_vec();
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;
        let content = std::fs::read_to_string(&path)?;
        let mut lines = content.lines().filter(|line| !line.trim().is_empty());
        let header_line = lines
            .next()
            .ok_or_else(|| SessionError::Io(std::io::Error::other("session file has no header")))?;
        let records: Vec<&str> = lines.collect();
        let superseded = records
            .iter()
            .rposition(|line| {
                serde_json::from_str::<SessionEntry>(line)
                    .map(|entry| entry.compaction_boundary)
                    .unwrap_or(false)
            })
            .map(|index| index + 1)
            .unwrap_or(0);
        let keep_records = superseded + keep;
        let mut replacement = tempfile::NamedTempFile::new_in(&self.root_dir)?;
        writeln!(replacement, "{header_line}")?;
        for record in &records[..keep_records] {
            writeln!(replacement, "{record}")?;
        }
        replacement.as_file().sync_all()?;
        self.archive_original(id, &path)?;
        replacement
            .persist(&path)
            .map_err(|e| SessionError::Io(e.error))?;
        Ok(dropped)
    }

    pub async fn load(&self, id: &SessionId) -> Result<(SessionMeta, Vec<SessionEntry>)> {
        let _lock_file = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;
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
        let (header_line_num, header_str) =
            all_lines.first().map(|&(n, s)| (n, s)).unwrap_or((1, ""));
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

        let mut damaged_tail = false;
        let entry_lines = &all_lines[1..];
        let mut entries = Vec::with_capacity(entry_lines.len());
        let mut entry_line_nums = Vec::with_capacity(entry_lines.len());

        for (idx, (line_num, line_str)) in entry_lines.iter().enumerate() {
            let is_final_line = idx == entry_lines.len() - 1;
            match serde_json::from_str::<SessionEntry>(line_str) {
                Ok(entry) => {
                    entries.push(entry);
                    entry_line_nums.push(*line_num);
                }
                Err(e) => {
                    if is_final_line {
                        damaged_tail = true;
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

        // Structural validation on the full on-disk graph (before the drain):
        // version, filename-vs-header id, duplicates, parent chain. Old
        // version-1 linear files pass unchanged.
        Self::validate_loaded_graph(
            &path,
            id,
            header_line_num,
            &header,
            &entries,
            &entry_line_nums,
        )?;

        // A resumed session must remain appendable. Preserve the original
        // before atomically replacing only a damaged final record/newline.
        if damaged_tail || !content.ends_with('\n') {
            use std::io::Write;
            let mut backup = tempfile::Builder::new()
                .prefix("session-tail-backup-")
                .tempfile_in(&self.root_dir)?;
            backup.write_all(content.as_bytes())?;
            let (_, backup_path) = backup.keep().map_err(|e| SessionError::Io(e.error))?;
            let mut repaired = String::new();
            let keep = if damaged_tail {
                all_lines.len() - 1
            } else {
                all_lines.len()
            };
            for (_, line) in all_lines.iter().take(keep) {
                repaired.push_str(line);
                repaired.push('\n');
            }
            let mut replacement = tempfile::NamedTempFile::new_in(&self.root_dir)?;
            replacement.write_all(repaired.as_bytes())?;
            replacement.as_file().sync_all()?;
            replacement
                .persist(&path)
                .map_err(|e| SessionError::Io(e.error))?;
            log::warn!(
                "repaired session tail; original saved at {}",
                backup_path.display()
            );
        }

        let meta = SessionMeta {
            id: header.id,
            timestamp: header.timestamp,
            cwd: header.cwd.clone(),
            model: header.model,
        };
        self.remember(id, &meta.cwd).await;

        // Compaction boundary: replay only the active transcript after the
        // last marker. Old files carry no markers and load whole.
        let mut active = entries;
        if let Some(last) = active.iter().rposition(|e| e.compaction_boundary) {
            active.drain(..=last);
        }

        Ok((meta, active))
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

        // Pass one: the directory itself. Cheap, and it collects the paths
        // so the reads below can overlap instead of serializing behind one
        // another on a slow disk.
        let mut paths = Vec::new();
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            // Real file, not a symlink; stem must be a valid session id.
            if !entry
                .file_type()
                .await
                .map(|t| t.is_file())
                .unwrap_or(false)
            {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !valid_session_id(stem) {
                log::warn!("skipping session with invalid id: {}", path.display());
                continue;
            }
            paths.push(path);
        }

        // Pass two: read both ends of each file on the blocking pool, a
        // bounded number at a time. One task per file (not per read) keeps
        // the dispatch count down and lets the disk work in parallel.
        use futures::StreamExt as _;
        let mut reads = futures::stream::iter(paths)
            .map(|path| {
                let path = path.clone();
                async move {
                    let read = tokio::task::spawn_blocking({
                        let path = path.clone();
                        move || (read_session_ends(&path), path)
                    })
                    .await;
                    read.ok()
                        .and_then(|(slices, path)| slices.map(|s| (path, s)))
                }
            })
            .buffer_unordered(64);
        let mut ends = Vec::new();
        while let Some(pair) = reads.next().await {
            if let Some(pair) = pair {
                ends.push(pair);
            }
        }
        // Directory order is arbitrary; sort for a stable summary order
        // before the activity sort below decides the list.
        ends.sort_by(|a, b| a.0.cmp(&b.0));

        let mut summaries = Vec::new();
        for (path, slices) in ends {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };

            // A first line with no newline is a creator mid-write.
            let complete_first_line = slices.head.contains('\n');

            let header_str = match slices.head.lines().find(|l| !l.trim().is_empty()) {
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
                    // Quarantine only a *complete* first line that will
                    // not parse. A file whose first line has no terminating
                    // newline is a creator mid-write (create streams the
                    // header into a `create_new` file), so renaming it away
                    // would steal a session that is about to be valid.
                    if complete_first_line {
                        Self::quarantine_corrupt_file(&path).await;
                    }
                    continue;
                }
            };
            if header.id.as_str() != stem {
                log::warn!(
                    "skipping session whose header id does not match its filename: {}",
                    path.display()
                );
                continue;
            }

            // A header-only session still has a usable "last active" time.
            let mut last_message_at = header.timestamp;
            let mut first_user_text = None;
            let mut last_user_text = None;
            // Head first, then tail. `in_tail` means "this line came from
            // the tail slice", not "a boundary was seen": a post-boundary
            // turn inside the head is still the session's first turn, while
            // a tail turn never is (the head owns the real beginning).
            for (in_tail, chunk) in [(false, &slices.head), (true, &slices.tail)] {
                for line in chunk.lines().filter(|l| !l.trim().is_empty()) {
                    let Ok(entry) = serde_json::from_str::<SessionEntry>(line) else {
                        continue;
                    };
                    last_message_at = last_message_at.max(entry.timestamp);
                    if entry.compaction_boundary {
                        // Replacement follows: the pre-compact messages no
                        // longer describe the session (both ends move).
                        first_user_text = None;
                        last_user_text = None;
                        continue;
                    }
                    if entry.message.role == Role::User {
                        let text = entry.message.text_content();
                        if !text.is_empty() {
                            if first_user_text.is_none() && !in_tail {
                                first_user_text = Some(text.clone());
                            }
                            last_user_text = Some(text);
                        }
                    }
                }
            }

            summaries.push(SessionSummary {
                id: header.id,
                started_at: header.timestamp,
                last_message_at,
                cwd: header.cwd,
                first_user_text,
                last_user_text,
            });
        }

        // Newest activity first — the same key the row prints, so the list
        // reads monotonically instead of mixing "when it started" with
        // "when it was last used".
        summaries.sort_by_key(|s| s.last_message_at);
        summaries.reverse();
        summaries
    }

    pub async fn delete(&self, id: &SessionId) -> Result<()> {
        // Cross-process lock first, in-memory second: without the file lock
        // a delete can race another process's append and leave that writer
        // appending to an unlinked inode (its writes vanish silently).
        let _lock = self.lock_session_file(id).await?;
        let _guard = self.lock.lock().await;
        let path = self.session_path(id)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SessionError::Io(e)),
        }
    }

    /// Deletes every session whose header timestamp predates `cutoff_ms`.
    /// Returns the ids removed. Corrupt files are quarantined by `list`, not
    /// deleted (`gray sessions prune --older-than-days N`, audit F8).
    pub async fn prune_before(&self, cutoff_ms: u64) -> Result<Vec<SessionId>> {
        let mut removed = Vec::new();
        for summary in self.list().await {
            if summary.started_at < cutoff_ms {
                self.delete(&summary.id).await?;
                removed.push(summary.id);
            }
        }
        Ok(removed)
    }
}

/// Returns the default session directory (`~/.gray/sessions`), or `None` if `$HOME` is not set.
pub fn default_root() -> Option<PathBuf> {
    gray_core::paths::gray_home().map(|home| home.join("sessions"))
}

/// Helper function to return current time in milliseconds since Unix epoch.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[path = "session_store_tests.rs"]
#[cfg(test)]
mod tests;
