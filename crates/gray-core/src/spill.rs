//! Content-addressed store for tool output that cannot fit in context.
//!
//! Truncation amputates the middle of a result and leaves a note, which is a
//! way of lying to the model: it cannot tell that what it is reading is the
//! beginning and the end of something else. This store is the other half of
//! that trade — the context keeps a cheap preview, the full original lands
//! here under a content hash, and the handle in the footer is the way back.
//!
//! Handles are 16 hex characters of the SHA-256 of the content, so the same
//! output spilled twice costs one file and the same handle, and a handle can
//! never name a path outside the store (every digit is checked).
//!
//! This is the same trust domain as the session transcripts next to it and
//! the shell logs the bash tool already writes: raw tool output, 0600, under
//! `$GRAY_HOME`. Redaction happens on the way into the model's context, not on
//! the way to disk — a spilled original that had been scrubbed would hand back
//! text that never existed.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Hex characters in a handle (`sha2` prints 64; the handle is the first half).
pub const HANDLE_LEN: usize = 16;

/// Files kept before the oldest go. The store exists to serve the current
/// session; a spilled original older than a few hundred results is not a
/// recovery path, it is litter.
pub const MAX_ENTRIES: usize = 256;

/// Meter lines before recording stops. 1 MiB of JSONL is ~20k results; past
/// that the file is not worth a truncation dance and the counters simply stop
/// moving.
const MAX_METER_BYTES: u64 = 1024 * 1024;

/// Why a handle could not be served.
#[derive(Debug)]
pub enum SpillError {
    /// `$GRAY_HOME` is not resolvable, so there is no store to talk to.
    NoHome,
    /// The handle is not 16 hex characters, so it never named a stored file.
    BadHandle(String),
    /// The handle is well-formed but nothing is stored under it: evicted, or
    /// from another machine's home. Never an empty success.
    NotFound(String),
    /// Read/write failure, with the path that failed.
    Io(String),
}

impl std::fmt::Display for SpillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpillError::NoHome => write!(f, "no $GRAY_HOME to store tool output under"),
            SpillError::BadHandle(h) => write!(
                f,
                "not a spill handle: {h:?} (handles are {HANDLE_LEN} hex characters, e.g. from a `[spilled … gray spill …]` footer)"
            ),
            SpillError::NotFound(h) => write!(
                f,
                "spill {h} is gone — it was evicted (the store keeps the newest {MAX_ENTRIES} results) or belongs to another $GRAY_HOME. Re-run the command to get it back."
            ),
            SpillError::Io(e) => write!(f, "spill store I/O failed: {e}"),
        }
    }
}

impl std::error::Error for SpillError {}

/// The store directory (`$GRAY_HOME/spill`), or `None` without a home.
pub fn dir() -> Option<PathBuf> {
    crate::paths::gray_home().map(|home| home.join("spill"))
}

/// The one-line footer a spilled result carries: what was left out, where the
/// whole thing is, and the exact commands that read it back.
pub fn footer(handle: &str, raw_bytes: usize, raw_lines: usize) -> String {
    format!(
        "\n[spilled {raw_lines} lines / {} B — this is a preview. \
         Read it back with: gray spill head {handle} · gray spill tail {handle} · \
         gray spill grep {handle} <pattern> (-n CONTEXT)]",
        fmt_bytes(raw_bytes)
    )
}

/// Store `text` and return its handle, or `None` when it could not be stored
/// (no home, unwritable directory, or `GRAY_NO_SPILL=1`). A store failure
/// degrades to today's behaviour — truncation with no handle — and never fails
/// the tool call.
pub fn store(text: &str) -> Option<String> {
    if std::env::var_os("GRAY_NO_SPILL").is_some() {
        return None;
    }
    let dir = dir()?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!(target: "gray_spill", "cannot create {}: {e}", dir.display());
        return None;
    }
    restrict_dir(&dir);
    let handle = handle_for(text);
    let path = dir.join(format!("{handle}.txt"));
    write_private(&path, text.as_bytes())?;
    prune(&dir);
    Some(handle)
}

/// The handle for content — the same text always yields the same handle, so a
/// repeated command does not fill the store.
pub fn handle_for(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    hex(&digest)[..HANDLE_LEN].to_string()
}

/// Path of a stored original, after checking the handle really is one.
pub fn path(handle: &str) -> Result<PathBuf, SpillError> {
    if !is_handle(handle) {
        return Err(SpillError::BadHandle(handle.to_string()));
    }
    let dir = dir().ok_or(SpillError::NoHome)?;
    let path = dir.join(format!("{handle}.txt"));
    if path.is_file() {
        Ok(path)
    } else {
        Err(SpillError::NotFound(handle.to_string()))
    }
}

/// The full original behind a handle.
pub fn read(handle: &str) -> Result<String, SpillError> {
    std::fs::read_to_string(path(handle)?).map_err(|e| SpillError::Io(format!("{e}")))
}

/// True when `handle` could have come out of this store. Every character is
/// checked, so `../../etc/passwd` is a [`SpillError::BadHandle`] and never a
/// path.
fn is_handle(handle: &str) -> bool {
    handle.len() == HANDLE_LEN
        && handle
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Drop the oldest entries until the store is back under [`MAX_ENTRIES`].
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut stored: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            // Only content files: the meter lives here too.
            if path.extension().is_none_or(|x| x != "txt") {
                return None;
            }
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((mtime, path))
        })
        .collect();
    if stored.len() <= MAX_ENTRIES {
        return;
    }
    stored.sort_by_key(|(mtime, _)| *mtime);
    let excess = stored.len() - MAX_ENTRIES;
    for (_, path) in stored.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

/// One metered result: how many bytes the tool produced, how many reached the
/// model, and which rule (if any) closed the gap.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MeterEvent {
    /// Unix millis.
    pub ts: u64,
    /// Rule id from [`crate::squeeze`], or the empty string for a plain spill.
    /// No separate kind: a result with no rule is a spill, by construction.
    pub rule: String,
    /// Bytes the tool produced.
    pub raw: u64,
    /// Bytes that reached the model.
    pub sent: u64,
}

/// Aggregate view over the meter file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeterTotals {
    pub events: u64,
    pub raw_bytes: u64,
    pub sent_bytes: u64,
    /// rule id → (events, raw bytes, sent bytes)
    pub by_rule: std::collections::BTreeMap<String, (u64, u64, u64)>,
}

impl MeterTotals {
    /// Bytes never sent, and the share of the total that is.
    pub fn saved(&self) -> u64 {
        self.raw_bytes.saturating_sub(self.sent_bytes)
    }
    pub fn saved_pct(&self) -> f64 {
        if self.raw_bytes == 0 {
            0.0
        } else {
            self.saved() as f64 / self.raw_bytes as f64 * 100.0
        }
    }
    fn add(&mut self, e: &MeterEvent) {
        self.events += 1;
        self.raw_bytes += e.raw;
        self.sent_bytes += e.sent;
        let slot = self.by_rule.entry(e.rule.clone()).or_default();
        slot.0 += 1;
        slot.1 += e.raw;
        slot.2 += e.sent;
    }
}

/// Append one line to the meter. Best effort: a session must not fail because
/// the meter could not be written.
pub fn record(event: MeterEvent) {
    let Some(dir) = dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    restrict_dir(&dir);
    let path = dir.join("meter.jsonl");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= MAX_METER_BYTES) {
        return;
    }
    if let Ok(line) = serde_json::to_string(&event) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Read every metered event back, in file order. Unparseable lines are
/// skipped: one torn write must not make the whole meter unreadable.
pub fn events() -> Vec<MeterEvent> {
    let Some(dir) = dir() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(dir.join("meter.jsonl")) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Totals over [`events`].
pub fn totals() -> MeterTotals {
    let mut totals = MeterTotals::default();
    for event in events() {
        totals.add(&event);
    }
    totals
}

/// Unix millis, for the meter's own timestamp.
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Unix seconds, for cron timestamps and staleness windows.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn write_private(path: &Path, bytes: &[u8]) -> Option<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = match opts.open(path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!(target: "gray_spill", "cannot write {}: {e}", path.display());
            return None;
        }
    };
    if f.write_all(bytes).is_ok() {
        Some(())
    } else {
        log::warn!(target: "gray_spill", "short write to {}", path.display());
        None
    }
}

/// The store holds raw tool output: owner-only, like the transcripts beside it.
fn restrict_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // GC-8: best-effort, but a silent failure leaves raw output readable by
        // other users on a shared box, so say so.
        if let Err(err) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
            log::warn!(target: "gray_spill", "spill dir not restricted to owner: {err}");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
    }
    out
}

/// Human-readable byte count for the footer: bytes below 1 KiB, then one
/// decimal. A footer that says `51200 B` costs the model a conversion.
pub fn fmt_bytes(bytes: usize) -> String {
    const KIB: f64 = 1024.0;
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kib = bytes as f64 / KIB;
    if kib < 1024.0 {
        return format!("{kib:.1} KiB");
    }
    format!("{:.1} MiB", kib / KIB)
}
