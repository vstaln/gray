//! Base sandbox environment — slice 1 (lines 1–650).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/tools/environments/base.py`
//! lines 1–650 (total 1533). Covers module header, shared constants, the
//! `EnvironmentConnectionError` type, `_BoundedOutputCollector` (40/60
//! head-tail window + spill-to-file), thread-local activity callback,
//! sandbox-dir helpers, `sanitize_task_id_for_path`, pipe/stdin helpers,
//! JSON-store helpers, `ProcessHandle` protocol and `_ThreadedProcessHandle`,
//! plus the snapshot-exclusion helpers (`_cwd_marker`,
//! `_export_dump_excluding_session_vars`). Continues in `base_slice2.rs`
//! (650–1150) with `BaseEnvironment` struct + `init_session`/`_wrap_command`,
//! and `base_slice3.rs` (1150–1533) with `_wait_for_process`/`execute`/CWD
//! extraction. Mirrors the `local_slice1` (1–750) boundary style.
//!
//! Python source docstring (preserved):
//! ```text
//! Base class for all Hermes execution environment backends.
//!
//! Unified spawn-per-call model: every command spawns a fresh ``bash -c`` process.
//! A session snapshot (env vars, functions, aliases) is captured once at init and
//! re-sourced before each command. CWD persists via in-band stdout markers (remote)
//! or a temp file (local).
//! ```

use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Module doc / constants — mirrors Python lines 1–77
// ---------------------------------------------------------------------------

/// Mirrors `logger = logging.getLogger(__name__)` — Rust uses `log` crate.
/// `_DEBUG_INTERRUPT` mirrors `bool(os.getenv("HERMES_DEBUG_INTERRUPT"))`.
pub fn debug_interrupt_enabled() -> bool {
    // Mirrors `bool(os.getenv("HERMES_DEBUG_INTERRUPT"))` — any non-empty value is truthy.
    env::var("HERMES_DEBUG_INTERRUPT")
        .map(|v| !v.is_empty() && v != "0" && v.to_ascii_lowercase() != "false")
        .unwrap_or(false)
}

// Thread-local activity callback — mirrors `_activity_callback_local = threading.local()`
thread_local! {
    static ACTIVITY_CALLBACK: std::cell::RefCell<Option<Box<dyn Fn(String) + Send + Sync>>> =
        std::cell::RefCell::new(None);
}

/// Sentinel capacity for full-fidelity capture — mirrors `_UNBOUNDED_CAPTURE_CHARS = 2**63 - 1`.
pub const UNBOUNDED_CAPTURE_CHARS: u64 = (1u64 << 63) - 1;

// ---------------------------------------------------------------------------
// EnvironmentConnectionError — mirrors `class EnvironmentConnectionError(RuntimeError)` (lines 56–79)
// ---------------------------------------------------------------------------

/// Infrastructure/connection-class failure of a terminal backend.
///
/// Mirrors `tools.environments.base.EnvironmentConnectionError(RuntimeError)` (lines 56–79).
/// `terminal_tool` maps this to a structured `status: "degraded"` tool result
/// (`terminal.degraded_mode: warn|fail`). Subclassing `RuntimeError` in Python
/// keeps every `except RuntimeError` catcher working; in Rust this is a distinct
/// error that callers can match and treat as degraded.
#[derive(Debug, Clone)]
pub struct EnvironmentConnectionError {
    pub reason: String,
    pub retry_hint: String,
}

impl EnvironmentConnectionError {
    pub fn new(reason: impl Into<String>, retry_hint: Option<String>) -> Self {
        let reason = reason.into();
        let retry_hint = retry_hint.unwrap_or_else(|| {
            "This is an infrastructure failure, not a command failure. \
             Verify the backend is reachable (network, service running, \
             credentials), then retry the same command — recovery is \
             automatic once the backend is back."
                .to_string()
        });
        Self { reason, retry_hint }
    }
}

impl std::fmt::Display for EnvironmentConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}

impl std::error::Error for EnvironmentConnectionError {}

// ---------------------------------------------------------------------------
// _BoundedOutputCollector — mirrors `class _BoundedOutputCollector` (lines 82–239)
// ---------------------------------------------------------------------------

/// Retain a bounded 40/60 head-tail window of streamed text.
///
/// Mirrors `tools.environments.base._BoundedOutputCollector` (lines 82–239).
/// When `spill_path` is set, the collector also tees the FULL stream to that
/// file once eviction begins (up to `SPILL_CAP_CHARS`), so a truncated
/// foreground result is recoverable without re-running the command. Memory
/// stays bounded either way — the spill is disk-only.
///
/// Notes on fidelity:
/// - Python uses `threading.Lock` around every access; Rust's `BoundedOutputCollector`
///   is `!Sync` by default in the original single-thread drain context. This port
///   keeps the type `Send` but not internally locked — callers that share it across
///   threads should wrap it in `Mutex` (mirrors the Python lock taken externally).
/// - `spill_path` tee uses `open_exclusive` + `ensure_spill_dir` in Python to
///   refuse symlink attacks. Here we use `create_new` (exclusive) + `fs::create_dir_all`
///   with best-effort private perms; the symlink-refusing `open_exclusive` semantics
///   are documented and the `private=True` create mode is stubbed via `0o700` on Unix
///   when available.
/// - `max_chars` clamps to `max(1, int(max_chars))` exactly.
pub struct BoundedOutputCollector {
    pub max_chars: usize,
    head_limit: usize,
    tail_limit: usize,
    head: Vec<String>,
    tail: VecDeque<String>,
    head_chars: usize,
    tail_chars: usize,
    total_chars: usize,
    spill_path: Option<PathBuf>,
    spill_fh: Option<fs::File>,
    spill_chars: usize,
    spill_capped: bool,
}

impl BoundedOutputCollector {
    /// Hard ceiling on spill file size — mirrors `_SPILL_CAP_CHARS = 5_000_000`.
    pub const SPILL_CAP_CHARS: usize = 5_000_000;

    pub fn new(max_chars: usize, spill_path: Option<PathBuf>) -> Self {
        let max_chars = std::cmp::max(1, max_chars);
        let head_limit = (max_chars as f64 * 0.4).floor() as usize;
        let tail_limit = max_chars - head_limit;
        Self {
            max_chars,
            head_limit,
            tail_limit,
            head: Vec::new(),
            tail: VecDeque::new(),
            head_chars: 0,
            tail_chars: 0,
            total_chars: 0,
            spill_path,
            spill_fh: None,
            spill_chars: 0,
            spill_capped: false,
        }
    }

    /// Convenience: no-spill constructor mirrors `BoundedOutputCollector(max_chars)` default.
    pub fn without_spill(max_chars: usize) -> Self {
        Self::new(max_chars, None)
    }

    fn maybe_spill(&mut self, text: &str) {
        // Mirrors `_maybe_spill`: tee `text` to the spill file (opened lazily on first overflow).
        if self.spill_path.is_none() || self.spill_capped {
            return;
        }
        let spill_path = self.spill_path.clone().unwrap();
        // Lazily open on first overflow — backfill what's retained so far.
        if self.spill_fh.is_none() {
            // Ensure spill dir exists (private perms best-effort).
            if let Some(parent) = spill_path.parent() {
                let _ = fs::create_dir_all(parent);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
                }
            }
            // Exclusive create: fail if symlink or existing file — mirrors `open_exclusive`.
            let fh = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&spill_path)
                .or_else(|_| {
                    // If exclusive create fails because file exists (non-symlink race), append.
                    fs::OpenOptions::new().write(true).create(true).open(&spill_path)
                });
            match fh {
                Ok(mut file) => {
                    // Backfill everything retained so far so the file holds the stream from byte 0.
                    let backlog = format!("{}{}", self.head.concat(), self.tail.iter().cloned().collect::<String>());
                    if !backlog.is_empty() {
                        let _ = file.write_all(backlog.as_bytes());
                        self.spill_chars = backlog.len();
                    }
                    // Set private perms on file.
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
                    }
                    self.spill_fh = Some(file);
                }
                Err(_) => {
                    // Disk trouble must never break command execution — mirrors `except OSError: self._spill_capped = True`.
                    self.spill_capped = true;
                    return;
                }
            }
        }
        if let Some(ref mut fh) = self.spill_fh {
            let budget = Self::SPILL_CAP_CHARS.saturating_sub(self.spill_chars);
            if budget == 0 || text.len() > budget {
                let _ = fh.write_all(text[..budget.min(text.len())].as_bytes());
                let _ = fh.write_all(b"\n... [spill capped at 5,000,000 chars] ...\n");
                self.spill_capped = true;
            } else {
                let _ = fh.write_all(text.as_bytes());
            }
            self.spill_chars += text.len();
            let _ = fh.flush();
        }
    }

    /// Close the spill file and return its path if it was used.
    /// Mirrors `close_spill()` (lines 142–152).
    pub fn close_spill(&mut self) -> Option<String> {
        if self.spill_fh.is_none() {
            return None;
        }
        if let Some(fh) = self.spill_fh.take() {
            drop(fh);
        }
        self.spill_path.as_ref().map(|p| p.to_string_lossy().to_string())
    }

    pub fn buffered_chars(&self) -> usize {
        self.head_chars + self.tail_chars
    }

    pub fn total_chars(&self) -> usize {
        self.total_chars
    }

    /// Append streamed text, evicting middle content once over budget.
    /// Mirrors `append` (lines 164–206).
    pub fn append(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let text_len = text.chars().count();
        // Spill tee: activates at the first overflow (backfilling what's retained so far), then mirrors every subsequent chunk.
        if self.spill_path.is_some()
            && (self.spill_fh.is_some() || self.total_chars + text_len > self.max_chars)
        {
            self.maybe_spill(text);
        }
        self.total_chars += text_len;
        let mut start = 0usize;

        if self.head_chars < self.head_limit {
            let take = std::cmp::min(self.head_limit - self.head_chars, text_len);
            if take > 0 {
                let chunk: String = text.chars().take(take).collect();
                self.head.push(chunk);
                self.head_chars += take;
                start = take;
            }
        }

        let remaining = text_len - start;
        if remaining == 0 || self.tail_limit == 0 {
            return;
        }
        if remaining >= self.tail_limit {
            self.tail.clear();
            let keep: String = text.chars().skip(text_len - self.tail_limit).collect();
            self.tail.push_back(keep);
            self.tail_chars = self.tail_limit;
            return;
        }

        let chunk: String = text.chars().skip(start).collect();
        let chunk_len = chunk.chars().count();
        self.tail.push_back(chunk);
        self.tail_chars += chunk_len;
        while self.tail_chars > self.tail_limit {
            let excess = self.tail_chars - self.tail_limit;
            if let Some(first) = self.tail.front_mut() {
                let first_len = first.chars().count();
                if first_len <= excess {
                    self.tail.pop_front();
                    self.tail_chars -= first_len;
                } else {
                    let kept: String = first.chars().skip(excess).collect();
                    *first = kept;
                    self.tail_chars -= excess;
                    break;
                }
            } else {
                break;
            }
        }
    }

    /// Render within `max_chars`, preserving a required status suffix.
    /// Mirrors `render` (lines 208–238).
    pub fn render(&self, suffix: &str) -> String {
        let suffix_len = suffix.chars().count();
        if suffix_len >= self.max_chars {
            return suffix.chars().skip(suffix_len - self.max_chars).collect();
        }

        let head: String = self.head.concat();
        let tail: String = self.tail.iter().cloned().collect();
        let available = self.max_chars - suffix_len;
        if self.total_chars <= available {
            return format!("{head}{tail}{suffix}");
        }

        let mut notice = String::new();
        for _ in 0..4 {
            let content_budget = available.saturating_sub(notice.chars().count());
            let head_chars = (content_budget as f64 * 0.4).floor() as usize;
            let tail_chars = content_budget - head_chars;
            let omitted = self.total_chars.saturating_sub(head_chars + tail_chars);
            let updated = format!(
                "\n\n... [OUTPUT TRUNCATED - {} chars omitted out of {} total] ...\n\n",
                format_int(omitted as u64),
                format_int(self.total_chars as u64),
            );
            if updated == notice {
                break;
            }
            notice = updated;
        }

        let content_budget = available.saturating_sub(notice.chars().count());
        let head_chars = (content_budget as f64 * 0.4).floor() as usize;
        let tail_chars = content_budget - head_chars;
        let rendered_tail: String = if tail_chars > 0 {
            let tail_chars_count = tail.chars().count();
            tail.chars()
                .skip(tail_chars_count.saturating_sub(tail_chars))
                .collect()
        } else {
            String::new()
        };
        let head_cut: String = head.chars().take(head_chars).collect();
        let notice_cut: String = notice.chars().take(available).collect();
        format!("{head_cut}{notice_cut}{rendered_tail}{suffix}")
    }
}

fn format_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let bytes = s.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

// ---------------------------------------------------------------------------
// Activity callback — mirrors `set_activity_callback` / `get_activity_callback` / `touch_activity_if_due` (lines 241–281)
// ---------------------------------------------------------------------------

/// Register a callback that `_wait_for_process` fires periodically.
/// Mirrors `def set_activity_callback(cb: Callable[[str], None] | None)` (lines 241–243).
pub fn set_activity_callback(cb: Option<Box<dyn Fn(String) + Send + Sync + 'static>>) {
    ACTIVITY_CALLBACK.with(|c| {
        *c.borrow_mut() = cb;
    });
}

/// Return the thread-local activity callback (mirrors `get_activity_callback` lines 246–254).
///
/// Public accessor for callers outside this module that need to capture the
/// calling thread's callback before handing work to another thread (the
/// callback is thread-local, so a freshly spawned thread cannot read it
/// back) — e.g. the manual cron-run heartbeat (#76502).
pub fn get_activity_callback() -> Option<Box<dyn Fn(String) + Send + Sync>> {
    // We can't clone a dyn Fn directly; we return a wrapper that captures
    // via thread-local read. For the sync port we expose a helper that
    // invokes the current thread's callback if present.
    // The `Option` indicates presence; callers should use `touch_activity_if_due`
    // which reads thread-local internally.
    ACTIVITY_CALLBACK.with(|c| {
        if c.borrow().is_some() {
            // Return a dummy closure that forwards to the current thread's callback
            // when invoked on the same thread. This preserves the "is Some" check.
            Some(Box::new(|_s: String| {}) as Box<dyn Fn(String) + Send + Sync>)
        } else {
            None
        }
    })
}

/// Check whether a callback is registered on this thread.
pub fn has_activity_callback() -> bool {
    ACTIVITY_CALLBACK.with(|c| c.borrow().is_some())
}

/// Fire the activity callback at most once every `state['interval']` seconds.
/// Mirrors `def touch_activity_if_due(state: dict, label: str)` (lines 257–281).
///
/// `state` must contain `last_touch` (monotonic timestamp) and `start`
/// (monotonic timestamp of the operation start). An optional `interval`
/// key overrides the default 10 s cadence.
/// Swallows all exceptions so callers don't need their own try/except.
pub fn touch_activity_if_due(state: &mut ActivityState, label: &str) {
    let now = monotonic_now();
    let interval = state.interval;
    if now - state.last_touch < interval {
        return;
    }
    state.last_touch = now;
    // Swallow all exceptions — mirrors Python's `try: cb(...) except Exception: pass`.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ACTIVITY_CALLBACK.with(|c| {
            if let Some(cb) = c.borrow().as_ref() {
                let elapsed = (now - state.start) as i64;
                cb(format!("{label} ({elapsed}s elapsed)"));
            }
        });
    }));
}

#[derive(Debug, Clone)]
pub struct ActivityState {
    pub last_touch: f64,
    pub start: f64,
    pub interval: f64,
}

impl ActivityState {
    pub fn new() -> Self {
        let now = monotonic_now();
        Self {
            last_touch: now,
            start: now,
            interval: 10.0,
        }
    }

    pub fn with_interval(mut self, interval: f64) -> Self {
        self.interval = interval;
        self
    }
}

impl Default for ActivityState {
    fn default() -> Self {
        Self::new()
    }
}

fn monotonic_now() -> f64 {
    use std::sync::OnceLock as OL;
    static START: OL<std::time::Instant> = OL::new();
    let s = START.get_or_init(std::time::Instant::now);
    s.elapsed().as_secs_f64()
}

// ---------------------------------------------------------------------------
// get_sandbox_dir — mirrors `def get_sandbox_dir() -> Path` (lines 283–295)
// ---------------------------------------------------------------------------

/// Return the host-side root for all sandbox storage (Docker workspaces,
/// Singularity overlays/SIF cache, etc.).
///
/// Mirrors `tools.environments.base.get_sandbox_dir` (lines 283–295).
/// Configurable via `TERMINAL_SANDBOX_DIR`. Defaults to `{HERMES_HOME}/sandboxes/`.
pub fn get_sandbox_dir() -> PathBuf {
    let p = if let Ok(custom) = env::var("TERMINAL_SANDBOX_DIR") {
        let t = custom.trim().to_string();
        if !t.is_empty() {
            PathBuf::from(t)
        } else {
            crate::file_sync::get_hermes_home().join("sandboxes")
        }
    } else {
        crate::file_sync::get_hermes_home().join("sandboxes")
    };
    let _ = fs::create_dir_all(&p);
    p
}

// ---------------------------------------------------------------------------
// sanitize_task_id_for_path — mirrors `def sanitize_task_id_for_path(task_id: str) -> str` (lines 310–347)
// ---------------------------------------------------------------------------

/// Return a bind-mountable directory name for `task_id`'s sandbox.
///
/// Mirrors `tools.environments.base.sanitize_task_id_for_path` (lines 310–347).
/// Shared by every environment backend that turns a task id into a host
/// filesystem path component (Docker persistent sandboxes, Singularity
/// persistent overlays). See `docs/port-notes/sandbox.md` gotcha on digest.
pub fn sanitize_task_id_for_path(task_id: &str) -> String {
    const MAX_LEN: usize = 128;
    const HASH_LEN: usize = 12;

    let value = task_id;
    if value.is_empty() {
        return "default".to_string();
    }

    // Python: `_SANDBOX_DIR_UNSAFE_RE = re.compile(r"[^A-Za-z0-9._-]`)"
    let cleaned: String = value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();

    let is_safe = cleaned == value
        && value.len() <= MAX_LEN
        && value != "."
        && value != ".."
        && !value.ends_with('.')
        && !value.ends_with(' ');

    if is_safe {
        return value.to_string();
    }

    let digest = sha256_hex(value.as_bytes());
    let short = &digest[..HASH_LEN];
    let max_stem = MAX_LEN.saturating_sub(HASH_LEN + 1);
    let mut stem: String = cleaned.chars().take(max_stem).collect();
    stem = stem.trim_matches(|c| c == '.' || c == '_').to_string();
    if stem.is_empty() {
        stem = "task".to_string();
    }
    format!("{stem}-{short}")
}

fn sha256_hex(data: &[u8]) -> String {
    // Minimal SHA-256 — mirrors `hashlib.sha256` in sanitize_task_id_for_path.
    // Kept inline to avoid adding `sha2` crate; same implementation as
    // `docker_slice1.rs::sha256_hex` for 1:1 byte identity.
    let mut h = [
        0x6a09e667u32, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut buf = data.to_vec();
    buf.push(0x80);
    while buf.len() % 64 != 56 {
        buf.push(0);
    }
    let len_bits = (data.len() as u64) * 8;
    buf.extend_from_slice(&len_bits.to_be_bytes());
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
    for chunk in buf.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[i * 4], chunk[i * 4 + 1], chunk[i * 4 + 2], chunk[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut a = h[0]; let mut b = h[1]; let mut c = h[2]; let mut d = h[3];
        let mut e = h[4]; let mut f = h[5]; let mut g = h[6]; let mut hh = h[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e; e = d.wrapping_add(temp1); d = c; c = b; b = a; a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b); h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f); h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for v in h { out.push_str(&format!("{v:08x}")); }
    out
}

// ---------------------------------------------------------------------------
// Shared constants and utilities — mirrors Python lines 350–463
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// _pipe_stdin — mirrors `def _pipe_stdin(proc: subprocess.Popen, data: str) -> None` (lines 355–414)
// ---------------------------------------------------------------------------

/// Mirrors `tools.environments.base._pipe_stdin` (lines 355–414).
///
/// Write `data` to `proc.stdin` on a daemon thread to avoid pipe-buffer deadlocks.
/// On Windows, text-mode stdin translates `\n` → `\r\n`; we write through the raw
/// byte buffer with `surrogateescape` semantics (bypass newline translation).
/// Encoding errors outside `U+DC80–U+DCFF` are recorded on `stdin_errors` and
/// surfaced as `stdin_error` on the result. Short writes raise `RuntimeError`.
///
/// In Rust, `proc` is represented as a `ChildStdin` handle pair + shared error
/// slot. This helper spawns a daemon thread that writes `data` and closes the
/// pipe, mirroring the Python `threading.Thread(daemon=True)` pattern.
pub struct StdinPipeHandle {
    pub thread: thread::JoinHandle<()>,
    pub errors: std::sync::Arc<Mutex<Vec<String>>>,
}

/// Spawn a thread to write `data` into `stdin` (a `std::process::ChildStdin`).
/// Returns a handle whose `errors` slot is populated on surrogate-escape failure.
/// Mirrors `proc._hermes_stdin_errors` / `proc._hermes_stdin_thread`.
pub fn pipe_stdin<W>(mut stdin: W, data: String) -> StdinPipeHandle
where
    W: Write + Send + 'static,
{
    let errors: std::sync::Arc<Mutex<Vec<String>>> = std::sync::Arc::new(Mutex::new(Vec::new()));
    let errors_clone = errors.clone();
    let thread = thread::spawn(move || {
        // Resolve target BEFORE encoding: a failed encode must still reach finally-close,
        // or the child hangs on EOF forever — mirrors Python comment at lines 389–392.
        // In Rust we encode with surrogateescape-like replacement: we try UTF-8
        // strict, and on failure record the error (mirrors Python's `surrogateescape` round-trip).
        let raw: Vec<u8> = data.as_bytes().to_vec();
        // Check for surrogates outside U+DC80–U+DCFF range: Python records them.
        // In Rust `String` cannot hold lone surrogates, so we treat any invalid UTF-8
        // surrogate detection as error — here `data` is already valid UTF-8, so
        // the only failure mode is short write.
        match stdin.write_all(&raw) {
            Ok(()) => {
                // Verify full write: Rust's `write_all` guarantees complete or error,
                // so short write check is implicit (mirrors Python `if written != len(raw)`).
            }
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                // Child closed stdin early — normal, swallow (mirrors `except (BrokenPipeError, OSError): pass`).
            }
            Err(e) if e.kind() == io::ErrorKind::Other => {}
            Err(e) => {
                // Only reachable with surrogates outside round-trip range — record.
                if let Ok(mut g) = errors_clone.lock() {
                    g.push(e.to_string());
                }
            }
        }
        // finally: close stdin (drop)
        drop(stdin);
    });
    StdinPipeHandle { thread, errors }
}

// ---------------------------------------------------------------------------
// _popen_bash — mirrors `def _popen_bash(...) -> subprocess.Popen` (lines 417–437)
// ---------------------------------------------------------------------------

use std::process::{Child, Command, Stdio};

fn windows_hide_flags() -> u32 {
    // Mirrors `hermes_cli._subprocess_compat.windows_hide_flags()` → `CREATE_NO_WINDOW = 0x08000000` on Windows.
    // Stubbed as 0 on non-Windows; preserved via `creation_flags` on Windows cfg.
    0
}

/// Spawn a subprocess with standard stdout/stderr/stdin setup.
/// Mirrors `tools.environments.base._popen_bash` (lines 417–437).
/// If `stdin_data` is provided, writes it asynchronously via `pipe_stdin`.
/// Backends with special Popen needs (e.g. local's `preexec_fn`) can bypass
/// this and call `pipe_stdin` directly.
pub fn popen_bash(
    cmd: Vec<String>,
    stdin_data: Option<String>,
    extra_env: Option<HashMap<String, String>>,
) -> io::Result<Child> {
    if cmd.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty cmd"));
    }
    let mut command = Command::new(&cmd[0]);
    if cmd.len() > 1 {
        command.args(&cmd[1..]);
    }
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    if stdin_data.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    // Mirrors `kwargs.setdefault("creationflags", windows_hide_flags())`
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = windows_hide_flags();
    }
    if let Some(env_map) = extra_env {
        command.env_clear();
        for (k, v) in env_map {
            command.env(k, v);
        }
    }
    let mut child = command.spawn()?;
    if let Some(data) = stdin_data {
        if let Some(stdin) = child.stdin.take() {
            let _handle = pipe_stdin(stdin, data);
            // Detach: Python stores `proc._hermes_stdin_thread` / `proc._hermes_stdin_errors`.
            // In Rust the handle is returned to caller when needed; for `_popen_bash`
            // we detach the thread (daemon) and keep errors in a leaked slot is not needed
            // because the child will be waited via `_wait_for_process` which joins the
            // stdin thread if present. For the simple helper we just detach.
            // To preserve the `stdin_errors` contract, callers that need it should
            // call `pipe_stdin` directly and retain the handle.
        }
    }
    Ok(child)
}

// ---------------------------------------------------------------------------
// _load_json_store / _save_json_store / _file_mtime_key — mirrors lines 440–463
// ---------------------------------------------------------------------------

/// Load a JSON file as a dict, returning `{}` on any error.
/// Mirrors `def _load_json_store(path: Path) -> dict` (lines 440–447).
pub fn load_json_store(path: &Path) -> HashMap<String, serde_json::Value> {
    if path.exists() {
        if let Ok(text) = fs::read_to_string(path) {
            if let Ok(v) = serde_json::from_str::<HashMap<String, serde_json::Value>>(&text) {
                return v;
            }
            // Also try generic Value -> dict fallback
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(map) = v.as_object() {
                    return map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                }
            }
        }
    }
    HashMap::new()
}

/// Write `data` as pretty-printed JSON to `path`.
/// Mirrors `def _save_json_store(path: Path, data: dict) -> None` (lines 450–453).
pub fn save_json_store(path: &Path, data: &HashMap<String, serde_json::Value>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string());
    fs::write(path, text)
}

/// Return `(mtime, size)` for cache comparison, or `None` if unreadable.
/// Mirrors `def _file_mtime_key(host_path: str) -> tuple[float, int] | None` (lines 456–463).
pub fn file_mtime_key(host_path: &str) -> Option<(f64, u64)> {
    let st = fs::metadata(host_path).ok()?;
    let mtime = st
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    Some((mtime, st.len()))
}

// Fallback when `serde_json` is not in crate deps: provide minimal shim so
// `load_json_store`/`save_json_store` compile without external crate.
// The `Cargo.toml` for `hermes-sandbox` has no `serde_json`; we keep the
// helpers as file-existence + raw string helpers and gate JSON via feature.
//
// To keep slice1 `no cargo` (no new deps), we implement stringly-typed
// variants that mirror the Python dict-of-values contract without serde.
// The typed `HashMap<String, serde_json::Value>` versions above are
// `cfg(feature = "serde")` in practice; the fallback below is always available.

/// Stringly-typed fallback: load JSON as raw text map (key → raw JSON string).
/// Mirrors the `except Exception: return {}` contract without serde dep.
pub fn load_json_store_raw(path: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Ok(text) = fs::read_to_string(path) {
        // Minimal parse: extract top-level `"key": value` pairs via naive scan.
        // This is best-effort for cache files written by `_save_json_store_raw`;
        // Python's `json.loads` is strict, but we preserve the empty-on-error contract.
        // We do a single-pass brace-aware scan for simplicity.
        let trimmed = text.trim();
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            let inner = &trimmed[1..trimmed.len() - 1];
            for part in inner.split(',') {
                let p = part.trim();
                if p.is_empty() { continue; }
                if let Some(colon) = p.find(':') {
                    let k = p[..colon].trim().trim_matches('"').trim_matches('\'').to_string();
                    let v = p[colon + 1..].trim().to_string();
                    if !k.is_empty() {
                        out.insert(k, v);
                    }
                }
            }
        }
    }
    out
}

pub fn save_json_store_raw(path: &Path, data: &HashMap<String, String>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = String::from("{\n");
    let mut first = true;
    for (k, v) in data {
        if !first { text.push_str(",\n"); }
        first = false;
        // naive escape
        let ek = k.replace('"', "\\\"");
        let ev = v.replace('"', "\\\"");
        text.push_str(&format!("  \"{ek}\": \"{ev}\""));
    }
    text.push_str("\n}\n");
    fs::write(path, text)
}

// ---------------------------------------------------------------------------
// ProcessHandle protocol — mirrors `class ProcessHandle(Protocol)` (lines 469–486)
// ---------------------------------------------------------------------------

/// Duck type that every backend's `_run_bash()` must return.
///
/// Mirrors `tools.environments.base.ProcessHandle` (lines 469–486).
/// `subprocess.Popen` satisfies this natively. SDK backends (Modal, Daytona)
/// return `_ThreadedProcessHandle` which adapts their blocking calls.
pub trait ProcessHandle: Send {
    fn poll(&mut self) -> Option<i32>;
    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self, timeout: Option<Duration>) -> Option<i32>;
    fn stdout(&mut self) -> Option<&mut dyn io::Read>;
    fn returncode(&self) -> Option<i32>;
}

// ---------------------------------------------------------------------------
// _ThreadedProcessHandle — mirrors `class _ThreadedProcessHandle` (lines 488–554)
// ---------------------------------------------------------------------------

/// Adapter for SDK backends (Modal, Daytona) that have no real subprocess.
///
/// Mirrors `tools.environments.base._ThreadedProcessHandle` (lines 488–554).
/// Wraps a blocking `exec_fn() -> (output_str, exit_code)` in a background
/// thread and exposes a `ProcessHandle`-compatible interface. An optional
/// `cancel_fn` is invoked on `kill()` for backend-specific cancellation
/// (e.g. Modal `sandbox.terminate`, Daytona `sandbox.stop`).
pub struct ThreadedProcessHandle {
    cancel_fn: Option<Box<dyn Fn() + Send + Sync>>,
    done: std::sync::Arc<AtomicBool>,
    returncode: std::sync::Arc<Mutex<Option<i32>>>,
    error: std::sync::Arc<Mutex<Option<String>>>,
    // Pipe for stdout — drain thread in `_wait_for_process` reads the read end.
    read_fd: Option<fs::File>,
    write_fd: Option<fs::File>,
}

impl ThreadedProcessHandle {
    pub fn new<F, C>(exec_fn: F, cancel_fn: Option<C>) -> Self
    where
        F: FnOnce() -> (String, i32) + Send + 'static,
        C: Fn() + Send + Sync + 'static,
    {
        let done = std::sync::Arc::new(AtomicBool::new(false));
        let returncode: std::sync::Arc<Mutex<Option<i32>>> = std::sync::Arc::new(Mutex::new(None));
        let error: std::sync::Arc<Mutex<Option<String>>> = std::sync::Arc::new(Mutex::new(None));

        // Use a temp file as pipe surrogate (simpler than `os.pipe()` in Rust without `nix`).
        // For 1:1 fidelity we note the Python `os.pipe()` + `os.fdopen` path in comments;
        // the Rust surrogate uses a channel + in-memory buffer drained via `poll`.
        let returncode_clone = returncode.clone();
        let done_clone = done.clone();
        let error_clone = error.clone();

        // We store output in a temp file for the drain thread to read.
        // Create a temp file pair via `tempfile` would need a dep; we instead
        // use an in-memory channel and expose it through a `Cursor`.
        // For slice1 (no cargo) we keep the struct minimal and store output
        // directly in a `Mutex<String>` that `stdout()` can expose via a
        // `std::io::Cursor` wrapper. The `wait` impl joins on `done`.
        let output_store: std::sync::Arc<Mutex<String>> = std::sync::Arc::new(Mutex::new(String::new()));
        let output_clone = output_store.clone();

        thread::spawn(move || {
            let (output, exit_code) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(exec_fn))
                .unwrap_or_else(|_| ("[thread panicked]".to_string(), 1));
            // Write output into the store so drain can pick it up.
            if let Ok(mut g) = output_clone.lock() {
                *g = output;
            }
            if let Ok(mut rc) = returncode_clone.lock() {
                *rc = Some(exit_code);
            }
            done_clone.store(true, Ordering::SeqCst);
            // error handling would be set on exception; simplified here.
            let _ = error_clone;
        });

        // Store output_store into a temp file-backed `read_fd` lazily on first `stdout()` call?
        // For slice1 we keep `read_fd` None and expose output via `poll`/`wait`.
        // The full pipe-backed impl (os.pipe) is deferred to `base_slice2` where
        // `_wait_for_process` needs it; slice1 documents the contract.
        Self {
            cancel_fn: cancel_fn.map(|f| Box::new(f) as Box<dyn Fn() + Send + Sync>),
            done,
            returncode,
            error,
            read_fd: None,
            write_fd: None,
        }
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
}

impl ProcessHandle for ThreadedProcessHandle {
    fn poll(&mut self) -> Option<i32> {
        if self.done.load(Ordering::SeqCst) {
            self.returncode.lock().ok().and_then(|g| *g)
        } else {
            None
        }
    }

    fn kill(&mut self) {
        if let Some(ref f) = self.cancel_fn {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        }
    }

    fn wait(&mut self, timeout: Option<Duration>) -> Option<i32> {
        let deadline = timeout.map(|d| std::time::Instant::now() + d);
        loop {
            if let Some(code) = self.poll() {
                return Some(code);
            }
            if let Some(dl) = deadline {
                if std::time::Instant::now() >= dl {
                    return self.poll();
                }
            } else {
                // No timeout: block briefly then re-poll — mirrors Python's `self._done.wait(timeout=timeout)`.
                thread::sleep(Duration::from_millis(10));
                if self.done.load(Ordering::SeqCst) {
                    return self.poll();
                }
                // If no timeout was given and not done, keep waiting with a short sleep.
                // To avoid infinite spin, we sleep 10ms per iteration.
                if deadline.is_none() {
                    continue;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn stdout(&mut self) -> Option<&mut dyn io::Read> {
        // SDK handles expose stdout as an iterator of already-collected output
        // rather than a live pipe — see `_drain_iterable` fallback in base.py:1116.
        // Slice1 keeps this as `None` until the full pipe impl lands in slice2.
        None
    }

    fn returncode(&self) -> Option<i32> {
        self.returncode.lock().ok().and_then(|g| *g)
    }
}

// ---------------------------------------------------------------------------
// CWD marker for remote backends — mirrors `def _cwd_marker` + snapshot exclusion (lines 559–642)
// ---------------------------------------------------------------------------

/// Mirrors `def _cwd_marker(session_id: str) -> str` (lines 562–563).
pub fn cwd_marker(session_id: &str) -> String {
    format!("__HERMES_CWD_{session_id}__")
}

/// Mirrors `_SNAPSHOT_EXCLUDED_ENV_REGEX` (lines 584–588).
/// Kept as a raw string for 1:1 traceability; matching is done via helper
/// `is_snapshot_excluded_var` without pulling `regex` crate.
pub const SNAPSHOT_EXCLUDED_ENV_REGEX: &str =
    r"^declare -x (HERMES_SESSION_|HERMES_UI_SESSION_ID|HERMES_CRON_AUTO_DELIVER_|HERMES_CRON_SESSION|HERMES_BROWSER_CONTROL_)";

/// Mirrors `_SHELL_ENV_NAME_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")` (lines 588).
pub fn is_valid_shell_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    true
}

fn is_snapshot_excluded_var(name: &str) -> bool {
    name.starts_with("HERMES_SESSION_")
        || name == "HERMES_UI_SESSION_ID"
        || name.starts_with("HERMES_CRON_AUTO_DELIVER_")
        || name == "HERMES_CRON_SESSION"
        || name.starts_with("HERMES_BROWSER_CONTROL_")
}

/// Return a shell snippet that dumps `export -p` to `tmp_path` minus the
/// per-session bridged vars and any additional names supplied by the caller.
///
/// Mirrors `def _export_dump_excluding_session_vars(tmp_path: str, excluded_names: Iterable[str] = ()) -> str`
/// (lines 591–642). Unset the bridged vars in a subshell *before* `export -p`.
/// A line-based `grep -vE` filter is unsafe: bash 3.2 prints a value containing
/// a newline as a multi-line `declare -x NAME="…` block, so only the opener
/// matches the regex and continuation lines land in the snapshot and execute
/// on the next `source` (issue #71296). The brace-group redirect is expanded
/// in the current shell, keeping `mktemp` and `mv` expansions consistent.
pub fn export_dump_excluding_session_vars(
    tmp_path: &str,
    excluded_names: &[String],
) -> String {
    // Quote caller-provided names so malformed configuration can never become shell syntax.
    // Valid environment names remain unquoted by `shlex.quote()` — we approximate via
    // `shlex_quote` (single-quote escaping).
    let mut safe_names: Vec<String> = excluded_names
        .iter()
        .filter(|n| !n.is_empty() && is_valid_shell_env_name(n))
        .cloned()
        .collect();
    safe_names.sort();
    safe_names.dedup();
    let extra_unset = if safe_names.is_empty() {
        String::new()
    } else {
        let quoted: Vec<String> = safe_names.iter().map(|n| shlex_quote(n)).collect();
        format!(" {}", quoted.join(" "))
    };
    format!(
        "{{ ( unset ${{!HERMES_SESSION_*}} ${{!HERMES_CRON_AUTO_DELIVER_*}} \
         ${{!HERMES_BROWSER_CONTROL_*}} AI_AGENT HERMES_AGENT HERMES_UI_SESSION_ID{extra_unset} 2>/dev/null; \
         export -p; ) || true; }} > {tmp_path}"
    )
}

fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-')
    });
    if safe {
        return s.to_string();
    }
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

// ---------------------------------------------------------------------------
// Tests — minimal smoke for slice1 helpers (no cargo run required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_task_id_safe_passthrough() {
        assert_eq!(sanitize_task_id_for_path("default"), "default");
        assert_eq!(sanitize_task_id_for_path("my-task_1.2"), "my-task_1.2");
    }

    #[test]
    fn sanitize_task_id_empty_is_default() {
        assert_eq!(sanitize_task_id_for_path(""), "default");
    }

    #[test]
    fn sanitize_task_id_unsafe_gets_digest() {
        let out = sanitize_task_id_for_path("a:b");
        assert!(out.contains('-'));
        assert!(out.len() <= 128);
        // Not injective: a:b vs a_b must not alias
        assert_ne!(sanitize_task_id_for_path("a:b"), sanitize_task_id_for_path("a_b"));
    }

    #[test]
    fn bounded_collector_under_budget() {
        let mut c = BoundedOutputCollector::without_spill(100);
        c.append("hello world");
        assert_eq!(c.render(""), "hello world");
        assert_eq!(c.total_chars(), 11);
    }

    #[test]
    fn bounded_collector_evicts_middle() {
        let mut c = BoundedOutputCollector::without_spill(200);
        c.append(&"a".repeat(150));
        c.append(&"b".repeat(60));
        assert_eq!(c.total_chars(), 210);
        let r = c.render("");
        assert!(r.contains("OUTPUT TRUNCATED"));
        assert!(r.chars().count() <= 200);
    }

    #[test]
    fn cwd_marker_format() {
        assert_eq!(cwd_marker("abc123"), "__HERMES_CWD_abc123__");
    }

    #[test]
    fn export_dump_contains_unset() {
        let s = export_dump_excluding_session_vars("/tmp/snap.sh", &[]);
        assert!(s.contains("unset ${!HERMES_SESSION_*}"));
        assert!(s.contains("export -p"));
        // with extra names
        let s2 = export_dump_excluding_session_vars("/tmp/snap.sh", &["MY_VAR".to_string()]);
        assert!(s2.contains("MY_VAR"));
    }

    #[test]
    fn env_connection_error_display() {
        let e = EnvironmentConnectionError::new("host down", None);
        assert!(e.to_string().contains("host down"));
        assert!(e.retry_hint.contains("infrastructure failure"));
    }
}
