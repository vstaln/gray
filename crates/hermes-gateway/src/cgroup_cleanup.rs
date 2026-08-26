//! SIGKILL any process left in this systemd unit's cgroup.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/cgroup_cleanup.py` (81 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! SIGKILL any process left in this systemd unit's cgroup.
//!
//! Runs as ``ExecStopPost=`` so it only fires after the gateway's main process
//! has exited. The gateway already reaps its own tool subprocesses on a clean
//! shutdown; this is the safety net for long-lived helpers it doesn't track
//! (``adb``, platform bridges, etc.) that would otherwise be orphaned in the
//! cgroup and block ``Restart=always`` — issue #37454.
//!
//! We deliberately iterate ``cgroup.procs`` and send per-PID SIGKILLs instead
//! of writing ``1`` to ``cgroup.kill``: the original failure mode in #37454
//! was the kernel returning ``EINVAL`` on the cgroup-wide kill, while per-PID
//! signal delivery uses a separate code path that still works.
//! ```
//!
//! Mapping:
//! - `def _own_cgroup_path() -> str | None` → [`_own_cgroup_path`] / [`own_cgroup_path`]
//! - `def _read_cgroup_pids(cgroup_path: str) -> list[int]` → [`_read_cgroup_pids`] / [`read_cgroup_pids`]
//! - `def reap_cgroup(cgroup_path: str | None = None) -> int` → [`reap_cgroup`]
//! - `def main() -> int` → [`main`]
//! - `re.search(r"^0::(.+)$", text, re.MULTILINE)` → line-by-line `strip_prefix("0::")` scan
//! - `Path(f"/sys/fs/cgroup{cgroup_path}/cgroup.procs").read_text()` → [`_read_cgroup_pids`] `read_to_string`
//! - `os.getpid()` → `std::process::id()`
//! - `os.kill(pid, signal.SIGKILL)` → [`kill_pid`] (`kill(2)` via `extern "C"`, `SIGKILL=9`; Linux-only)
//! - `except ProcessLookupError` / `except PermissionError` → `ESRCH(3)` / `EPERM(1)`/`EACCES(13)` arms

use std::path::Path;

// ---------------------------------------------------------------------------
// Internal helpers — mirrors Python underscore-prefixed helpers
// ---------------------------------------------------------------------------

/// Return the cgroup v2 path for the calling process, or `None`.
///
/// Mirrors:
/// ```python
/// def _own_cgroup_path() -> str | None:
///     try:
///         text = Path("/proc/self/cgroup").read_text(encoding="utf-8")
///     except OSError:
///         return None
///     match = re.search(r"^0::(.+)$", text, re.MULTILINE)
///     if not match:
///         return None
///     return match.group(1).strip()
/// ```
pub fn _own_cgroup_path() -> Option<String> {
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    // re.MULTILINE ^0::(.+)$ — scan each line for leading "0::" with at least one char after.
    for line in text.lines() {
        if let Some(content) = line.strip_prefix("0::") {
            // (.+) requires at least one character on that line after "0::"
            if content.is_empty() {
                continue;
            }
            return Some(content.trim().to_string());
        }
    }
    None
}

/// Public alias for readability.
pub fn own_cgroup_path() -> Option<String> {
    _own_cgroup_path()
}

/// Read PIDs from `/sys/fs/cgroup{cgroup_path}/cgroup.procs`.
///
/// Mirrors:
/// ```python
/// def _read_cgroup_pids(cgroup_path: str) -> list[int]:
///     procs_file = Path(f"/sys/fs/cgroup{cgroup_path}/cgroup.procs")
///     try:
///         raw = procs_file.read_text(encoding="utf-8")
///     except OSError:
///         return []
///     pids: list[int] = []
///     for line in raw.splitlines():
///         line = line.strip()
///         if not line:
///             continue
///         try:
///             pids.append(int(line))
///         except ValueError:
///             continue
///     return pids
/// ```
pub fn _read_cgroup_pids(cgroup_path: &str) -> Vec<i32> {
    let procs_file = format!("/sys/fs/cgroup{}/cgroup.procs", cgroup_path);
    let raw = match std::fs::read_to_string(Path::new(&procs_file)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut pids = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(pid) = trimmed.parse::<i32>() {
            pids.push(pid);
        }
    }
    pids
}

/// Public alias.
pub fn read_cgroup_pids(cgroup_path: &str) -> Vec<i32> {
    _read_cgroup_pids(cgroup_path)
}

// ---------------------------------------------------------------------------
// kill helper — mirrors `os.kill(pid, signal.SIGKILL)`
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn kill_pid(pid: i32) -> Result<(), i32> {
    const SIGKILL: i32 = 9;
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let rc = unsafe { kill(pid, SIGKILL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(-1))
    }
}

#[cfg(not(unix))]
fn kill_pid(_pid: i32) -> Result<(), i32> {
    // windows-footgun: ok — Linux-only (reads /proc, /sys/fs/cgroup; runs from a systemd unit)
    // On non-Unix, pretend PermissionError so the caller skips.
    Err(1)
}

// ---------------------------------------------------------------------------
// Public API — mirrors Python top-level functions
// ---------------------------------------------------------------------------

/// SIGKILL every PID in the cgroup other than the caller. Returns the count killed.
///
/// Mirrors:
/// ```python
/// def reap_cgroup(cgroup_path: str | None = None) -> int:
///     if cgroup_path is None:
///         cgroup_path = _own_cgroup_path()
///     if not cgroup_path:
///         return 0
///     own = os.getpid()
///     killed = 0
///     for pid in _read_cgroup_pids(cgroup_path):
///         if pid == own:
///             continue
///         try:
///             os.kill(pid, signal.SIGKILL)
///             killed += 1
///         except ProcessLookupError:
///             continue
///         except PermissionError:
///             continue
///     return killed
/// ```
pub fn reap_cgroup(cgroup_path: Option<&str>) -> usize {
    let resolved: Option<String> = match cgroup_path {
        Some(p) => Some(p.to_string()),
        None => _own_cgroup_path(),
    };
    let cgroup_path = match resolved {
        Some(s) if !s.trim().is_empty() => s,
        _ => return 0,
    };
    // Mirror `if not cgroup_path: return 0` — empty/whitespace-only is falsy.
    if cgroup_path.trim().is_empty() {
        return 0;
    }
    let own = std::process::id() as i32;
    let mut killed: usize = 0;
    for pid in _read_cgroup_pids(&cgroup_path) {
        if pid == own {
            continue;
        }
        match kill_pid(pid) {
            Ok(()) => killed += 1,
            Err(errno) if errno == 3 => continue,             // ESRCH — ProcessLookupError
            Err(errno) if errno == 1 || errno == 13 => continue, // EPERM/EACCES — PermissionError
            Err(_) => continue, // best-effort: any other OSError also skipped (e.g. EINVAL)
        }
    }
    killed
}

/// Entrypoint — mirrors Python `def main() -> int: reap_cgroup(); return 0`.
///
/// The Python `if __name__ == "__main__": sys.exit(main())` is not needed in a
/// library port; callers can invoke `reap_cgroup(None)` directly or call this
/// helper from a binary's `fn main`.
pub fn main() -> i32 {
    reap_cgroup(None);
    0
}

// Provide private aliases mirroring Python's underscore helpers for traceability
#[allow(dead_code)]
fn _reap_cgroup(cgroup_path: Option<&str>) -> usize {
    reap_cgroup(cgroup_path)
}
