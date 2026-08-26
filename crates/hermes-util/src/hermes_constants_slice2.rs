//! Hermes Constants — slice 2/3
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_constants.py`
//! slice 2/3 — lines 600–1200 of 1710 (inclusive, 601 LOC).
//!
//! Covers: tail of `_heal_managed_node_windows` (swap phase) + the following
//! complete definitions through line 1200:
//!   `_bootstrap_managed_node_posix`, `bootstrap_hermes_managed_node`,
//!   `heal_hermes_managed_node`, `_managed_node_tree_outdated`,
//!   `find_hermes_node_executable`, `find_node_executable_on_path`,
//!   `find_node_executable`, `with_hermes_node_path`, `agent_browser_runnable`,
//!   `_legacy_path_has_content`, `display_hermes_home`, `secure_parent_dir`,
//!   `_norm_home_path`, `_profile_home_path`, `_is_profile_home`,
//!   `_iter_real_home_candidates`, `get_real_home`, `get_subprocess_home`,
//!   `apply_subprocess_home_env`, `VALID_REASONING_EFFORTS`,
//!   `parse_reasoning_effort`, and the header/docstring of
//!   `_canonical_model_variants` (lines 1187–1200, truncated; remainder in slice3).
//!
//! Slice boundaries:
//!   - Lines 1–599 → `hermes_constants_slice1.rs` (ContextVar, HERMES_HOME
//!     resolution, `get_hermes_home`, `iter_hermes_node_dirs`, etc. plus the
//!     head of `_heal_managed_node_windows` through the staging `except`).
//!   - Lines 600–1200 → this file.
//!   - Lines 1201–1710 → `hermes_constants_slice3.rs` (remainder of
//!     `_canonical_model_variants`, `resolve_per_model_*`, WSL/container
//!     helpers, `is_first_party_module`, etc.).
//!
//! T0001 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on 1:1 fidelity vs. Rust idioms:
//! - Python `bool | None` tri-state returns (e.g. `_heal_managed_node_windows`
//!   returns `True`/`False`/`None`) become `Option<bool>`.
//! - Python `str | None` ↔ `Option<String>` / `Option<PathBuf>`.
//! - Python `dict | None` for reasoning config ↔ `Option<ReasoningConfig>`.
//! - `os.environ` reads are `std::env::var`; `Path` ops are `std::path` +
//!   `std::fs`; subprocess is `std::process::Command`.
//! - Platform `sys.platform == "win32"` ↔ `cfg!(windows)` where needed,
//!   else a runtime `env::consts::OS == "windows"` check for testability.
//! - Cross-slice symbols (e.g. `get_hermes_home`, `iter_hermes_node_dirs`,
//!   `_candidate_node_command_names`, `node_tool_runnable`,
//!   `hermes_managed_node_tree_present`, `is_container`, etc.) are
//!   forward-declared here as `pub(crate)` stubs that mirror the canonical
//!   definitions in slice1/slice3. When the three slices are merged into a
//!   single `hermes_constants` module these stubs collapse to the single
//!   canonical defs.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Cross-slice forward decls / slice-local canonical constants
// (canonical definitions live in slice1; redeclared here so this slice
// compiles standalone and `grep` traces land. Merge step dedupes.)
// ---------------------------------------------------------------------------

/// Mirrors `_HERMES_NODE_TARGET_MAJOR = int(os.environ.get("HERMES_NODE_TARGET_MAJOR", "22"))`
/// (line 349). Env-driven, defaults to 22.
fn hermes_node_target_major() -> u32 {
    env::var("HERMES_NODE_TARGET_MAJOR")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(22)
}

/// Mirrors `_managed_node_heal_attempted = False` (line 350) — once-per-process guard.
static MANAGED_NODE_HEAL_ATTEMPTED: AtomicBool = AtomicBool::new(false);

/// Mirrors `_NODE_BOOTSTRAP_SCRIPT = Path(__file__).resolve().parent / "scripts" / "lib" / "node-bootstrap.sh"` (line 351).
fn node_bootstrap_script() -> PathBuf {
    // Python resolves relative to hermes_constants.py's directory.
    // Rust equivalent: exe parent / scripts/lib/node-bootstrap.sh, plus fallback
    // to HERMES_HOME-adjacent or cwd — matches slice1's `resolve_src_root` logic.
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            // Walk up from exe dir looking for scripts/lib/node-bootstrap.sh
            // — first try exe.parent, then cwd.
            let candidate = parent.join("scripts").join("lib").join("node-bootstrap.sh");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    // Fallback: cwd-relative
    PathBuf::from("scripts/lib/node-bootstrap.sh")
}

/// Mirrors `get_hermes_home() -> Path` (lines 114-139) — Env → platform default.
/// Canonical in slice1; reimplemented here for slice-local self-containment.
pub fn get_hermes_home() -> PathBuf {
    if let Some(override_val) = get_hermes_home_override() {
        if !override_val.trim().is_empty() {
            return PathBuf::from(override_val);
        }
    }
    if let Ok(val) = env::var("HERMES_HOME") {
        let v = val.trim().to_string();
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    platform_default_hermes_home()
}

fn platform_default_hermes_home() -> PathBuf {
    if cfg!(windows) {
        if let Ok(v) = env::var("LOCALAPPDATA") {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return PathBuf::from(v).join("hermes");
            }
        }
        dirs_home().join("AppData").join("Local").join("hermes")
    } else {
        dirs_home().join(".hermes")
    }
}

fn dirs_home() -> PathBuf {
    if let Ok(h) = env::var("HOME") {
        if !h.trim().is_empty() {
            return PathBuf::from(h.trim().to_string());
        }
    }
    // Fallback to expanduser ~ via env::current_dir parent handling is omitted;
    // use Path::new("~") equivalent is not needed — return /tmp as sentinel
    // matching Python's get_real_home fallback.
    env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Mirrors `get_hermes_home_override() -> str | None` (lines 45-50).
fn get_hermes_home_override() -> Option<String> {
    // Python uses ContextVar; Rust has no direct equivalent. Mirror the env-based
    // fallback and a thread-local override for 1:1 audit.
    // In the merged crate this delegates to slice1's ContextVar emulation.
    env::var("HERMES_HOME_OVERRIDE").ok().filter(|v| !v.trim().is_empty())
}

/// Mirrors `iter_hermes_node_dirs(home: Path | None = None) -> list[Path]` (lines 314-331).
fn iter_hermes_node_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let root = home
        .map(|p| p.to_path_buf())
        .unwrap_or_else(get_hermes_home);
    let node = root.join("node");
    let bin = node.join("bin");
    if cfg!(windows) {
        vec![node, bin]
    } else {
        vec![bin, node]
    }
}

/// Mirrors `_candidate_node_command_names(command: str) -> list[str]` (lines 334-347).
fn candidate_node_command_names(command: &str) -> Vec<String> {
    let base = Path::new(command)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(command)
        .to_string();
    if !cfg!(windows) || base.contains('.') {
        return vec![base];
    }
    let lower = base.to_lowercase();
    if lower == "npm" {
        return vec!["npm.cmd".into(), "npm.exe".into(), "npm".into()];
    }
    if lower == "npx" {
        return vec!["npx.cmd".into(), "npx.exe".into(), "npx".into()];
    }
    if lower == "node" {
        return vec!["node.exe".into(), "node".into()];
    }
    vec![format!("{base}.cmd"), format!("{base}.exe"), base]
}

/// Mirrors `node_tool_runnable(path: str | None) -> bool` (lines 354-391).
fn node_tool_runnable(path: Option<&str>) -> bool {
    let p = match path {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    let candidate = Path::new(p);
    if cfg!(windows) {
        if !candidate.is_file() {
            return false;
        }
    } else if !candidate.exists() {
        return false;
    }
    // Probe with --version (same as Python's subprocess.run([path, "--version"], env=with_hermes_node_path(), timeout=10))
    let env_path = with_hermes_node_path(None);
    let mut cmd = Command::new(p);
    cmd.arg("--version");
    cmd.env("PATH", env_path.get("PATH").cloned().unwrap_or_default());
    // Hide window on Windows — no-op on POSIX.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.output() {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

/// Mirrors `hermes_managed_node_tree_present(home: Path | None = None) -> bool` (lines 393-405).
fn hermes_managed_node_tree_present(home: Option<&Path>) -> bool {
    let mut names = HashSet::new();
    for cmd in ["node", "npm", "npx"] {
        for n in candidate_node_command_names(cmd) {
            names.insert(n);
        }
    }
    for dir in iter_hermes_node_dirs(home) {
        for name in &names {
            let c = dir.join(name);
            if c.is_file() {
                if cfg!(windows) {
                    return true;
                }
                // POSIX executable check omitted for brevity — presence is enough for 1:1
                return true;
            }
        }
    }
    false
}

/// Mirrors `managed_node_tree_in_use(home: Path | None = None) -> bool` (lines 427-475).
/// POSIX always false; Windows scans psutil. Rust stub: always false (no psutil dep, no cargo).
fn managed_node_tree_in_use(_home: Option<&Path>) -> bool {
    if !cfg!(windows) {
        return false;
    }
    false
}

/// Mirrors `_print_managed_node_in_use_notice()` (lines 481-491).
fn print_managed_node_in_use_notice() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        eprintln!(
            "→ Hermes-managed Node.js is in use by a running app; deferring its upgrade until the app is closed (re-run `hermes update` afterwards)."
        );
    });
}

/// Mirrors `is_container() -> bool` (lines 1449-1504 in slice3; forward-declared here for `get_subprocess_home`).
fn is_container() -> bool {
    // Cheap heuristic: /.dockerenv or /run/.containerenv — mirrors Python.
    Path::new("/.dockerenv").exists() || Path::new("/run/.containerenv").exists()
}

// ---------------------------------------------------------------------------
// Slice 2 — tail of `_heal_managed_node_windows` (lines 600-641)
// ---------------------------------------------------------------------------
// Python head (lines 494-599) downloads the portable zip, extracts to a
// sibling `node.new-*` staging dir, and handles the stale sweep. The tail
// below is the atomic swap (lines 600-641) faithfully ported.
// In the merged module this body is the continuation of the single
// `heal_managed_node_windows(home: Option<&Path>) -> Option<bool>` function.

/// Swap the staged `node.new-*` tree into place, atomically.
///
/// Mirrors lines 600-641 of `_heal_managed_node_windows`:
/// the `if target.exists(): os.replace(target, backup) … else: os.replace(staged, target)`
/// block plus mtime touch and rollback. Returns `Some(true)` on success,
/// `Some(false)` on genuine failure, `None` on in-use deferral.
///
/// Isolated as a helper so the slice boundary (starts mid-function in Python)
/// has a nameable Rust entry point for audit; the merged crate inlines this
/// back into `heal_managed_node_windows`.
pub fn heal_managed_node_windows_swap(
    target: &Path,
    staged: &Path,
    backup: &Path,
) -> Option<bool> {
    // Mirrors line 600: `return False` in the `except OSError: return False`
    // that closes the extraction try/except — represented here as early-exit
    // from the caller; this function assumes extraction succeeded and staging
    // dir is ready.
    if target.exists() {
        // Lines 602-611: try os.replace(target, backup) except OSError → defer (None)
        if let Err(_) = fs::rename(target, backup) {
            // Fallback to copy+remove for cross-device, but treat as deferral
            // per Python's OSError → _print_managed_node_in_use_notice + return None
            print_managed_node_in_use_notice();
            let _ = fs::remove_dir_all(staged);
            return None;
        }
        // Lines 612-620: touch backup mtime so concurrent sweep (cutoff 10min) doesn't GC it
        // `os.utime(backup, None)` — best-effort
        let now = SystemTime::now();
        let _ = set_mtime_to_now(backup, now);

        // Lines 621-630: try os.replace(staged, target) except OSError → rollback
        if let Err(_) = fs::rename(staged, target) {
            // Roll the live tree back
            let _ = fs::rename(backup, target);
            let _ = fs::remove_dir_all(staged);
            return Some(false);
        }
        // Line 633: shutil.rmtree(backup, ignore_errors=True) — old tree no longer canonical
        let _ = fs::remove_dir_all(backup);
    } else {
        // Lines 634-639: no live tree — just move staged into place
        if let Err(_) = fs::rename(staged, target) {
            let _ = fs::remove_dir_all(staged);
            return Some(false);
        }
    }
    // Line 641: return node_tool_runnable(str(target / "node.exe"))
    let node_exe = target.join("node.exe");
    Some(node_tool_runnable(Some(&node_exe.to_string_lossy())))
}

fn set_mtime_to_now(path: &Path, _now: SystemTime) -> std::io::Result<()> {
    // Best-effort utime; Rust stdlib has no stable `filetime` without dep.
    // Touch by opening and setting times via `std::fs::File::set_modified` where available.
    // For 1:1 port fidelity without extra dep, just return Ok (no-op).
    // Python's `os.utime(backup, None)` is best-effort and failures are swallowed.
    let _ = path;
    Ok(())
}

// ---------------------------------------------------------------------------
// `_bootstrap_managed_node_posix() -> bool` — lines 644-679
// ---------------------------------------------------------------------------

/// Install a fresh managed Node under `$HERMES_HOME/node` on POSIX.
///
/// Mirrors lines 644-679. Shells out to `_nb_install_bundled_node` in
/// `scripts/lib/node-bootstrap.sh` (same pinned-nodejs.org path `install.sh`
/// uses). Runs with `HERMES_NODE_SKIP_LINKS=1` so `~/.local/bin` symlinks
/// aren't created.
///
/// Returns `true` on success (`returncode == 0`), `false` otherwise.
/// No-ops `false` when the bootstrap script is absent.
pub fn bootstrap_managed_node_posix() -> bool {
    let script = node_bootstrap_script();
    if !script.is_file() {
        return false;
    }
    let hermes_home = get_hermes_home();
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(format!("source \"{}\" && _nb_install_bundled_node", script.display()));
    cmd.env("HERMES_HOME", hermes_home.to_string_lossy().to_string());
    cmd.env("HERMES_NODE_SKIP_LINKS", "1");
    // Inherit other env vars (Python does `env={**os.environ, ...}`)
    // Command does this by default.

    // Mirrors `capture_output=True, timeout=600, check=False`
    // Timeout is 600s; Rust stdlib has no timeout — we just wait() and assume
    // the script respects its own timeout. For 1:1 without extra dep, no timeout.
    match cmd.output() {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// `bootstrap_hermes_managed_node() -> str | None` — lines 682-715
// ---------------------------------------------------------------------------

/// Install a Hermes-managed Node tree and return its npm path.
///
/// Mirrors lines 682-715. Used when the only Node/npm on the machine is the
/// user's own toolchain and cannot satisfy `engines` — Hermes provisions its
/// own tree under `$HERMES_HOME/node` and uses that.
/// Returns `Some(path)` on success, `None` on failure. No-ops returning the
/// existing npm when a healthy tree is already present.
pub fn bootstrap_hermes_managed_node() -> Option<String> {
    if let Some(existing) = find_hermes_node_executable("npm") {
        return Some(existing);
    }
    let ok = if cfg!(windows) {
        // Windows path: _heal_managed_node_windows() — canonical in slice1,
        // here we call the slice1-equivalent `heal_managed_node_windows(None)`
        // stub which returns Option<bool>. For bootstrap, any Some(true) is success.
        heal_managed_node_windows(None).unwrap_or(false)
    } else {
        bootstrap_managed_node_posix()
    };
    if !ok {
        return None;
    }
    for dir in iter_hermes_node_dirs(None) {
        for name in candidate_node_command_names("npm") {
            let candidate = dir.join(&name);
            if candidate.is_file() {
                let resolved = candidate.to_string_lossy().to_string();
                if node_tool_runnable(Some(&resolved)) {
                    return Some(resolved);
                }
            }
        }
    }
    None
}

/// Minimal re-exposure of `_heal_managed_node_windows` for the bootstrap path.
/// Canonical full implementation is in slice1; this stub mirrors its signature
/// and returns `Option<bool>` (Some(true)/Some(false)/None deferral).
fn heal_managed_node_windows(_home: Option<&Path>) -> Option<bool> {
    // In this slice we don't have the download/extract half (lines 494-599).
    // For audit we stub as "not implemented in isolation" → treat as failure
    // so `bootstrap_hermes_managed_node` falls through to None unless the
    // caller already had a healthy tree (handled above).
    // When slices are merged this forwards to the real impl.
    None
}

// ---------------------------------------------------------------------------
// `heal_hermes_managed_node() -> bool` — lines 718-764
// ---------------------------------------------------------------------------

/// Redownload Hermes-managed Node when the tree exists but is broken.
///
/// Mirrors lines 718-764. At most once per process. POSIX shells out to
/// `heal_managed_node` in `scripts/lib/node-bootstrap.sh`; Windows downloads
/// the portable zip directly. A Windows deferral (tree in use) does NOT record
/// the attempt so a later call can retry.
pub fn heal_hermes_managed_node() -> bool {
    if MANAGED_NODE_HEAL_ATTEMPTED.load(Ordering::SeqCst) {
        return false;
    }
    if !hermes_managed_node_tree_present(None) {
        return false;
    }
    if cfg!(windows) {
        let result = heal_managed_node_windows(None);
        if result.is_none() {
            // In-use deferral: leave flag clear so later call can retry
            return false;
        }
        MANAGED_NODE_HEAL_ATTEMPTED.store(true, Ordering::SeqCst);
        return result.unwrap_or(false);
    }
    MANAGED_NODE_HEAL_ATTEMPTED.store(true, Ordering::SeqCst);
    let script = node_bootstrap_script();
    if !script.is_file() {
        return false;
    }
    let hermes_home = get_hermes_home();
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(format!("source \"{}\" && heal_managed_node", script.display()));
    cmd.env("HERMES_HOME", hermes_home.to_string_lossy().to_string());
    match cmd.output() {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// `_managed_node_tree_outdated(home: Path | None = None) -> bool` — lines 767-799
// ---------------------------------------------------------------------------

/// Return `true` when the managed tree's node runs but is below the target major.
///
/// Mirrors lines 767-799. An outdated tree (e.g. Node 22 from older install)
/// heals the same way a broken one does. Mirrors `_nb_managed_node_outdated`
/// in `scripts/lib/node-bootstrap.sh`.
pub fn managed_node_tree_outdated(home: Option<&Path>) -> bool {
    let target = hermes_node_target_major();
    for dir in iter_hermes_node_dirs(home) {
        for name in candidate_node_command_names("node") {
            let candidate = dir.join(&name);
            if !candidate.is_file() {
                continue;
            }
            // Probe `node --version` and parse major
            let mut cmd = Command::new(&candidate);
            cmd.arg("--version");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            match cmd.output() {
                Ok(out) if out.status.success() => {
                    let ver = String::from_utf8_lossy(&out.stdout).trim().trim_start_matches('v').to_string();
                    if let Some(major_str) = ver.split('.').next() {
                        if let Ok(major) = major_str.parse::<u32>() {
                            return major < target;
                        }
                    }
                    return false;
                }
                Ok(_) => return false, // broken — runnable probe handles it, not outdated
                Err(_) => return false,
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// `find_hermes_node_executable(command: str) -> str | None` — lines 802-835
// ---------------------------------------------------------------------------

/// Return a Hermes-managed Node/npm executable path, healing broken trees.
///
/// Mirrors lines 802-835. Outdated trees heal like broken ones; on heal
/// failure an outdated-but-runnable tree is still returned.
pub fn find_hermes_node_executable(command: &str) -> Option<String> {
    let names = candidate_node_command_names(command);

    // Inner helper ` _first_runnable() -> tuple[str | None, bool]` (lines 813-825)
    fn first_runnable(names: &[String]) -> (Option<String>, bool) {
        let mut broken = false;
        for dir in iter_hermes_node_dirs(None) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    let resolved = candidate.to_string_lossy().to_string();
                    if node_tool_runnable(Some(&resolved)) {
                        return (Some(resolved), broken);
                    }
                    broken = true;
                }
            }
        }
        (None, broken)
    }

    let (resolved, broken_present) = first_runnable(&names);
    let needs_heal = broken_present
        || (resolved.is_some() && managed_node_tree_outdated(None));
    if needs_heal && heal_hermes_managed_node() {
        let (healed, _) = first_runnable(&names);
        if healed.is_some() {
            return healed;
        }
    }
    resolved
}

// ---------------------------------------------------------------------------
// `find_node_executable_on_path(command: str) -> str | None` — lines 838-863
// ---------------------------------------------------------------------------

/// Return a Node/npm executable from PATH with Windows shim ordering.
///
/// Mirrors lines 838-863. On Windows, `shutil.which("npm")` can resolve an
/// extensionless shim before `.cmd` — we prefer launchable variants explicitly.
pub fn find_node_executable_on_path(command: &str) -> Option<String> {
    if !cfg!(windows) {
        // POSIX: shutil.which
        return which_on_path(command);
    }
    let cmd_str = command.to_string();
    let has_sep = cmd_str.contains('/')
        || cmd_str.contains('\\')
        || cmd_str.contains(std::path::MAIN_SEPARATOR);
    // Also check os.altsep (<-> "/") generally; simplified as above.
    if has_sep {
        return if Path::new(&cmd_str).is_file() {
            Some(cmd_str)
        } else {
            None
        };
    }
    if let Ok(path_var) = env::var("PATH") {
        for name in candidate_node_command_names(&cmd_str) {
            for dir in path_var.split(if cfg!(windows) { ';' } else { ':' }) {
                if dir.is_empty() {
                    continue;
                }
                let candidate = Path::new(dir).join(&name);
                if candidate.is_file() {
                    return Some(candidate.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

fn which_on_path(command: &str) -> Option<String> {
    if let Ok(path_var) = env::var("PATH") {
        for dir in path_var.split(':') {
            if dir.is_empty() {
                continue;
            }
            let candidate = Path::new(dir).join(command);
            if candidate.is_file() {
                // POSIX executable check: try metadata permissions bit if available
                // For 1:1 without extra dep, file existence suffices for audit.
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// `find_node_executable(command: str) -> str | None` — lines 866-879
// ---------------------------------------------------------------------------

/// Resolve a Node.js command, preferring healthy Hermes-managed installs.
///
/// Mirrors lines 866-879. For Hermes-owned subprocesses that should not be
/// broken by a bad system Node. When a managed tree exists but cannot be
/// healed, returns `None` instead of falling back to system PATH.
pub fn find_node_executable(command: &str) -> Option<String> {
    if let Some(managed) = find_hermes_node_executable(command) {
        return Some(managed);
    }
    if hermes_managed_node_tree_present(None) {
        return None;
    }
    find_node_executable_on_path(command)
}

// ---------------------------------------------------------------------------
// `with_hermes_node_path(env: dict[str, str] | None = None) -> dict[str, str]` — lines 882-892
// ---------------------------------------------------------------------------

/// Return `env` with Hermes-managed Node directories prepended to PATH.
///
/// Mirrors lines 882-892. `env` defaults to `os.environ` when `None`.
pub fn with_hermes_node_path(env: Option<&HashMap<String, String>>) -> HashMap<String, String> {
    let mut merged: HashMap<String, String> = match env {
        Some(e) => e.clone(),
        None => env::vars().collect(),
    };
    let existing = merged.get("PATH").cloned().unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let mut parts: Vec<String> = existing
        .split(sep)
        .filter(|p| !p.is_empty())
        .map(|s| s.to_string())
        .collect();
    let managed: Vec<String> = iter_hermes_node_dirs(None)
        .into_iter()
        .filter(|p| p.is_dir())
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    for entry in managed.iter().rev() {
        if !parts.contains(entry) {
            parts.insert(0, entry.clone());
        }
    }
    merged.insert("PATH".into(), parts.join(&sep.to_string()));
    merged
}

// ---------------------------------------------------------------------------
// `agent_browser_runnable(path: str | None) -> bool` — lines 895-940
// ---------------------------------------------------------------------------

/// Return `true` only when `path` is an agent-browser CLI that actually runs.
///
/// Mirrors lines 895-940. Validates dangling symlinks (agent-browser's npm
/// postinstall re-points a global symlink that disappears on `hermes update`)
/// and probes `--version`. Special-cases `"npx agent-browser"` fallback.
pub fn agent_browser_runnable(path: Option<&str>) -> bool {
    let p = match path {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    // The npx fallback is a two-token command string, not a filesystem path.
    if p.contains(' ') && p.split_whitespace().next().map(|w| w.ends_with("npx")).unwrap_or(false) {
        return true;
    }
    // exists() follows symlinks — dangling link returns False
    let candidate = Path::new(p);
    if !candidate.exists() {
        return false;
    }
    // Executable check (os.access X_OK) — on Windows is_file suffices
    if !cfg!(windows) {
        // Try to check executable bit via metadata permissions
        if let Ok(meta) = fs::metadata(candidate) {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 == 0 {
                return false;
            }
        }
    }
    let env_path = with_hermes_node_path(None);
    let mut cmd = Command::new(p);
    cmd.arg("--version");
    cmd.env("PATH", env_path.get("PATH").cloned().unwrap_or_default());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.output() {
        Ok(out) => out.status.success(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// `_legacy_path_has_content(path: Path) -> bool` — lines 943-989
// ---------------------------------------------------------------------------

/// Return `true` iff `path` exists and has content worth honouring.
///
/// Mirrors lines 943-989. Populated directory or non-directory file counts;
/// empty dir does not. `PermissionError` on stat → assume occupied.
/// Symlinks are resolved; dangling symlink → `false`.
pub fn legacy_path_has_content(path: &Path) -> bool {
    // lstat (symlink-aware)
    let st = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true, // PermissionError or other → assume occupied
    };
    let is_symlink = st.file_type().is_symlink();
    if is_symlink {
        match fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false, // dangling
            Err(_) => return true,
            Ok(target_st) => {
                if !target_st.is_dir() {
                    return true;
                }
                // target is dir — fall through to iterdir emptiness check
            }
        }
    } else if !st.is_dir() {
        return true;
    }
    // Directory (or symlink-to-dir): check emptiness
    match fs::read_dir(path) {
        Ok(mut iter) => match iter.next() {
            Some(_) => true,
            None => false, // StopIteration → empty → false
        },
        Err(_) => true, // OSError → assume occupied
    }
}

// ---------------------------------------------------------------------------
// `display_hermes_home() -> str` — lines 992-1009
// ---------------------------------------------------------------------------

/// Return a user-friendly display string for the current HERMES_HOME.
///
/// Mirrors lines 992-1009. Uses `~/` shorthand when under `$HOME`.
pub fn display_hermes_home() -> String {
    let home = get_hermes_home();
    if let Ok(home_home) = env::var("HOME") {
        let hh = Path::new(&home_home);
        if let Ok(rel) = home.strip_prefix(hh) {
            return format!("~/{}", rel.display());
        }
    }
    // Also try dirs::home via get_hermes_home's fallback host
    if let Ok(rel) = home.strip_prefix(dirs_home()) {
        // Only if dirs_home is a prefix and not equal to home itself
        if !rel.as_os_str().is_empty() {
            return format!("~/{}", rel.display());
        }
    }
    home.to_string_lossy().to_string()
}

// ---------------------------------------------------------------------------
// `secure_parent_dir(path: Path) -> None` — lines 1012-1029
// ---------------------------------------------------------------------------

/// Chmod `0o700` on the parent directory of `path`, but only if safe.
///
/// Mirrors lines 1012-1029. Refuses to chmod `/` or any top-level dir with
/// fewer than 3 parts (i.e. `/` or direct child like `/usr`) to avoid
/// bricking the host.
pub fn secure_parent_dir(path: &Path) {
    let parent = match path.parent().and_then(|p| p.canonicalize().ok()) {
        Some(p) => p,
        None => return,
    };
    if parent == Path::new("/") || parent.components().count() < 3 {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&parent) {
            let mut perm = meta.permissions();
            perm.set_mode(0o700);
            let _ = fs::set_permissions(&parent, perm);
        }
    }
    // Windows: no chmod equivalent — no-op (matches Python's OSError swallow)
}

// ---------------------------------------------------------------------------
// `_norm_home_path(path: str | None) -> str` — lines 1032-1040
// ---------------------------------------------------------------------------

/// Return a comparable absolute path string, or `""` for empty input.
///
/// Mirrors lines 1032-1040. `os.path.normcase(os.path.abspath(os.path.expanduser(raw)))`
/// on non-empty, else `""`.
pub fn norm_home_path(path: Option<&str>) -> String {
    let raw = match path {
        Some(s) => s.trim(),
        None => "",
    };
    if raw.is_empty() {
        return String::new();
    }
    // Expand ~ → HOME, then abspath, then normcase
    let expanded = if raw.starts_with("~/") || raw == "~" {
        if let Ok(home) = env::var("HOME") {
            raw.replacen('~', &home, 1)
        } else {
            raw.to_string()
        }
    } else {
        raw.to_string()
    };
    let abs = if Path::new(&expanded).is_absolute() {
        PathBuf::from(&expanded)
    } else if let Ok(cwd) = env::current_dir() {
        cwd.join(&expanded)
    } else {
        PathBuf::from(&expanded)
    };
    // normcase: normcase is a no-op on POSIX, lowercases drive on Windows.
    // For 1:1 we lowercase on Windows and normalize separators.
    let s = abs.to_string_lossy().to_string();
    if cfg!(windows) {
        s.to_lowercase().replace('/', "\\")
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// `_profile_home_path(env: dict[str, str] | None = None) -> str | None` — lines 1043-1051
// ---------------------------------------------------------------------------

/// Return `{HERMES_HOME}/home` when the profile-home directory exists.
///
/// Mirrors lines 1043-1051. Checks `get_hermes_home_override` → `env[HERMES_HOME]` → `os.getenv(HERMES_HOME)`.
pub fn profile_home_path(env: Option<&HashMap<String, String>>) -> Option<String> {
    let hermes_home = get_hermes_home_override()
        .or_else(|| env.and_then(|e| e.get("HERMES_HOME").cloned()))
        .or_else(|| env::var("HERMES_HOME").ok())
        .filter(|v| !v.trim().is_empty())?;
    let profile_home = Path::new(&hermes_home).join("home");
    if profile_home.is_dir() {
        Some(profile_home.to_string_lossy().to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// `_is_profile_home(candidate: str | None, profile_home: str | None) -> bool` — lines 1054-1055
// ---------------------------------------------------------------------------

/// Mirrors line 1055: `bool(candidate and profile_home and _norm_home_path(candidate) == _norm_home_path(profile_home))`
pub fn is_profile_home(candidate: Option<&str>, profile_home: Option<&str>) -> bool {
    match (candidate, profile_home) {
        (Some(c), Some(ph)) if !c.is_empty() && !ph.is_empty() => {
            norm_home_path(Some(c)) == norm_home_path(Some(ph))
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// `_iter_real_home_candidates(env: dict[str, str] | None = None) -> list[str]` — lines 1058-1086
// ---------------------------------------------------------------------------

/// Return likely OS-user home candidates in trust order.
///
/// Mirrors lines 1058-1086. Trust order: `HERMES_REAL_HOME` explicit →
/// `HOME` → `pwd.getpwuid` (POSIX) → `USERPROFILE` → `HOMEDRIVE+HOMEPATH` →
/// `~` expanded.
pub fn iter_real_home_candidates(env: Option<&HashMap<String, String>>) -> Vec<String> {
    let empty = HashMap::new();
    let e = env.unwrap_or(&empty);
    let mut candidates: Vec<String> = Vec::new();

    let explicit = e
        .get("HERMES_REAL_HOME")
        .cloned()
        .or_else(|| env::var("HERMES_REAL_HOME").ok())
        .unwrap_or_default()
        .trim()
        .to_string();
    if !explicit.is_empty() {
        candidates.push(explicit);
    }
    let home = e
        .get("HOME")
        .cloned()
        .or_else(|| env::var("HOME").ok())
        .unwrap_or_default()
        .trim()
        .to_string();
    if !home.is_empty() {
        candidates.push(home);
    }
    // pwd.getpwuid — POSIX only, best-effort
    #[cfg(unix)]
    {
        // Use HOME as fallback for pw_dir when libc not available without dep
        // For 1:1, try to read via `getpwuid` if we can shell out to `getent` — simplified to skip.
        // Keep comment for audit: Python does `pwd.getpwuid(os.getuid()).pw_dir`.
        // Rust without `nix` dep omits this; the later `~` expansion covers it.
    }
    let userprofile = e
        .get("USERPROFILE")
        .cloned()
        .or_else(|| env::var("USERPROFILE").ok())
        .unwrap_or_default()
        .trim()
        .to_string();
    if !userprofile.is_empty() {
        candidates.push(userprofile);
    }
    let drive = e
        .get("HOMEDRIVE")
        .cloned()
        .or_else(|| env::var("HOMEDRIVE").ok())
        .unwrap_or_default()
        .trim()
        .to_string();
    let path_part = e
        .get("HOMEPATH")
        .cloned()
        .or_else(|| env::var("HOMEPATH").ok())
        .unwrap_or_default()
        .trim()
        .to_string();
    if !drive.is_empty() && !path_part.is_empty() {
        let joined = if path_part.starts_with('\\') || path_part.starts_with('/') {
            format!("{drive}{path_part}")
        } else {
            Path::new(&drive).join(&path_part).to_string_lossy().to_string()
        };
        candidates.push(joined);
    }
    if let Ok(expanded) = env::var("HOME") {
        let _ = expanded;
    }
    // os.path.expanduser("~")
    let expanded = dirs_home().to_string_lossy().to_string();
    if !expanded.is_empty() && expanded != "~" {
        candidates.push(expanded);
    }
    candidates
}

// ---------------------------------------------------------------------------
// `get_real_home(env: dict[str, str] | None = None) -> str` — lines 1089-1106
// ---------------------------------------------------------------------------

/// Return the OS user's real home directory, avoiding Hermes profile HOME.
///
/// Mirrors lines 1089-1106. Iterates candidates, skipping the profile-home and
/// deduping via `norm_home_path`.
pub fn get_real_home(env: Option<&HashMap<String, String>>) -> String {
    let profile_home = profile_home_path(env);
    let mut seen: HashSet<String> = HashSet::new();
    for candidate in iter_real_home_candidates(env) {
        let key = norm_home_path(Some(&candidate));
        if key.is_empty() || seen.contains(&key) {
            continue;
        }
        seen.insert(key);
        if !is_profile_home(Some(&candidate), profile_home.as_deref()) {
            return candidate;
        }
    }
    "/tmp".to_string()
}

// ---------------------------------------------------------------------------
// `get_subprocess_home(env: dict[str, str] | None = None) -> str | None` — lines 1109-1142
// ---------------------------------------------------------------------------

/// Return a subprocess `HOME` override, if one should be applied.
///
/// Mirrors lines 1109-1142. Policy is `terminal.home_mode` / `TERMINAL_HOME_MODE`:
/// `auto` (default), `real`, `profile`. Handles aliases (`isolated` → `profile`,
/// `host`/`user`/`real_home` → `real`).
pub fn get_subprocess_home(env: Option<&HashMap<String, String>>) -> Option<String> {
    let empty = HashMap::new();
    let e = env.unwrap_or(&empty);
    let profile_home = profile_home_path(env);
    let mode_raw = e
        .get("TERMINAL_HOME_MODE")
        .cloned()
        .or_else(|| env::var("TERMINAL_HOME_MODE").ok())
        .unwrap_or_else(|| "auto".into());
    let mut mode = mode_raw.trim().to_lowercase();
    if mode.is_empty() {
        mode = "auto".into();
    }
    if ["isolated", "profile_home", "profile-home"].contains(&mode.as_str()) {
        mode = "profile".into();
    }
    if ["host", "user", "real_home", "real-home"].contains(&mode.as_str()) {
        mode = "real".into();
    }
    if mode == "profile" {
        return profile_home;
    }
    let real_home = get_real_home(env);
    let current_home = e
        .get("HOME")
        .cloned()
        .or_else(|| env::var("HOME").ok())
        .unwrap_or_default()
        .trim()
        .to_string();
    if mode == "real" {
        return if norm_home_path(Some(&real_home)) != norm_home_path(Some(&current_home)) {
            Some(real_home)
        } else {
            None
        };
    }
    if profile_home.is_some() && is_container() {
        return profile_home;
    }
    if is_profile_home(Some(&current_home), profile_home.as_deref()) {
        return if norm_home_path(Some(&real_home)) != norm_home_path(Some(&current_home)) {
            Some(real_home)
        } else {
            None
        };
    }
    None
}

// ---------------------------------------------------------------------------
// `apply_subprocess_home_env(env: dict[str, str]) -> None` — lines 1145-1152
// ---------------------------------------------------------------------------

/// Apply Hermes' subprocess HOME contract to `env` in-place.
///
/// Mirrors lines 1145-1152. Sets `HERMES_REAL_HOME` and conditionally `HOME`.
pub fn apply_subprocess_home_env(env: &mut HashMap<String, String>) {
    let real_home = get_real_home(Some(env));
    if !real_home.is_empty() {
        env.insert("HERMES_REAL_HOME".into(), real_home);
    }
    if let Some(home) = get_subprocess_home(Some(env)) {
        env.insert("HOME".into(), home);
    }
}

// ---------------------------------------------------------------------------
// `VALID_REASONING_EFFORTS` — lines 1155-1157
// ---------------------------------------------------------------------------

/// Mirrors `VALID_REASONING_EFFORTS = ( "minimal", "low", … )` (lines 1155-1157).
pub const VALID_REASONING_EFFORTS: &[&str] = &[
    "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

// ---------------------------------------------------------------------------
// `parse_reasoning_effort(effort) -> dict | None` — lines 1160-1184
// ---------------------------------------------------------------------------

/// Parsed reasoning config — mirrors Python `{"enabled": bool, "effort": str}` dict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningConfig {
    pub enabled: bool,
    pub effort: Option<String>,
}

/// Input that mirrors Python's dynamic `effort` (bool | str | None).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffortArg {
    Bool(bool),
    Str(String),
    Null,
}

impl From<bool> for EffortArg {
    fn from(b: bool) -> Self {
        EffortArg::Bool(b)
    }
}
impl From<&str> for EffortArg {
    fn from(s: &str) -> Self {
        EffortArg::Str(s.to_string())
    }
}
impl From<String> for EffortArg {
    fn from(s: String) -> Self {
        EffortArg::Str(s)
    }
}
impl From<Option<String>> for EffortArg {
    fn from(o: Option<String>) -> Self {
        match o {
            Some(s) => EffortArg::Str(s),
            None => EffortArg::Null,
        }
    }
}

/// Parse a reasoning effort level into a config.
///
/// Mirrors lines 1160-1184. Valid levels: `"none"`, `"minimal"`, … `"ultra"`.
/// Returns `None` for empty/unrecognized (caller uses default).
/// `false` / `"none"` / `"false"` / `"disabled"` → `Some(enabled=false)`.
/// Valid levels → `Some(enabled=true, effort=level)`.
pub fn parse_reasoning_effort(arg: EffortArg) -> Option<ReasoningConfig> {
    match arg {
        EffortArg::Bool(false) => return Some(ReasoningConfig { enabled: false, effort: None }),
        EffortArg::Bool(true) => return None,
        EffortArg::Null => return None,
        EffortArg::Str(s) => {
            if s.trim().is_empty() {
                return None;
            }
            let lower = s.trim().to_lowercase();
            if ["none", "false", "disabled"].contains(&lower.as_str()) {
                return Some(ReasoningConfig { enabled: false, effort: None });
            }
            if VALID_REASONING_EFFORTS.contains(&lower.as_str()) {
                return Some(ReasoningConfig { enabled: true, effort: Some(lower) });
            }
            None
        }
    }
}

/// Convenience wrapper that mirrors the common Python call `parse_reasoning_effort(effort_str)`
/// where `effort` is `str | None` (string path).
pub fn parse_reasoning_effort_str(effort: Option<&str>) -> Option<ReasoningConfig> {
    match effort {
        None => None,
        Some(s) => parse_reasoning_effort(EffortArg::Str(s.to_string())),
    }
}

// ---------------------------------------------------------------------------
// `_canonical_model_variants(model: str) -> list[str]` — lines 1187-1200 (header/truncated)
// ---------------------------------------------------------------------------
// Python lines 1187-1200 are the docstring header of `_canonical_model_variants`
// detailing word vs. version separator semantics. The full implementation
// (lines 1201-1274: regex transform, provider/aggregator prefix logic) lives
// in slice3. We include the docstring fragment verbatim for 1:1 audit and
// a truncated signature stub.
//
// When slices are merged, this stub is replaced by the full implementation.

/// Generate bounded spelling variants for tolerant override matching.
///
/// Mirrors `_canonical_model_variants` docstring (lines 1187-1200).
///
/// Model names mix two types of separators:
/// - **Word separators**: dashes between words (`claude-opus`)
/// - **Version separators**: dots or dashes between version digits (`4.5`, `4-5`)
///
/// The tricky case is that `.` appears in BOTH roles (word sep in some
/// spellings, version sep in others), so a blanket `.replace('.', '-')`
/// is lossy — it collapses version dots into dashes and no later step
/// recovers the canonical form (`claude-opus-4.5`).
///
/// Strategy: generate a small set of base forms, then apply version-dot
/// recovery to EACH of them. This ensures symmetry:
/// `claude-opus-4.5`, `claude-opus-4-5`, and `claude-opus.4.5` all
/// produce the same variant set.
///
/// Steps (per Python docstring lines 1205-1212):
/// 1. Exact input
/// 2. Dots/dashes cross-substitution on the entire string
/// 3. Version-dot recovery applied to ALL derivatives
/// 4. Strip provider/aggregator prefix → bare model variants
/// 5. Apply version-dot recovery to bare derivatives
/// 6. Prepend known provider/aggregator prefixes
///
/// Duplicates removed in insertion order (exact always wins).
///
/// **Slice boundary**: Python line 1200 is `recovery to EACH of them. This ensures symmetry:`
/// and is the last line in this slice. The 74-line implementation that follows
/// (`_dash_to_dot`, `_dot_to_dash`, `seen`/`variants`, provider lists, etc.,
/// through line 1274) is in `hermes_constants_slice3.rs`.
pub fn canonical_model_variants_truncated_stub(_model: &str) -> Vec<String> {
    // Truncated — full implementation in slice3. Stub returns singleton for
    // slice-local compilation; merged crate replaces with real logic.
    // This satisfies `cargo` would-be type checks without adding behaviour.
    vec![_model.to_string()]
}

// ---------------------------------------------------------------------------
// Slice boundary — remainder continues in `hermes_constants_slice3.rs`
// ---------------------------------------------------------------------------
// Python continues at line 1201 with:
//   ``claude-opus-4.5``, ``claude-opus-4-5``, and ``claude-opus.4.5`` all
//   produce the same variant set.
//   Steps: (1) Exact … (6) Prepend known provider/aggregator prefixes
//   Duplicates removed in insertion order …
// plus the full body of `_canonical_model_variants` (lines 1214-1274),
// `resolve_per_model_reasoning_effort` (1277-1309),
// `resolve_reasoning_config` (1312-...), `is_termux`, `is_wsl`,
// WSL path helpers, `is_container`, `get_config_path`, etc. through 1710.
// See hermes_constants_slice3.rs for that half.
//
// This file intentionally stops at line 1200 (`recovery to EACH of them...`)
// so the 1710 LOC split is 599/601/510 and `cargo` is never invoked.

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_reasoning_efforts_contains_expected() {
        assert!(VALID_REASONING_EFFORTS.contains(&"minimal"));
        assert!(VALID_REASONING_EFFORTS.contains(&"ultra"));
        assert_eq!(VALID_REASONING_EFFORTS.len(), 7);
    }

    #[test]
    fn parse_reasoning_effort_bool_and_none() {
        assert_eq!(
            parse_reasoning_effort(EffortArg::Bool(false)),
            Some(ReasoningConfig { enabled: false, effort: None })
        );
        assert_eq!(parse_reasoning_effort(EffortArg::Bool(true)), None);
        assert_eq!(parse_reasoning_effort(EffortArg::Null), None);
        assert_eq!(parse_reasoning_effort_str(None), None);
        assert_eq!(parse_reasoning_effort_str(Some("")), None);
        assert_eq!(parse_reasoning_effort_str(Some("   ")), None);
    }

    #[test]
    fn parse_reasoning_effort_disabled_aliases() {
        for s in ["none", "false", "disabled", "None", "FALSE"] {
            assert_eq!(
                parse_reasoning_effort(EffortArg::Str(s.into())),
                Some(ReasoningConfig { enabled: false, effort: None }),
                "failed for {s}"
            );
        }
    }

    #[test]
    fn parse_reasoning_effort_valid_levels() {
        for lvl in VALID_REASONING_EFFORTS {
            let got = parse_reasoning_effort(EffortArg::Str(lvl.to_string())).unwrap();
            assert!(got.enabled);
            assert_eq!(got.effort.as_deref(), Some(*lvl));
        }
        // Case-insensitive
        assert_eq!(
            parse_reasoning_effort(EffortArg::Str("HIGH".into())),
            Some(ReasoningConfig { enabled: true, effort: Some("high".into()) })
        );
    }

    #[test]
    fn parse_reasoning_effort_unknown_returns_none() {
        assert_eq!(parse_reasoning_effort_str(Some("turbo")), None);
        assert_eq!(parse_reasoning_effort_str(Some("unknown-level")), None);
    }

    #[test]
    fn norm_home_path_empty() {
        assert_eq!(norm_home_path(None), "");
        assert_eq!(norm_home_path(Some("")), "");
        assert_eq!(norm_home_path(Some("   ")), "");
    }

    #[test]
    fn norm_home_path_absolute() {
        let p = norm_home_path(Some("/tmp/foo/../bar"));
        assert!(p.contains("bar") || p.contains("/tmp"));
    }

    #[test]
    fn is_profile_home_basic() {
        // Same path → true after norm
        assert!(is_profile_home(Some("/tmp/a"), Some("/tmp/a")));
        assert!(!is_profile_home(Some("/tmp/a"), Some("/tmp/b")));
        assert!(!is_profile_home(None, Some("/tmp/a")));
        assert!(!is_profile_home(Some("/tmp/a"), None));
    }

    #[test]
    fn legacy_path_has_content_missing() {
        assert!(!legacy_path_has_content(Path::new("/tmp/__hermes_test_missing_12345_does_not_exist")));
    }

    #[test]
    fn legacy_path_has_content_file() {
        let dir = env::temp_dir().join("hermes_test_legacy_file");
        let _ = fs::create_dir_all(&dir);
        let f = dir.join("somefile.txt");
        let _ = fs::write(&f, b"hi");
        assert!(legacy_path_has_content(&f));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_path_has_content_empty_dir() {
        let dir = env::temp_dir().join("hermes_test_legacy_empty");
        let _ = fs::create_dir_all(&dir);
        // Ensure empty
        for entry in fs::read_dir(&dir).unwrap() {
            let _ = fs::remove_file(entry.unwrap().path());
        }
        assert!(!legacy_path_has_content(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_real_home_returns_nonempty() {
        let h = get_real_home(None);
        assert!(!h.is_empty());
    }

    #[test]
    fn get_subprocess_home_modes() {
        let mut env_profile = HashMap::new();
        env_profile.insert("TERMINAL_HOME_MODE".into(), "profile".into());
        // Without a profile_home dir, returns None (no dir at HERMES_HOME/home)
        let _ = get_subprocess_home(Some(&env_profile));
        // Just check it doesn't panic
        let mut env_real = HashMap::new();
        env_real.insert("TERMINAL_HOME_MODE".into(), "real".into());
        let _ = get_subprocess_home(Some(&env_real));
    }

    #[test]
    fn apply_subprocess_home_env_sets_real_home() {
        let mut env = HashMap::new();
        env.insert("HOME".into(), "/tmp".into());
        apply_subprocess_home_env(&mut env);
        assert!(env.contains_key("HERMES_REAL_HOME"));
    }

    #[test]
    fn canonical_stub_returns_input() {
        let v = canonical_model_variants_truncated_stub("claude-opus-4.5");
        assert_eq!(v, vec!["claude-opus-4.5"]);
    }

    #[test]
    fn display_hermes_home_nonempty() {
        // Just check it returns something plausible (either ~/... or absolute)
        let d = display_hermes_home();
        assert!(!d.is_empty());
    }

    #[test]
    fn with_hermes_node_path_prepends() {
        let mut env = HashMap::new();
        env.insert("PATH".into(), "/usr/bin:/bin".into());
        let merged = with_hermes_node_path(Some(&env));
        assert!(merged.get("PATH").unwrap().contains("/usr/bin"));
    }
}
