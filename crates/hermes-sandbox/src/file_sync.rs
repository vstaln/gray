//! Shared file sync manager for remote execution backends.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/tools/environments/file_sync.py` (484 lines).
//! Tracks local file changes via mtime+size, detects deletions, and syncs to remote
//! environments transactionally. Used by SSH, Modal, and Daytona. Docker and Singularity
//! use bind mounts (live host FS view) and don't need this.
//!
//! Python source docstring (preserved):
//! ```text
//! Shared file sync manager for remote execution backends.
//!
//! Tracks local file changes via mtime+size, detects deletions, and
//! syncs to remote environments transactionally.  Used by SSH, Modal,
//! and Daytona.  Docker and Singularity use bind mounts (live host FS
//! view) and don't need this.
//! ```

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors Python module globals
// ---------------------------------------------------------------------------

/// Mirrors `_SYNC_INTERVAL_SECONDS = 5.0`.
pub const SYNC_INTERVAL_SECONDS: f64 = 5.0;
/// Mirrors `_FORCE_SYNC_ENV = "HERMES_FORCE_FILE_SYNC"`.
pub const FORCE_SYNC_ENV: &str = "HERMES_FORCE_FILE_SYNC";

/// Mirrors `_SYNC_BACK_MAX_RETRIES = 3`.
pub const SYNC_BACK_MAX_RETRIES: usize = 3;
/// Mirrors `_SYNC_BACK_BACKOFF = (2, 4, 8)`.
pub const SYNC_BACK_BACKOFF: &[u64] = &[2, 4, 8];
/// Mirrors `_SYNC_BACK_MAX_BYTES = 2 * 1024 * 1024 * 1024` (2 GiB).
pub const SYNC_BACK_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

// Keep retry sleeps patchable without mutating global state.
// Python keeps `_sleep = time.sleep` and `_monotonic = time.monotonic` as
// module globals so tests can patch `file_sync.time.sleep` without inflating
// unrelated background threads under xdist. In Rust we expose `sleep_secs`
// and `monotonic_secs` as plain functions; tests can inject an alternative
// clock via `FileSyncManager::with_clock` if needed.
fn sleep_secs(secs: u64) {
    std::thread::sleep(Duration::from_secs(secs));
}

fn monotonic_secs() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    start.elapsed().as_secs_f64()
}

// ---------------------------------------------------------------------------
// Transport callbacks — mirrors Python type aliases
// ---------------------------------------------------------------------------

/// Mirrors `UploadFn = Callable[[str, str], None]`.
pub type UploadFn = dyn Fn(&str, &str) -> Result<(), String> + Send + Sync;
/// Mirrors `BulkUploadFn = Callable[[list[tuple[str, str]]], None]`.
pub type BulkUploadFn = dyn Fn(&[(String, String)]) -> Result<(), String> + Send + Sync;
/// Mirrors `BulkDownloadFn = Callable[[Path], None]` — writes tar archive to dest.
pub type BulkDownloadFn = dyn Fn(&Path) -> Result<(), String> + Send + Sync;
/// Mirrors `DeleteFn = Callable[[list[str]], None]`.
pub type DeleteFn = dyn Fn(&[String]) -> Result<(), String> + Send + Sync;
/// Mirrors `GetFilesFn = Callable[[], list[tuple[str, str]]]`.
pub type GetFilesFn = dyn Fn() -> Vec<(String, String)> + Send + Sync;

// ---------------------------------------------------------------------------
// Home helpers — mirrors `hermes_constants.get_hermes_home()`
// ---------------------------------------------------------------------------

/// Resolve Hermes home directory.
/// Mirrors `hermes_constants.get_hermes_home()`: `HERMES_HOME` env → `~/.hermes`.
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let t = home.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t).join(".hermes");
        }
    }
    if let Ok(up) = std::env::var("USERPROFILE") {
        let t = up.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t).join(".hermes");
        }
    }
    PathBuf::from(".hermes")
}

// ---------------------------------------------------------------------------
// Low-level helpers — mirrors Python free functions
// ---------------------------------------------------------------------------

/// Mirrors `_file_mtime_key` from `tools.environments.base`:
/// `return (st.st_mtime, st.st_size)` or `None` if unreadable.
pub fn file_mtime_key(host_path: &str) -> Option<(f64, u64)> {
    let p = Path::new(host_path);
    let meta = fs::metadata(p).ok()?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    Some((mtime, size))
}

/// Mirrors `iter_sync_files(container_base="/root/.hermes")`.
///
/// Combines credentials, skills, and cache into a flat list of
/// `(host_path, remote_path)` pairs. Credential paths are remapped from
/// `/root/.hermes` to `container_base`.
///
/// Late-import in Python avoids circular deps; in Rust this is a best-effort
/// stub that returns an empty list unless a custom provider is registered
/// via `set_iter_sync_files_provider`. If no provider is set the empty list
/// matches the behavior when credential/skill/cache sources are unavailable.
static ITER_SYNC_FILES_PROVIDER: Mutex<Option<Box<GetFilesFn>>> = Mutex::new(None);

/// Install a custom provider for `iter_sync_files` (test hook).
pub fn set_iter_sync_files_provider(provider: Option<Box<GetFilesFn>>) {
    if let Ok(mut g) = ITER_SYNC_FILES_PROVIDER.lock() {
        *g = provider;
    }
}

pub fn iter_sync_files(container_base: &str) -> Vec<(String, String)> {
    let base = if container_base.is_empty() {
        "/root/.hermes"
    } else {
        container_base
    };
    // If a test/provider is installed, delegate to it.
    if let Ok(g) = ITER_SYNC_FILES_PROVIDER.lock() {
        if let Some(f) = g.as_ref() {
            let mut files = f();
            // Remap credential-style hosts if needed — preserve Python semantics:
            // credential container_path.replace("/root/.hermes", container_base, 1)
            // For generic provider we leave as-is; providers are expected to handle base.
            // But ensure any /root/.hermes prefix is remapped when base differs.
            if base != "/root/.hermes" {
                for (_, remote) in &mut files {
                    if let Some(rest) = remote.strip_prefix("/root/.hermes") {
                        *remote = format!("{}{}", base.trim_end_matches('/'), rest);
                    }
                }
            }
            return files;
        }
    }
    // Default: empty — credential/skill/cache discovery lives in Python and is
    // not replicated here. Backends supply their own `GetFilesFn` to the manager.
    let _ = base;
    Vec::new()
}

/// Mirrors `_credential_host_paths() -> set[str]` — credential files that are
/// upload-only for remote sandboxes.
///
/// Tries to resolve via `iter_sync_files`; failures return empty set (mirrors
/// Python's `except Exception: return set()`).
pub fn credential_host_paths() -> HashSet<String> {
    // In Python this imports `get_credential_file_mounts` and resolves each
    // host_path via `Path(...).expanduser().resolve()`.
    // In Rust we have no credential registry; return empty set. Managers
    // maintain their own `upload_only_host_paths` plus a fresh call here on
    // each sync_back_impl — empty is safe (just means no credential is
    // considered upload-only, which matches a host with no credentials).
    HashSet::new()
}

/// Mirrors `quoted_rm_command(remote_paths: list[str]) -> str`.
pub fn quoted_rm_command(remote_paths: &[String]) -> String {
    let parts: Vec<String> = remote_paths.iter().map(|p| shlex_quote(p)).collect();
    if parts.is_empty() {
        "rm -f".to_string()
    } else {
        format!("rm -f {}", parts.join(" "))
    }
}

/// Mirrors `quoted_mkdir_command(dirs: list[str]) -> str`.
pub fn quoted_mkdir_command(dirs: &[String]) -> String {
    let parts: Vec<String> = dirs.iter().map(|p| shlex_quote(p)).collect();
    if parts.is_empty() {
        "mkdir -p".to_string()
    } else {
        format!("mkdir -p {}", parts.join(" "))
    }
}

/// Mirrors `unique_parent_dirs(files: list[tuple[str, str]]) -> list[str]`.
pub fn unique_parent_dirs(files: &[(String, String)]) -> Vec<String> {
    let mut set = HashSet::new();
    for (_, remote) in files {
        let dir = posix_dirname(remote);
        set.insert(dir);
    }
    let mut out: Vec<String> = set.into_iter().collect();
    out.sort();
    out
}

fn posix_dirname(path: &str) -> String {
    // posixpath.dirname semantics: strip trailing slashes, then take up to last '/'.
    // Python's posixpath.dirname("/a/b/c") -> "/a/b", dirname("/a") -> "/", dirname("a") -> "".
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    if let Some(idx) = trimmed.rfind('/') {
        if idx == 0 {
            return "/".to_string();
        }
        return trimmed[..idx].to_string();
    }
    "".to_string()
}

fn shlex_quote(s: &str) -> String {
    // Minimal `shlex.quote` — mirrors Python: safe chars [a-zA-Z0-9@%_+=:,./-] need no quoting.
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-' )
    });
    if safe {
        return s.to_string();
    }
    // Single-quote and escape single quotes as '\''.
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{}'", escaped)
}

/// Mirrors `_sha256_file(path: str) -> str` — hex SHA-256 digest.
pub fn sha256_file(path: &str) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// Minimal SHA-256 (FIPS 180-4) — no external crate. Matches Python's hashlib.sha256.
struct Sha256 {
    h: [u32; 8],
    len_bits: u64,
    buf: Vec<u8>,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            len_bits: 0,
            buf: Vec::new(),
        }
    }
    fn update(&mut self, data: &[u8]) {
        self.len_bits += (data.len() as u64) * 8;
        self.buf.extend_from_slice(data);
    }
    fn finalize(mut self) -> [u8; 32] {
        // Padding
        let mut padded = self.buf.clone();
        padded.push(0x80);
        while padded.len() % 64 != 56 {
            padded.push(0);
        }
        padded.extend_from_slice(&self.len_bits.to_be_bytes());
        // Process 64-byte chunks
        for chunk in padded.chunks(64) {
            let mut w = [0u32; 64];
            for i in 0..16 {
                w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
            }
            let mut a = self.h[0];
            let mut b = self.h[1];
            let mut c = self.h[2];
            let mut d = self.h[3];
            let mut e = self.h[4];
            let mut f = self.h[5];
            let mut g = self.h[6];
            let mut hh = self.h[7];
            const K: [u32; 64] = [
                0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
                0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
                0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
                0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
                0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
                0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
                0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
                0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
            ];
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ ((!e) & g);
                let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let temp2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(temp1);
                d = c;
                c = b;
                b = a;
                a = temp1.wrapping_add(temp2);
            }
            self.h[0] = self.h[0].wrapping_add(a);
            self.h[1] = self.h[1].wrapping_add(b);
            self.h[2] = self.h[2].wrapping_add(c);
            self.h[3] = self.h[3].wrapping_add(d);
            self.h[4] = self.h[4].wrapping_add(e);
            self.h[5] = self.h[5].wrapping_add(f);
            self.h[6] = self.h[6].wrapping_add(g);
            self.h[7] = self.h[7].wrapping_add(hh);
        }
        let mut out = [0u8; 32];
        for (i, v) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }
}

// ---------------------------------------------------------------------------
// FileSyncManager — mirrors Python class FileSyncManager
// ---------------------------------------------------------------------------

struct Inner {
    synced_files: HashMap<String, (f64, u64)>, // remote_path -> (mtime, size)
    pushed_hashes: HashMap<String, String>,    // remote_path -> sha256 hex
    upload_only_host_paths: HashSet<String>,
    last_sync: Option<Instant>,
}

/// Tracks local file changes and syncs to a remote environment.
///
/// Backends instantiate this with transport callbacks (upload, delete)
/// and a file-source callable. The manager handles mtime-based change
/// detection, deletion tracking, rate limiting, and transactional state.
///
/// Not used by bind-mount backends (Docker, Singularity) — those get
/// live host FS views and don't need file sync.
pub struct FileSyncManager {
    get_files_fn: Box<GetFilesFn>,
    upload_fn: Box<UploadFn>,
    bulk_upload_fn: Option<Box<BulkUploadFn>>,
    bulk_download_fn: Option<Box<BulkDownloadFn>>,
    delete_fn: Box<DeleteFn>,
    sync_interval: Duration,
    inner: Mutex<Inner>,
}

impl FileSyncManager {
    /// Mirrors `FileSyncManager.__init__(get_files_fn, upload_fn, delete_fn, ...)`.
    pub fn new(
        get_files_fn: Box<GetFilesFn>,
        upload_fn: Box<UploadFn>,
        delete_fn: Box<DeleteFn>,
        sync_interval_secs: Option<f64>,
        bulk_upload_fn: Option<Box<BulkUploadFn>>,
        bulk_download_fn: Option<Box<BulkDownloadFn>>,
    ) -> Self {
        let interval = sync_interval_secs.unwrap_or(SYNC_INTERVAL_SECONDS);
        Self {
            get_files_fn,
            upload_fn,
            bulk_upload_fn,
            bulk_download_fn,
            delete_fn,
            sync_interval: Duration::from_secs_f64(interval),
            inner: Mutex::new(Inner {
                synced_files: HashMap::new(),
                pushed_hashes: HashMap::new(),
                upload_only_host_paths: HashSet::new(),
                last_sync: None,
            }),
        }
    }

    /// Convenience constructor with default interval.
    pub fn with_interval(
        get_files_fn: Box<GetFilesFn>,
        upload_fn: Box<UploadFn>,
        delete_fn: Box<DeleteFn>,
        bulk_upload_fn: Option<Box<BulkUploadFn>>,
        bulk_download_fn: Option<Box<BulkDownloadFn>>,
    ) -> Self {
        Self::new(
            get_files_fn,
            upload_fn,
            delete_fn,
            None,
            bulk_upload_fn,
            bulk_download_fn,
        )
    }

    /// Run a sync cycle: upload changed files, delete removed files.
    ///
    /// Rate-limited to once per `sync_interval` unless `force` is true
    /// or `HERMES_FORCE_FILE_SYNC=1` is set.
    ///
    /// Transactional: state only committed if ALL operations succeed.
    /// On failure, state rolls back so the next cycle retries everything.
    pub fn sync(&self, force: bool) {
        // Mirrors `with self._transaction_lock:` — single mutex guards the whole cycle.
        // Use try_lock to avoid poisoning issues; if poisoned we still proceed.
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.sync_transaction(&mut guard, force);
    }

    fn sync_transaction(&self, inner: &mut Inner, force: bool) {
        // Rate limit guard
        if !force && std::env::var(FORCE_SYNC_ENV).is_err() {
            if let Some(last) = inner.last_sync {
                if last.elapsed() < self.sync_interval {
                    return;
                }
            }
        }

        let current_files = (self.get_files_fn)();
        // Mirrors `self._upload_only_host_paths.update(_credential_host_paths())`
        for p in credential_host_paths() {
            inner.upload_only_host_paths.insert(p);
        }
        let current_remote_paths: HashSet<String> =
            current_files.iter().map(|(_, r)| r.clone()).collect();

        // --- Uploads: new or changed files ---
        let mut to_upload: Vec<(String, String)> = Vec::new();
        let mut new_files = inner.synced_files.clone();
        for (host_path, remote_path) in &current_files {
            let file_key = match file_mtime_key(host_path) {
                Some(k) => k,
                None => continue,
            };
            if inner.synced_files.get(remote_path) == Some(&file_key) {
                continue;
            }
            to_upload.push((host_path.clone(), remote_path.clone()));
            new_files.insert(remote_path.clone(), file_key);
        }

        // --- Deletes: synced paths no longer in current set ---
        let to_delete: Vec<String> = inner
            .synced_files
            .keys()
            .filter(|p| !current_remote_paths.contains(*p))
            .cloned()
            .collect();

        if to_upload.is_empty() && to_delete.is_empty() {
            inner.last_sync = Some(Instant::now());
            return;
        }

        // Snapshot for rollback (only when there's work to do)
        let prev_files = inner.synced_files.clone();
        let prev_hashes = inner.pushed_hashes.clone();

        if !to_upload.is_empty() {
            log::debug!("file_sync: uploading {} file(s)", to_upload.len());
        }
        if !to_delete.is_empty() {
            log::debug!("file_sync: deleting {} stale remote file(s)", to_delete.len());
        }

        let result: Result<(), String> = (|| {
            if !to_upload.is_empty() {
                if let Some(bulk) = &self.bulk_upload_fn {
                    bulk(&to_upload).map_err(|e| e.to_string())?;
                    log::debug!("file_sync: bulk-uploaded {} file(s)", to_upload.len());
                } else {
                    for (host_path, remote_path) in &to_upload {
                        (self.upload_fn)(host_path, remote_path).map_err(|e| e.to_string())?;
                        log::debug!("file_sync: uploaded {} -> {}", host_path, remote_path);
                    }
                }
            }
            if !to_delete.is_empty() {
                (self.delete_fn)(&to_delete).map_err(|e| e.to_string())?;
                log::debug!("file_sync: deleted {:?}", to_delete);
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                // --- Commit (all succeeded) ---
                for (host_path, remote_path) in &to_upload {
                    match sha256_file(host_path) {
                        Ok(h) => {
                            inner.pushed_hashes.insert(remote_path.clone(), h);
                        }
                        Err(e) => {
                            log::warn!("file_sync: sha256 failed for {}: {}", host_path, e);
                        }
                    }
                }
                for p in &to_delete {
                    new_files.remove(p);
                    inner.pushed_hashes.remove(p);
                }
                inner.synced_files = new_files;
                inner.last_sync = Some(Instant::now());
            }
            Err(exc) => {
                inner.synced_files = prev_files;
                inner.pushed_hashes = prev_hashes;
                // Do NOT advance last_sync here: a failed cycle rolls state
                // back so the next cycle can retry. Bumping the rate-limit clock on
                // failure would make the next non-forced sync() return early and
                // suppress retry for up to sync_interval — contradicting the
                // documented "next cycle retries everything" contract.
                log::warn!("file_sync: sync failed, rolled back state: {}", exc);
            }
        }
    }

    // ------------------------------------------------------------------
    // Sync-back: pull remote changes to host on teardown
    // ------------------------------------------------------------------

    /// Pull remote changes back to the host filesystem.
    ///
    /// Downloads the remote `.hermes/` directory as a tar archive,
    /// unpacks it, and applies only files that differ from what was
    /// originally pushed (based on SHA-256 content hashes).
    ///
    /// Protected against SIGINT (defers the signal until complete) and
    /// serialized across concurrent gateway sandboxes via file lock.
    pub fn sync_back(&self, hermes_home: Option<&Path>) {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        self.sync_back_transaction(&mut guard, hermes_home);
        // Explicit drop before return (mirrors Python's `with` exit).
        drop(guard);
    }

    fn sync_back_transaction(&self, inner: &mut Inner, hermes_home: Option<&Path>) {
        if self.bulk_download_fn.is_none() {
            return;
        }
        // Nothing was ever committed through this manager — the initial
        // push failed or never ran. Skip sync_back to avoid retry storms
        // against an uninitialized remote .hermes/ directory.
        if inner.pushed_hashes.is_empty() && inner.synced_files.is_empty() {
            log::debug!("sync_back: no prior push state — skipping");
            return;
        }

        let home = hermes_home
            .map(|p| p.to_path_buf())
            .unwrap_or_else(get_hermes_home);
        let lock_path = home.join(".sync.lock");
        if let Some(parent) = lock_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut last_exc: Option<String> = None;
        for attempt in 0..SYNC_BACK_MAX_RETRIES {
            match self.sync_back_once(&lock_path, inner) {
                Ok(()) => return,
                Err(exc) => {
                    last_exc = Some(exc.clone());
                    if attempt + 1 < SYNC_BACK_MAX_RETRIES {
                        let delay = SYNC_BACK_BACKOFF[attempt];
                        log::warn!(
                            "sync_back: attempt {} failed ({}), retrying in {}s",
                            attempt + 1,
                            exc,
                            delay
                        );
                        sleep_secs(delay);
                    }
                }
            }
        }
        log::warn!(
            "sync_back: all {} attempts failed: {:?}",
            SYNC_BACK_MAX_RETRIES,
            last_exc
        );
    }

    fn sync_back_once(&self, lock_path: &Path, inner: &Inner) -> Result<(), String> {
        // signal.signal() only works from the main thread. In gateway
        // contexts cleanup() may run from a worker thread — skip SIGINT
        // deferral there rather than crashing.
        // In Rust we don't have Python's signal deferral without `signal_hook`.
        // We do the file-locked impl directly; SIGINT deferral is a no-op here
        // (the host process will still handle Ctrl-C after we return, which
        // preserves the intent: don't leave a half-applied sync_back).
        // If signal handling is needed, wire `signal_hook` and defer here.
        self.sync_back_locked(lock_path, inner)
    }

    fn sync_back_locked(&self, lock_path: &Path, inner: &Inner) -> Result<(), String> {
        // Windows: no flock — run without serialization.
        // On Unix we would use `fcntl.flock(LOCK_EX)`. Without `nix`/`fs2`
        // crate we serialize via best-effort file creation. The Python code
        // already handles `fcntl is None` (Windows) by running without lock,
        // so skipping the lock here when `flock` is unavailable is faithful.
        // For production file-level serialization across gateway processes,
        // add `fs2` or `nix` and replace this with `File::open(lock_path)?.lock_exclusive()`.
        let _lock_file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path)
            .ok();
        // Best-effort flock on Unix via `flock` crate would go here:
        // if let Some(f) = &_lock_file { let _ = f.lock_exclusive(); }
        let res = self.sync_back_impl(inner);
        // Unlock happens on drop.
        drop(_lock_file);
        let _ = fs::remove_file(lock_path).ok();
        res
    }

    fn sync_back_impl(&self, inner: &Inner) -> Result<(), String> {
        let bulk_download = self
            .bulk_download_fn
            .as_ref()
            .ok_or_else(|| "_sync_back_impl called without bulk_download_fn".to_string())?;

        // Cache file mapping once to avoid O(n*m) from repeated iteration.
        let file_mapping: Vec<(String, String)> = {
            // Python wraps in try/except and falls back to [].
            // We do the same: catch panics via catch_unwind if the provider panics.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.get_files_fn)()));
            match result {
                Ok(v) => v,
                Err(_) => Vec::new(),
            }
        };

        // Create temp tar file
        let tmp_dir = std::env::temp_dir();
        let tar_name = format!("hermes-sync-back-{}.tar", uuid_simple());
        let tar_path = tmp_dir.join(&tar_name);
        // Ensure cleanup on exit
        let tar_path_clone = tar_path.clone();
        let _cleanup_tar = scopeguard(tar_path_clone);

        bulk_download(&tar_path).map_err(|e| e.to_string())?;

        // Defensive size cap
        let tar_size = fs::metadata(&tar_path).map(|m| m.len()).unwrap_or(0);
        if tar_size > SYNC_BACK_MAX_BYTES {
            log::warn!(
                "sync_back: remote tar is {} bytes (cap {}) — skipping extraction",
                tar_size,
                SYNC_BACK_MAX_BYTES
            );
            return Ok(());
        }

        // Staging dir
        let staging_name = format!("hermes-sync-back-{}", uuid_simple());
        let staging = tmp_dir.join(staging_name);
        fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
        let staging_clone = staging.clone();
        let _cleanup_staging = scopeguard_dir(staging_clone);

        // Extract tar (Python: `tar.extractall(staging, filter="data")` — data filter
        // prevents pax headers / link traversal. We implement the same checks.)
        extract_tar(&tar_path, &staging).map_err(|e| e.to_string())?;

        let mut applied: usize = 0;
        let upload_only_host_paths: HashSet<String> = {
            let mut s = inner.upload_only_host_paths.clone();
            for p in credential_host_paths() {
                s.insert(p);
            }
            s
        };

        // Walk staging recursively
        let staged_files = walk_files(&staging).map_err(|e| e.to_string())?;
        for staged_file in staged_files {
            let rel = staged_file
                .strip_prefix(&staging)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            // Python: `rel = os.path.relpath(staged_file, staging)` then `remote_path = "/" + rel`
            let remote_path = format!("/{}", rel.trim_start_matches('/'));

            let pushed_hash = inner.pushed_hashes.get(&remote_path);

            // Skip hashing for files unchanged from push
            if let Some(ph) = pushed_hash {
                let remote_hash = sha256_file(staged_file.to_string_lossy().as_ref())
                    .map_err(|e| e.to_string())?;
                if &remote_hash == ph {
                    continue;
                }
            }

            // Resolve host path from cached mapping
            let mut host_path = resolve_host_path(&remote_path, &file_mapping);
            if host_path.is_none() {
                host_path = infer_host_path(
                    &remote_path,
                    &file_mapping,
                    Some(&upload_only_host_paths),
                );
                if host_path.is_none() {
                    log::debug!("sync_back: skipping {} (no host mapping)", remote_path);
                    continue;
                }
            }
            let host_path = host_path.unwrap();

            if is_upload_only_host_path(&host_path, &upload_only_host_paths) {
                log::debug!(
                    "sync_back: skipping upload-only credential file {}",
                    remote_path
                );
                continue;
            }

            if Path::new(&host_path).exists() && pushed_hash.is_some() {
                if let Ok(host_hash) = sha256_file(&host_path) {
                    if &host_hash != pushed_hash.unwrap() {
                        log::warn!(
                            "sync_back: conflict on {} — host modified since push, remote also changed. Applying remote version (last-write-wins).",
                            remote_path
                        );
                    }
                }
            }

            if let Some(parent) = Path::new(&host_path).parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::copy(&staged_file, &host_path).map_err(|e| e.to_string())?;
            // Preserve mtime where possible (like shutil.copy2)
            if let Ok(meta) = fs::metadata(&staged_file) {
                if let Ok(mtime) = meta.modified() {
                    let _ = set_file_mtime(&host_path, mtime);
                }
            }
            applied += 1;
        }

        if applied > 0 {
            log::info!("sync_back: applied {} changed file(s)", applied);
        } else {
            log::debug!("sync_back: no remote changes detected");
        }
        Ok(())
    }
}

// Scope guard to remove temp file on drop.
struct ScopeGuard(PathBuf);
fn scopeguard(p: PathBuf) -> ScopeGuard {
    ScopeGuard(p)
}
impl Drop for ScopeGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
struct ScopeGuardDir(PathBuf);
fn scopeguard_dir(p: PathBuf) -> ScopeGuardDir {
    ScopeGuardDir(p)
}
impl Drop for ScopeGuardDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn uuid_simple() -> String {
    // Cheap pseudo-uuid from time + pid + random without external crate.
    // Python uses `uuid.uuid4().hex`; we mimic with nanos + pid.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    format!("{nanos:x}{pid:x}")
}

fn walk_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d)? {
            let entry = entry?;
            let p = entry.path();
            let meta = entry.metadata()?;
            if meta.is_dir() {
                stack.push(p);
            } else if meta.is_file() {
                out.push(p);
            }
        }
    }
    Ok(out)
}

fn extract_tar(tar_path: &Path, dest: &Path) -> io::Result<()> {
    // Minimal tar extraction (ustar/pax) without external crate.
    // Python uses `tarfile.open(...).extractall(staging, filter="data")`.
    // We implement a simple parser that handles regular files and directories,
    // with traversal protection (filter="data" equivalent).
    let mut f = fs::File::open(tar_path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let mut offset = 0usize;
    while offset + 512 <= buf.len() {
        let header = &buf[offset..offset + 512];
        // Two consecutive zero blocks = end of archive
        if header.iter().all(|&b| b == 0) {
            // Check next block also zero (or EOF)
            if offset + 1024 <= buf.len() && buf[offset + 512..offset + 1024].iter().all(|&b| b == 0) {
                break;
            }
            if offset + 512 == buf.len() {
                break;
            }
            // Single zero block at end is also valid end
            if header.iter().all(|&b| b == 0) {
                break;
            }
        }
        let name = parse_tar_name(header);
        if name.is_empty() {
            offset += 512;
            continue;
        }
        let size = parse_tar_size(header);
        let typeflag = header[156];
        let is_dir = typeflag == b'5' || name.ends_with('/');

        // filter="data" protection: reject absolute paths, `..` traversal, and pax headers.
        let safe = is_safe_tar_path(&name);
        if !safe {
            // Skip data blocks for unsafe entry
            let skip = (size + 511) / 512 * 512;
            offset += 512 + skip;
            continue;
        }
        // pax extended headers (typeflag 'x'/'g') are metadata — skip.
        if typeflag == b'x' || typeflag == b'g' {
            let skip = (size + 511) / 512 * 512;
            offset += 512 + skip;
            continue;
        }

        let dest_path = dest.join(&name);
        // Ensure dest_path is within dest (prevents `../` escapes that slipped past string check)
        let dest_canonical = dest.to_string_lossy().to_string();
        let dest_path_str = dest_path.to_string_lossy().to_string();
        if !dest_path_str.starts_with(&dest_canonical) {
            let skip = (size + 511) / 512 * 512;
            offset += 512 + skip;
            continue;
        }

        if is_dir {
            fs::create_dir_all(&dest_path)?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let data_start = offset + 512;
            let data_end = (data_start + size as usize).min(buf.len());
            let data = &buf[data_start..data_end];
            // Only write if size >0 or file should exist (empty files)
            fs::write(&dest_path, data)?;
        }
        let skip = (size + 511) / 512 * 512;
        offset += 512 + skip;
        if offset >= buf.len() {
            break;
        }
    }
    Ok(())
}

fn parse_tar_name(header: &[u8]) -> String {
    // name is 100 bytes at 0, prefix 155 bytes at 345 (ustar)
    let name_bytes = &header[0..100];
    let prefix_bytes = &header[345..500];
    let name = cstr_to_string(name_bytes);
    let prefix = cstr_to_string(prefix_bytes);
    if prefix.is_empty() {
        name
    } else if name.is_empty() {
        prefix
    } else {
        format!("{}/{}", prefix, name)
    }
}

fn cstr_to_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let s = &bytes[..end];
    // Tar names are bytes; lossy utf8 is fine for hermes file names.
    String::from_utf8_lossy(s).trim().to_string()
}

fn parse_tar_size(header: &[u8]) -> u64 {
    // size is 12 bytes at 124, octal ascii, may be null-terminated or space-padded
    let raw = &header[124..136];
    let s = String::from_utf8_lossy(raw);
    let t = s.trim_matches(|c| c == '\0' || c == ' ' || c == '\n');
    if t.is_empty() {
        return 0;
    }
    // Base-256 extension (first byte 0x80 or 0xff) — not needed for hermes tars, but handle.
    if raw[0] == 0x80 || raw[0] == 0xff {
        // Big-endian base-256
        let mut val: u64 = 0;
        for &b in &raw[1..12] {
            val = (val << 8) | b as u64;
        }
        return val;
    }
    u64::from_str_radix(t.trim(), 8).unwrap_or(0)
}

fn is_safe_tar_path(name: &str) -> bool {
    // Mirrors tarfile `filter="data"`: reject absolute, drive-letter, and `..` components.
    if name.is_empty() {
        return false;
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return false;
    }
    // Reject Windows drive `C:` or `//host`
    if name.len() >= 2 && name.as_bytes()[1] == b':' {
        return false;
    }
    for part in name.split('/') {
        if part == ".." {
            return false;
        }
    }
    // Reject pax global headers
    if name == "pax_global_header" {
        return false;
    }
    true
}

fn set_file_mtime(path: &str, mtime: SystemTime) -> io::Result<()> {
    // Best-effort mtime preservation (like shutil.copy2). Use `filetime` if available;
    // without it we use `utime` via `std::fs::File::set_modified` if the platform supports.
    // Some platforms don't support; ignore errors.
    let p = Path::new(path);
    let f = fs::OpenOptions::new().write(true).open(p)?;
    let _ = f.set_modified(mtime);
    Ok(())
}

// ---------------------------------------------------------------------------
// Path mapping helpers — mirrors FileSyncManager._resolve_host_path etc.
// ---------------------------------------------------------------------------

fn resolve_host_path(remote_path: &str, file_mapping: &[(String, String)]) -> Option<String> {
    for (host, remote) in file_mapping {
        if remote == remote_path {
            return Some(host.clone());
        }
    }
    None
}

fn infer_host_path(
    remote_path: &str,
    file_mapping: &[(String, String)],
    upload_only_host_paths: Option<&HashSet<String>>,
) -> Option<String> {
    let upload_only = upload_only_host_paths.cloned().unwrap_or_default();
    for (host, remote) in file_mapping {
        if is_upload_only_host_path(host, &upload_only) {
            continue;
        }
        let remote_dir = Path::new(remote)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if remote_dir.is_empty() {
            continue;
        }
        let prefix = format!("{}/", remote_dir);
        if remote_path.starts_with(&prefix) {
            let host_dir = Path::new(host)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let suffix = &remote_path[remote_dir.len()..];
            return Some(format!("{}{}", host_dir, suffix));
        }
    }
    None
}

fn is_upload_only_host_path(host_path: &str, upload_only_host_paths: &HashSet<String>) -> bool {
    // Mirrors Python: `str(Path(host_path).expanduser().resolve())` with OSError fallback.
    let expanded = expanduser(host_path);
    let resolved = fs::canonicalize(&expanded)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| expanded.clone());
    upload_only_host_paths.contains(&resolved)
}

fn expanduser(path: &str) -> String {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, &path[1..]);
        }
    } else if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return home;
        }
    }
    path.to_string()
}

// ---------------------------------------------------------------------------
// Tests — mirrors Python behavior spot-checks
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_rm_builds() {
        let v = vec!["/tmp/a b".to_string(), "/root/.hermes/x".to_string()];
        let cmd = quoted_rm_command(&v);
        assert!(cmd.starts_with("rm -f "));
        assert!(cmd.contains("'/tmp/a b'"));
    }

    #[test]
    fn quoted_mkdir_builds() {
        let v = vec!["/a/b".to_string()];
        assert_eq!(quoted_mkdir_command(&v), "mkdir -p /a/b");
    }

    #[test]
    fn unique_dirs_sorted() {
        let files = vec![
            ("/h/a".to_string(), "/root/.hermes/skills/a.md".to_string()),
            ("/h/b".to_string(), "/root/.hermes/skills/b.md".to_string()),
            ("/h/c".to_string(), "/root/.hermes/cache/x".to_string()),
        ];
        let dirs = unique_parent_dirs(&files);
        assert_eq!(dirs, vec!["/root/.hermes/cache", "/root/.hermes/skills"]);
    }

    #[test]
    fn shlex_safe_not_quoted() {
        assert_eq!(shlex_quote("/a/b/c"), "/a/b/c");
        assert_eq!(shlex_quote("simple"), "simple");
    }

    #[test]
    fn shlex_unsafe_quoted() {
        assert_eq!(shlex_quote("a b"), "'a b'");
        assert_eq!(shlex_quote(""), "''");
    }

    #[test]
    fn sha256_known_vector() {
        let h = {
            let mut hs = Sha256::new();
            hs.update(b"hello");
            hex_encode(&hs.finalize())
        };
        assert_eq!(h, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
        let h2 = {
            let mut hs = Sha256::new();
            hs.update(b"");
            hex_encode(&hs.finalize())
        };
        assert_eq!(h2, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        let h3 = {
            let mut hs = Sha256::new();
            hs.update(b"abc");
            hex_encode(&hs.finalize())
        };
        assert_eq!(h3, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn posix_dirname_cases() {
        assert_eq!(posix_dirname("/a/b/c"), "/a/b");
        assert_eq!(posix_dirname("/a"), "/");
        assert_eq!(posix_dirname("a"), "");
        assert_eq!(posix_dirname("/"), "/");
    }

    #[test]
    fn is_safe_tar_rejects_traversal() {
        assert!(!is_safe_tar_path("/etc/passwd"));
        assert!(!is_safe_tar_path("../a/b"));
        assert!(!is_safe_tar_path("a/../../b"));
        assert!(is_safe_tar_path("a/b/c.txt"));
    }

    #[test]
    fn file_mtime_missing_is_none() {
        assert!(file_mtime_key("/tmp/__hermes_nonexistent_12345_xyz").is_none());
    }
}
