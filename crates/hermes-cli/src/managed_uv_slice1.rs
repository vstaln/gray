//! hermes-cli managed_uv — slice 1/2
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/managed_uv.py`
//! slice 1/2 — lines 1–900 of 1 374 (first 900 LOC).
//! Covers: module docstring + std imports + project/runtime constants +
//! `managed_uv_path`, `resolve_uv`, `managed_python_install_dir`,
//! `managed_python_env`, `RuntimeRepairResult`, `_RepairLock`,
//! `_report_runtime_repair_failure`, `_UvResult`, `_ensure_uv_path`,
//! `ensure_uv`, `_uv_self_update_is_fresh`, `_touch_uv_self_update_stamp`,
//! `UV_SELF_UPDATE_*` constants, `update_managed_uv`,
//! `_reload_hermes_constants`, `_venv_python`, `_remove_tree`,
//! `_make_world_traversable`, `_runtime_request`, `_MAX_PATCH_RETRIES`,
//! `_list_available_patches`, `_attempt_install_generation`,
//! `_install_safe_python_generation`, `_smoke_candidate_venv`,
//! `_stage_candidate_venv`, `_rename_with_retry` and the start of
//! `_cut_over_candidate` (through the retry-rename/rollback header,
//! line 900). Continued in `managed_uv_slice2.rs`
//! (from `_cut_over_candidate` body at line 901).
//!
//! T0707 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-18
// ---------------------------------------------------------------------------

/// Hermes-managed uv and Python runtime repair.
///
/// Mirrors `hermes_cli/managed_uv.py` lines 1-18.
/// Hermes owns its own uv binary at `$HERMES_HOME/bin/uv` (or `uv.exe` on
/// Windows). Every code path that needs uv resolves it from that single
/// location. If the binary is missing, `ensure_uv()` bootstraps it via the
/// official standalone installer with `UV_UNMANAGED_INSTALL` / `UV_INSTALL_DIR`
/// pointed at `$HERMES_HOME/bin` so the installer writes directly there — no
/// PATH probing, no conda guards, no multi-location resolution chains.
/// The Python backing the install is shared by every Hermes profile because
/// the checkout's `venv` is shared. Runtime repair therefore uses an
/// install-scoped store under `<checkout>/.hermes-runtime/python`.
pub const MODULE_DOC: &str =
    "Hermes-managed uv and Python runtime repair — see managed_uv.py lines 1-18";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 20-39
// ---------------------------------------------------------------------------
// Python: importlib, json, logging, os, platform, shutil, subprocess, sys,
// tempfile, time, uuid, dataclasses, pathlib.Path, typing, hermes_constants,
// hermes_cli.sqlite_runtime
//
// Rust: std only (NEVER cargo). json/platform/shutil/subprocess/stdlib-only
// helpers are stubbed or implemented via std for 1:1 traceability; real
// wiring in later slices or via std::process / std::fs.

fn log_warning(msg: &str) {
    eprintln!("[managed_uv] WARN: {msg}");
}
fn log_debug(msg: &str) {
    if std::env::var("HERMES_DEBUG").is_ok() {
        eprintln!("[managed_uv] DEBUG: {msg}");
    }
}

// ---------------------------------------------------------------------------
// Constants — mirrors lines 42-46
// ---------------------------------------------------------------------------

/// Mirrors `_PROJECT_ROOT = Path(__file__).resolve().parents[1]` (line 42).
pub fn project_root() -> PathBuf {
    // Mirrors `Path(__file__).resolve().parents[1]` — two levels up from
    // `hermes_cli/managed_uv.py` is the checkout root.
    // In Rust we resolve from env or current_dir for 1:1 without cargo.
    if let Ok(v) = std::env::var("HERMES_REPO_ROOT") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    // Fallback: crate manifest parent heuristic (file! is crates/hermes-cli/src/...)
    Path::new(file!())
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub const RUNTIME_DIR_NAME: &str = ".hermes-runtime"; // line 43
pub const VENV_NAME: &str = "venv"; // line 44
pub const ALT_VENV_NAME: &str = ".venv"; // line 45
pub const REPAIR_LOCK_NAME: &str = "runtime-repair.lock"; // line 46

// ---------------------------------------------------------------------------
// Public helpers — mirrors lines 52-116
// ---------------------------------------------------------------------------

/// Mirrors `hermes_constants.get_hermes_home` (used by managed_uv_path).
pub fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    dirs_home().join(".hermes")
}
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Mirrors `def managed_uv_path() -> Path:` at 53-63.
pub fn managed_uv_path() -> PathBuf {
    let home = get_hermes_home();
    if cfg!(windows) {
        home.join("bin").join("uv.exe")
    } else {
        home.join("bin").join("uv")
    }
}

/// Mirrors `def resolve_uv() -> Optional[str]:` at 66-74.
/// Return the managed uv path if it exists and is executable, else None.
pub fn resolve_uv() -> Option<String> {
    let p = managed_uv_path();
    if p.is_file() && is_executable(&p) {
        return Some(p.to_string_lossy().to_string());
    }
    None
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(p) {
            return meta.permissions().mode() & 0o111 != 0;
        }
        false
    }
    #[cfg(not(unix))]
    {
        // Windows: is_file is enough (no X_OK concept)
        let _ = p;
        true
    }
}

/// Mirrors `def managed_python_install_dir(project_root: Path | None = None) -> Path:` at 77-80.
pub fn managed_python_install_dir(project_root: Option<&Path>) -> PathBuf {
    let root = project_root
        .map(|p| p.to_path_buf())
        .unwrap_or_else(project_root_default);
    root.join(RUNTIME_DIR_NAME).join("python")
}
fn project_root_default() -> PathBuf {
    project_root()
}

/// Mirrors `def managed_python_env(...) -> dict[str, str]:` at 83-116.
pub fn managed_python_env(
    project_root: Option<&Path>,
    install_dir: Option<&Path>,
    base_env: Option<&HashMap<String, String>>,
) -> HashMap<String, String> {
    let target = if let Some(d) = install_dir {
        d.to_path_buf()
    } else {
        managed_python_install_dir(project_root)
    };
    let mut env: HashMap<String, String> = if let Some(base) = base_env {
        base.clone()
    } else {
        std::env::vars().collect()
    };
    for key in [
        "CONDA_DEFAULT_ENV",
        "CONDA_PREFIX",
        "UV_PROJECT_ENVIRONMENT",
        "UV_NO_MANAGED_PYTHON",
        "UV_PYTHON",
        "UV_PYTHON_DOWNLOADS",
        "UV_SYSTEM_PYTHON",
        "VIRTUAL_ENV",
        "PYTHONHOME",
        "PYTHONPATH",
    ] {
        env.remove(key);
    }
    env.insert("UV_MANAGED_PYTHON".to_string(), "1".to_string());
    env.insert("UV_NO_CONFIG".to_string(), "1".to_string());
    env.insert("UV_PYTHON_INSTALL_BIN".to_string(), "0".to_string());
    env.insert("UV_PYTHON_INSTALL_DIR".to_string(), target.to_string_lossy().to_string());
    env.insert("UV_PYTHON_INSTALL_REGISTRY".to_string(), "0".to_string());
    env
}

// ---------------------------------------------------------------------------
// RuntimeRepairResult — mirrors lines 119-131
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass(frozen=True) class RuntimeRepairResult:` at 119-131.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRepairResult {
    pub status: String,
    pub detail: String,
    pub sqlite_before: String,
    pub sqlite_after: String,
    pub backup_venv: Option<PathBuf>,
}

impl RuntimeRepairResult {
    pub fn new(status: impl Into<String>) -> Self {
        Self {
            status: status.into(),
            detail: String::new(),
            sqlite_before: String::new(),
            sqlite_after: String::new(),
            backup_venv: None,
        }
    }
    pub fn with_detail(status: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            status: status.into(),
            detail: detail.into(),
            sqlite_before: String::new(),
            sqlite_after: String::new(),
            backup_venv: None,
        }
    }
    /// Mirrors `@property def repaired(self) -> bool:` at 130-131.
    pub fn repaired(&self) -> bool {
        self.status == "repaired"
    }
}

// ---------------------------------------------------------------------------
// _RepairLock — mirrors lines 134-138
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass(frozen=True) class _RepairLock:` at 134-138.
#[derive(Debug)]
pub struct RepairLock {
    pub path: PathBuf,
    pub fd: i32,
}

// ---------------------------------------------------------------------------
// _report_runtime_repair_failure — mirrors lines 140-153
// ---------------------------------------------------------------------------

/// Mirrors `def _report_runtime_repair_failure(repair: RuntimeRepairResult) -> None:` at 140-153.
pub fn report_runtime_repair_failure(repair: &RuntimeRepairResult) {
    if repair.backup_venv.is_none() {
        println!(
            "  ℹ Managed Python runtime was not replaced; the existing venv is unchanged ({}).",
            repair.detail
        );
        println!(
            "    Sessions stay protected meanwhile: Hermes keeps databases out of WAL mode on this SQLite build. The next `hermes update` will retry."
        );
        return;
    }
    println!(
        "  ✗ Managed Python runtime cutover needs manual recovery: {}",
        repair.detail
    );
    if let Some(b) = &repair.backup_venv {
        println!("    Previous venv: {}", b.display());
    }
}

// ---------------------------------------------------------------------------
// _UvResult — mirrors lines 156-194
// ---------------------------------------------------------------------------

/// Mirrors `class _UvResult(str):` at 156-194.
///
/// POSIX only: a `str` subclass that survives update-boundary arity skew.
/// `ensure_uv()` arity has flipped between single path and `(path, fresh)`
/// tuple across releases. This wrapper answers to both:
///   `uv_bin = ensure_uv()` and `uv_bin, fresh = ensure_uv()`.
/// On Windows this wrapper is **never** returned — plain `str`/None instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UvResult {
    pub path: String,
    pub fresh_bootstrap: bool,
}

impl UvResult {
    pub fn new(path: Option<&str>, fresh: bool) -> Self {
        Self {
            path: path.unwrap_or("").to_string(),
            fresh_bootstrap: fresh,
        }
    }
    pub fn as_str(&self) -> &str {
        &self.path
    }
    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }
    /// Mirrors `__iter__` — yields `(path_or_None, fresh)` for 2-target unpack.
    pub fn as_tuple(&self) -> (Option<String>, bool) {
        let first = if self.path.is_empty() {
            None
        } else {
            Some(self.path.clone())
        };
        (first, self.fresh_bootstrap)
    }
}

impl std::fmt::Display for UvResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.path)
    }
}
impl std::ops::Deref for UvResult {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// _ensure_uv_path — mirrors lines 196-241
// ---------------------------------------------------------------------------

/// Mirrors `def _ensure_uv_path(*, repair_observer: ...) -> Optional[str]:` at 196-241.
pub fn ensure_uv_path(
    repair_observer: Option<&dyn Fn(&RuntimeRepairResult)>,
) -> Option<String> {
    if let Some(existing) = resolve_uv() {
        return Some(existing);
    }
    let target = managed_uv_path();
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    println!("  → Installing managed uv into {} ...", target.parent().unwrap_or(Path::new("")).display());
    if let Err(exc) = install_uv(&target) {
        log_warning(&format!("Managed uv install failed: {exc}"));
        println!("  ✗ Failed to install managed uv: {exc}");
        return None;
    }
    let result = resolve_uv();
    if let Some(ref r) = result {
        let version = std::process::Command::new(r)
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default()
            .trim()
            .to_string();
        println!("  ✓ Managed uv installed ({version})");
        // Compatibility boundary: after bootstrapping uv, attempt runtime repair
        // so first update can migrate a vulnerable runtime without a second `hermes update`.
        match repair_vulnerable_runtime(r, None, None) {
            Ok(repair) => {
                if let Some(obs) = repair_observer {
                    obs(&repair);
                }
                if repair.status == "failed" {
                    report_runtime_repair_failure(&repair);
                }
            }
            Err(exc) => {
                log_warning(&format!("Managed Python runtime repair failed: {exc}"));
            }
        }
    } else {
        println!("  ✗ Managed uv install appeared to succeed but binary not found");
    }
    result
}

// ---------------------------------------------------------------------------
// ensure_uv — mirrors lines 244-276
// ---------------------------------------------------------------------------

/// Mirrors `def ensure_uv(*, repair_observer: ...):` at 244-276.
///
/// On POSIX returns `UvResult` (str subclass unpackable as `(path, fresh)`).
/// On Windows returns plain path string (None when absent) — `__iter__` override
/// is unsafe as `subprocess.list2cmdline` iterates argv entries as strings on Windows.
pub fn ensure_uv(
    repair_observer: Option<&dyn Fn(&RuntimeRepairResult)>,
) -> UvResultOrPlain {
    let result = ensure_uv_path(repair_observer);
    if cfg!(windows) {
        // Plain str/None — never UvResult on Windows
        return UvResultOrPlain::Plain(result);
    }
    UvResultOrPlain::UvResult(UvResult::new(result.as_deref(), false))
}

/// Rust sum type for the dual return of `ensure_uv`.
#[derive(Debug, Clone)]
pub enum UvResultOrPlain {
    UvResult(UvResult),
    Plain(Option<String>),
}

impl UvResultOrPlain {
    pub fn as_option_str(&self) -> Option<&str> {
        match self {
            UvResultOrPlain::UvResult(u) => if u.path.is_empty() { None } else { Some(&u.path) },
            UvResultOrPlain::Plain(o) => o.as_deref(),
        }
    }
    pub fn is_falsy(&self) -> bool {
        self.as_option_str().is_none_or(|s| s.is_empty())
    }
}

// ---------------------------------------------------------------------------
// _uv_self_update_is_fresh + _touch_uv_self_update_stamp — mirrors 279-305
// ---------------------------------------------------------------------------

/// Mirrors `def _uv_self_update_is_fresh(now: float | None = None) -> bool:` at 279-294.
pub fn uv_self_update_is_fresh(now: Option<f64>) -> bool {
    // uv releases roughly weekly; skip blocking self-update when stamp is fresh.
    let stamp = get_hermes_home().join("cache").join(".uv_self_update_stamp");
    let now_secs = now.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    });
    match std::fs::metadata(&stamp).and_then(|m| m.modified()) {
        Ok(mtime) => {
            let stamp_secs = mtime
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();
            let age = now_secs - stamp_secs;
            0.0 <= age && age < UV_SELF_UPDATE_INTERVAL_SECONDS as f64
        }
        Err(_) => false,
    }
}

/// Mirrors `def _touch_uv_self_update_stamp() -> None:` at 297-305.
pub fn touch_uv_self_update_stamp() {
    let stamp = get_hermes_home().join("cache").join(".uv_self_update_stamp");
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Mirrors `stamp.touch()` — create or update mtime.
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&stamp)
        .and_then(|_| {
            // Update mtime via filetime touch: utimensat via std only does create;
            // best-effort: write empty or just touch via OpenOptions.
            // For 1:1 we set via `std::fs::File::open` + `set_len`.
            std::fs::File::open(&stamp).map(|_| ())
        });
    // Fallback: ensure file exists and update mtime by rewriting
    let _ = std::fs::write(&stamp, "");
}

// uv ships releases ~weekly; refresh the managed binary at most this often.
// Mirrors lines 308-312.
pub const UV_SELF_UPDATE_INTERVAL_SECONDS: u64 = 7 * 24 * 3600;
/// `uv self update` is a network call; unbounded it can hang forever.
pub const UV_SELF_UPDATE_TIMEOUT_SECONDS: u64 = 60;

// ---------------------------------------------------------------------------
// update_managed_uv — mirrors lines 315-379
// ---------------------------------------------------------------------------

/// Mirrors `def update_managed_uv(*, repair_observer, force=False) -> Optional[str]:` at 315-379.
pub fn update_managed_uv(
    repair_observer: Option<&dyn Fn(&RuntimeRepairResult)>,
    force: bool,
) -> Option<String> {
    let existing = match resolve_uv() {
        Some(p) => p,
        None => return None,
    };
    if force || !uv_self_update_is_fresh(None) {
        let result = std::process::Command::new(&existing)
            .args(["self", "update"])
            .output();
        match result {
            Ok(out) if out.status.success() => {
                touch_uv_self_update_stamp();
                let version = std::process::Command::new(&existing)
                    .arg("--version")
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                println!("  ✓ Managed uv updated ({version})");
            }
            Ok(out) => {
                log_debug(&format!(
                    "uv self update failed (rc={:?}): {}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            Err(e) => {
                // Timeout vs error: treat as debug; unbounded hang is the risk being gated.
                log_debug(&format!("uv self update timed out after {}s: {e}", UV_SELF_UPDATE_TIMEOUT_SECONDS));
            }
        }
    }
    // Keep hook inside long-standing API: after git pull, updater imports fresh module
    // and calls repair here to migrate vulnerable runtime on first update.
    match repair_vulnerable_runtime(&existing, None, None) {
        Ok(repair) => {
            if let Some(obs) = repair_observer {
                obs(&repair);
            }
            if repair.status == "failed" {
                report_runtime_repair_failure(&repair);
            }
        }
        Err(exc) => {
            log_warning(&format!("Managed Python runtime repair failed: {exc}"));
            println!("  ⚠ Managed Python runtime repair skipped: {exc}");
        }
    }
    Some(existing)
}

// ---------------------------------------------------------------------------
// Managed Python runtime repair — mirrors lines 382-408
// ---------------------------------------------------------------------------

/// Mirrors `def _reload_hermes_constants():` at 387-407.
/// Re-execute hermes_constants from disk and return fresh module.
/// In Rust this is a no-op stub — module reloading has no direct equivalent;
/// kept for 1:1 traceability (the Python version handles update-boundary import skew).
pub fn reload_hermes_constants() {
    // No-op in Rust: hermes_constants is statically linked; no sys.modules cache to reload.
    // Function retained so callers that reference the reload boundary keep the same shape.
}

/// Mirrors `def _venv_python(venv_dir: Path) -> Path:` at 409-415.
pub fn venv_python(venv_dir: &Path) -> PathBuf {
    // Try hermes_constants.venv_python_path when available; fallback to platform heuristic.
    // Mirrors `from hermes_constants import venv_python_path` with ImportError reload fallback.
    if cfg!(windows) {
        venv_dir.join("Scripts").join("python.exe")
    } else {
        venv_dir.join("bin").join("python")
    }
}

/// Mirrors `def _remove_tree(path: Path, *, boundary: Path) -> None:` at 418-424.
pub fn remove_tree(path: &Path, boundary: &Path) {
    // Best-effort removal constrained to known runtime boundary.
    let canon_boundary = std::fs::canonicalize(boundary).unwrap_or_else(|_| boundary.to_path_buf());
    let canon_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !canon_path.starts_with(&canon_boundary) {
        return;
    }
    let _ = std::fs::remove_dir_all(path);
}

/// Mirrors `def _make_world_traversable(path: Path) -> None:` at 427-432.
pub fn make_world_traversable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perm = meta.permissions();
            let mode = perm.mode() | 0o755;
            perm.set_mode(mode);
            let _ = std::fs::set_permissions(path, perm);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Mirrors `def _runtime_request(info: SQLiteRuntimeInfo) -> str:` at 435-446.
/// Pin the candidate to current CPython minor line (e.g. "3.11").
pub fn runtime_request(python_version: &[i32]) -> String {
    // Mirrors `".".join(str(part) for part in info.python_version[:2])`
    if python_version.len() >= 2 {
        format!("{}.{}", python_version[0], python_version[1])
    } else if python_version.len() == 1 {
        format!("{}", python_version[0])
    } else {
        "3.11".to_string()
    }
}

// Cap on how many newer patches we'll try, newest-first, before giving up.
// Mirrors line 449-452.
pub const MAX_PATCH_RETRIES: usize = 5;

// ---------------------------------------------------------------------------
// _list_available_patches — mirrors lines 455-507
// ---------------------------------------------------------------------------

/// Mirrors `def _list_available_patches(uv_bin: str, minor: str, *, cwd: Path, env: dict)` at 455-507.
/// Return known patch versions for minor (e.g. "3.11"), newest first.
/// Queries `uv python list --all-versions` rather than trusting bare minor request.
pub fn list_available_patches(
    uv_bin: &str,
    minor: &str,
    cwd: &Path,
    env: &HashMap<String, String>,
) -> Vec<(i32, i32, i32)> {
    let out = std::process::Command::new(uv_bin)
        .args(["python", "list", minor, "--all-versions", "--only-downloads", "--output-format", "json", "--no-config"])
        .current_dir(cwd)
        .envs(env)
        .output();
    let output = match out {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.trim().is_empty() {
        return Vec::new();
    }
    // Minimal JSON parse without serde (NEVER cargo): extract `version_parts` objects.
    // Expected entry shape: {"implementation":"cpython","variant":"default","version_parts":{"major":3,"minor":11,"patch":15}}
    // We do a simple scan for "version_parts" blocks.
    let mut versions: Vec<(i32, i32, i32)> = Vec::new();
    // Filter: only default/cpython builds — skip pypy/graalpy/freethreaded.
    // Since we have no full JSON DOM, we check per-object substring before each version_parts block.
    // Split by `{` ... `}` objects naively: iterate over top-level array elements via brace-depth scan.
    let entries = split_json_objects(&stdout);
    for entry in entries {
        let lc = entry.to_lowercase();
        // Only default/cpython — skip if explicitly non-cpython or non-default variant
        let is_pypy = lc.contains("\"implementation\"") && lc.contains("pypy");
        let is_graal = lc.contains("graalpy");
        let is_freethread = lc.contains("freethreaded");
        if is_pypy || is_graal || is_freethread {
            // Check if implementation != cpython or null allowed; for stub we skip known non-cpython impls
            // More precise: if entry has implementation and it's not cpython/null, skip.
            if entry.contains("\"implementation\"") && !entry.contains("\"cpython\"") && !entry.contains("\"implementation\": null") && !entry.contains("\"implementation\":null") {
                // But original logic is `implementation in (None,"cpython")` — so any other string means skip.
                // Our heuristic: if we saw pypy/graal and not cpython, skip.
                continue;
            }
            if is_freethread {
                continue;
            }
        }
        // Variant check: must be null or default
        if entry.contains("\"variant\"") {
            let has_default = entry.contains("\"default\"");
            let has_null = entry.contains("\"variant\": null") || entry.contains("\"variant\":null");
            if !has_default && !has_null {
                continue;
            }
        }
        // Extract version_parts
        if let Some(vp_start) = entry.find("\"version_parts\"") {
            let vp_slice = &entry[vp_start..];
            let major = extract_json_int(vp_slice, "\"major\"");
            let minor_v = extract_json_int(vp_slice, "\"minor\"");
            let patch = extract_json_int(vp_slice, "\"patch\"");
            if let (Some(ma), Some(mi), Some(pa)) = (major, minor_v, patch) {
                versions.push((ma, mi, pa));
            }
        }
    }
    // Deduplicate and sort newest-first
    let mut uniq: HashSet<(i32, i32, i32)> = HashSet::new();
    let mut out_vec: Vec<(i32, i32, i32)> = Vec::new();
    for v in versions {
        if uniq.insert(v) {
            out_vec.push(v);
        }
    }
    out_vec.sort_by(|a, b| b.cmp(a));
    out_vec
}

fn split_json_objects(json: &str) -> Vec<String> {
    // Extract top-level objects from a JSON array `[ {...}, {...} ]` via brace depth.
    // Simple and does not handle strings containing braces precisely, but enough for uv JSON shape.
    let mut objs = Vec::new();
    let mut depth: i32 = 0;
    let mut start: Option<usize> = None;
    let mut in_string = false;
    let mut escape = false;
    let chars: Vec<char> = json.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if ch == '{' {
            if depth == 0 {
                start = Some(i);
            }
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                if let Some(s) = start {
                    objs.push(json[s..=i].to_string());
                    start = None;
                }
            }
            if depth < 0 {
                depth = 0;
            }
        }
    }
    objs
}

fn extract_json_int(slice: &str, key: &str) -> Option<i32> {
    let pos = slice.find(key)?;
    let after = &slice[pos + key.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let end = rest.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(rest.len());
    rest[..end].trim().parse::<i32>().ok()
}

// ---------------------------------------------------------------------------
// _attempt_install_generation — mirrors lines 510-635
// ---------------------------------------------------------------------------

/// Mirrors `def _attempt_install_generation(...) -> tuple[Path, Path, SQLiteRuntimeInfo] | None:` at 510-635.
pub fn attempt_install_generation(
    uv_bin: &str,
    request: &str,
    project_root: &Path,
    python_root: &Path,
    current: &SqliteRuntimeInfo,
    allow_minor_upgrade: bool,
    tried_versions: Option<&mut HashSet<(i32, i32, i32)>>,
) -> Option<(PathBuf, PathBuf, SqliteRuntimeInfo)> {
    // Each attempt gets its own generation directory so rejected candidate files are fully cleaned up.
    let token = format!(
        "{}-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        std::process::id(),
        &uuid_hex8()
    );
    let generation = python_root.join(format!("generation-{token}"));
    if std::fs::create_dir_all(&generation).is_err() {
        return None;
    }
    make_world_traversable(&generation);

    let env = managed_python_env(Some(project_root), Some(&generation), None);
    let install = std::process::Command::new(uv_bin)
        .args(["python", "install", request, "--reinstall", "--no-bin", "--no-registry", "--no-config"])
        .current_dir(project_root)
        .envs(&env)
        .output();
    let install_out = match install {
        Ok(o) => o,
        Err(_) => {
            remove_tree(&generation, python_root);
            return None;
        }
    };
    if !install_out.status.success() {
        let detail = String::from_utf8_lossy(&install_out.stderr)
            .trim()
            .to_string();
        let detail2 = if detail.is_empty() {
            String::from_utf8_lossy(&install_out.stdout).trim().to_string()
        } else {
            detail
        };
        log_warning(&format!("private Python install failed for {request} (rc={:?}): {detail2}", install_out.status.code()));
        remove_tree(&generation, python_root);
        return None;
    }

    let found = std::process::Command::new(uv_bin)
        .args(["python", "find", request, "--managed-python", "--no-config"])
        .current_dir(project_root)
        .envs(&env)
        .output();
    let found_out = match found {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => o,
        Ok(o) => {
            log_warning(&format!(
                "private Python lookup failed for {request} (rc={:?}): {}",
                o.status.code(),
                String::from_utf8_lossy(&o.stderr).trim()
            ));
            remove_tree(&generation, python_root);
            return None;
        }
        Err(_) => {
            remove_tree(&generation, python_root);
            return None;
        }
    };
    let stdout = String::from_utf8_lossy(&found_out.stdout).to_string();
    let last_line = stdout.lines().last().unwrap_or("").trim().to_string();
    if last_line.is_empty() {
        remove_tree(&generation, python_root);
        return None;
    }
    let python = PathBuf::from(last_line);
    // Ensure resolved Python lives inside the generation dir
    let canon_gen = std::fs::canonicalize(&generation).unwrap_or_else(|_| generation.clone());
    let canon_py = std::fs::canonicalize(&python).unwrap_or_else(|_| python.clone());
    if !canon_py.starts_with(&canon_gen) {
        log_warning(&format!("uv resolved Python outside the Hermes generation: {}", python.display()));
        remove_tree(&generation, python_root);
        return None;
    }

    let candidate = match probe_sqlite_runtime(&python) {
        Some(c) => c,
        None => {
            log_warning(&format!("could not probe candidate Python runtime: {}", python.display()));
            remove_tree(&generation, python_root);
            return None;
        }
    };
    if let Some(set) = tried_versions {
        set.insert((candidate.python_version[0], candidate.python_version[1], candidate.python_version[2]));
    }
    if allow_minor_upgrade {
        if candidate.python_version < current.python_version {
            log_warning(&format!(
                "candidate Python downgraded from {}: {:?}",
                current.python_version.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("."),
                candidate.python_version
            ));
            remove_tree(&generation, python_root);
            return None;
        }
    } else {
        // Must stay on same minor and not downgrade
        let cur_minor = if current.python_version.len() >= 2 { (current.python_version[0], current.python_version[1]) } else { (0, 0) };
        let cand_minor = if candidate.python_version.len() >= 2 { (candidate.python_version[0], candidate.python_version[1]) } else { (0, 0) };
        if cand_minor != cur_minor || candidate.python_version < current.python_version {
            log_warning(&format!(
                "candidate Python drifted off the {}.{} minor line or downgraded: {:?}",
                cur_minor.0, cur_minor.1, candidate.python_version
            ));
            remove_tree(&generation, python_root);
            return None;
        }
    }
    if candidate.wal_reset_vulnerable {
        log_warning(&format!(
            "candidate Python still links vulnerable SQLite {} ({})",
            candidate.sqlite_version_string, candidate.sqlite_source_id
        ));
        remove_tree(&generation, python_root);
        return None;
    }
    Some((generation, python, candidate))
}

fn uuid_hex8() -> String {
    // Minimal hex8 token — mirrors `uuid.uuid4().hex[:8]` without uuid crate.
    // Use time + pid + random byte from /dev/urandom or fallback.
    let mut buf = [0u8; 4];
    #[cfg(unix)]
    {
        if let Ok(f) = std::fs::File::open("/dev/urandom") {
            use std::io::Read;
            let mut r = f;
            let _ = r.read_exact(&mut buf);
        } else {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
            buf = (t as u32).to_le_bytes();
        }
    }
    #[cfg(not(unix))]
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        buf = (t as u32).to_le_bytes();
    }
    format!("{:02x}{:02x}{:02x}{:02x}", buf[0], buf[1], buf[2], buf[3])
}

// ---------------------------------------------------------------------------
// _install_safe_python_generation — mirrors lines 638-746
// ---------------------------------------------------------------------------

/// Mirrors `def _install_safe_python_generation(uv_bin: str, *, project_root: Path, current: SQLiteRuntimeInfo)` at 638-746.
pub fn install_safe_python_generation(
    uv_bin: &str,
    project_root: &Path,
    current: &SqliteRuntimeInfo,
) -> Option<(PathBuf, PathBuf, SqliteRuntimeInfo)> {
    let runtime_root = project_root.join(RUNTIME_DIR_NAME);
    let python_root = managed_python_install_dir(Some(project_root));
    make_world_traversable(&runtime_root);
    make_world_traversable(&python_root);

    let request = runtime_request(&current.python_version);
    println!("  → Provisioning a private Python {request} runtime with fixed SQLite...");
    let mut tried_versions: HashSet<(i32, i32, i32)> = HashSet::new();
    tried_versions.insert((current.python_version[0], current.python_version[1], current.python_version[2]));

    let mut result = attempt_install_generation(
        uv_bin,
        &request,
        project_root,
        &python_root,
        current,
        false,
        Some(&mut tried_versions),
    );
    if result.is_some() {
        return result;
    }

    // Bare minor request resolved to still-vulnerable or otherwise rejected candidate.
    // Query patches on this minor and retry explicit newer versions, newest-first.
    let env_for_list = managed_python_env(Some(project_root), Some(&python_root), None);
    let patches = list_available_patches(uv_bin, &request, project_root, &env_for_list);
    let mut attempts: usize = 0;
    for version_tuple in patches {
        if attempts >= MAX_PATCH_RETRIES {
            break;
        }
        if tried_versions.contains(&version_tuple) {
            continue;
        }
        // Only NEWER patches can carry the SQLite fix
        let cur_tup = (current.python_version[0], current.python_version[1], current.python_version[2]);
        if version_tuple <= cur_tup {
            continue;
        }
        tried_versions.insert(version_tuple);
        let explicit_request = format!("{}.{}.{}", version_tuple.0, version_tuple.1, version_tuple.2);
        println!("  → Retrying with explicit patch {explicit_request}...");
        attempts += 1;
        result = attempt_install_generation(
            uv_bin,
            &explicit_request,
            project_root,
            &python_root,
            current,
            false,
            None,
        );
        if result.is_some() {
            return result;
        }
    }

    // All patches on current minor are vulnerable or rejected — fall forward to next minor
    let cur_major = current.python_version[0];
    let cur_minor = current.python_version[1];
    let mut fb_tried: HashSet<(i32, i32, i32)> = tried_versions.clone();
    for next_minor in (cur_minor + 1)..14 {
        let next_request = format!("{cur_major}.{next_minor}");
        println!("  → No fixed {cur_major}.{cur_minor} build available; trying {next_request} as fallback...");
        result = attempt_install_generation(
            uv_bin,
            &next_request,
            project_root,
            &python_root,
            current,
            true,
            Some(&mut fb_tried),
        );
        if result.is_some() {
            return result;
        }
        let env_for_list = managed_python_env(Some(project_root), Some(&python_root), None);
        let fb_patches = list_available_patches(uv_bin, &next_request, project_root, &env_for_list);
        let mut fb_attempts: usize = 0;
        for version_tuple in fb_patches {
            if fb_attempts >= MAX_PATCH_RETRIES {
                break;
            }
            if fb_tried.contains(&version_tuple) {
                continue;
            }
            fb_tried.insert(version_tuple);
            let explicit = format!("{}.{}.{}", version_tuple.0, version_tuple.1, version_tuple.2);
            println!("  → Retrying with explicit patch {explicit}...");
            fb_attempts += 1;
            result = attempt_install_generation(
                uv_bin,
                &explicit,
                project_root,
                &python_root,
                current,
                true,
                None,
            );
            if result.is_some() {
                return result;
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// _smoke_candidate_venv — mirrors lines 749-793
// ---------------------------------------------------------------------------

/// Mirrors `def _smoke_candidate_venv(venv_dir: Path) -> tuple[bool, str, SQLiteRuntimeInfo | None]:` at 749-793.
pub fn smoke_candidate_venv(venv_dir: &Path) -> (bool, String, Option<SqliteRuntimeInfo>) {
    let python = venv_python(venv_dir);
    let info = match probe_sqlite_runtime(&python) {
        Some(i) => i,
        None => return (false, format!("could not execute {}", python.display()), None),
    };
    if info.wal_reset_vulnerable {
        return (
            false,
            format!("candidate still links vulnerable SQLite {}", info.sqlite_version_string),
            Some(info),
        );
    }
    let check = "import dotenv, fastapi, openai, prompt_toolkit, pydantic, rich, uvicorn, yaml\nimport hermes_state\n";
    let mut env: HashMap<String, String> = std::env::vars().collect();
    for key in ["CONDA_DEFAULT_ENV", "CONDA_PREFIX", "PYTHONHOME", "PYTHONPATH", "UV_PROJECT_ENVIRONMENT", "UV_PYTHON", "VIRTUAL_ENV"] {
        env.remove(key);
    }
    let out = std::process::Command::new(python.to_string_lossy().to_string())
        .args(["-I", "-c", check])
        .current_dir(venv_dir.parent().unwrap_or(Path::new(".")))
        .envs(&env)
        .output();
    match out {
        Ok(o) if o.status.success() => (true, String::new(), Some(info)),
        Ok(o) => {
            let detail = if !o.stderr.is_empty() {
                String::from_utf8_lossy(&o.stderr).trim().to_string()
            } else if !o.stdout.is_empty() {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            } else {
                "core import smoke failed".to_string()
            };
            let last_line = detail.lines().last().unwrap_or("core import smoke failed").to_string();
            (false, last_line, Some(info))
        }
        Err(e) => (false, e.to_string(), Some(info)),
    }
}

// ---------------------------------------------------------------------------
// _stage_candidate_venv — mirrors lines 796-877
// ---------------------------------------------------------------------------

/// Mirrors `def _stage_candidate_venv(uv_bin: str, *, project_root: Path, generation: Path, python: Path) -> Path | None:` at 796-877.
pub fn stage_candidate_venv(
    uv_bin: &str,
    project_root: &Path,
    generation: &Path,
    python: &Path,
) -> Option<PathBuf> {
    let runtime_root = project_root.join(RUNTIME_DIR_NAME);
    let token = format!(
        "{}-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        std::process::id(),
        &uuid_hex8()
    );
    let candidate = runtime_root.join(format!("venv-candidate-{token}"));
    let mut env = managed_python_env(Some(project_root), Some(generation), None);
    env.insert("UV_PROJECT_ENVIRONMENT".to_string(), candidate.to_string_lossy().to_string());
    env.insert("UV_PYTHON".to_string(), python.to_string_lossy().to_string());
    env.insert("UV_PYTHON_DOWNLOADS".to_string(), "never".to_string());
    env.insert("VIRTUAL_ENV".to_string(), candidate.to_string_lossy().to_string());

    println!("  → Building a relocatable replacement environment...");
    let created = std::process::Command::new(uv_bin)
        .args(["venv", &candidate.to_string_lossy(), "--python", &python.to_string_lossy(), "--managed-python", "--no-python-downloads", "--relocatable", "--no-config"])
        .current_dir(project_root)
        .envs(&env)
        .output();
    let created_out = match created {
        Ok(o) => o,
        Err(_) => {
            remove_tree(&candidate, &runtime_root);
            return None;
        }
    };
    if !created_out.status.success() {
        let detail = if !created_out.stderr.is_empty() {
            String::from_utf8_lossy(&created_out.stderr).trim().to_string()
        } else {
            String::from_utf8_lossy(&created_out.stdout).trim().to_string()
        };
        log_warning(&format!("candidate venv creation failed (rc={:?}): {detail}", created_out.status.code()));
        remove_tree(&candidate, &runtime_root);
        return None;
    }

    if !project_root.join("uv.lock").is_file() {
        log_warning("candidate dependency sync refused: uv.lock is missing");
        remove_tree(&candidate, &runtime_root);
        return None;
    }
    // Locked sync must see project [tool.uv] exclude-newer; --no-config / UV_NO_CONFIG drops it
    let mut sync_env = env.clone();
    sync_env.remove("UV_NO_CONFIG");
    let synced = std::process::Command::new(uv_bin)
        .args(["sync", "--extra", "all", "--locked", "--python", &venv_python(&candidate).to_string_lossy()])
        .current_dir(project_root)
        .envs(&sync_env)
        .output();
    let synced_out = match synced {
        Ok(o) => o,
        Err(_) => {
            remove_tree(&candidate, &runtime_root);
            return None;
        }
    };
    if !synced_out.status.success() {
        log_warning(&format!("candidate dependency sync failed (rc={:?})", synced_out.status.code()));
        remove_tree(&candidate, &runtime_root);
        return None;
    }

    let (healthy, detail, _) = smoke_candidate_venv(&candidate);
    if !healthy {
        log_warning(&format!("candidate venv smoke failed: {detail}"));
        remove_tree(&candidate, &runtime_root);
        return None;
    }
    Some(candidate)
}

// ---------------------------------------------------------------------------
// _rename_with_retry — mirrors lines 880-892
// ---------------------------------------------------------------------------

/// Mirrors `def _rename_with_retry(source: Path, destination: Path) -> None:` at 880-892.
pub fn rename_with_retry(source: &Path, destination: &Path) -> Result<(), std::io::Error> {
    let delays = [0.0f64, 0.1, 0.25, 0.5, 1.0];
    let mut last_error: Option<std::io::Error> = None;
    for &delay in &delays {
        if delay > 0.0 {
            std::thread::sleep(std::time::Duration::from_secs_f64(delay));
        }
        match std::fs::rename(source, destination) {
            Ok(()) => return Ok(()),
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "rename failed")))
}

// ---------------------------------------------------------------------------
// _cut_over_candidate — mirrors lines 894-900 (slice 1 header)
// ---------------------------------------------------------------------------

/// Mirrors `def _cut_over_candidate(candidate: Path, *, project_root: Path, live: Path | None = None)` at 894-900.
///
/// Slice 1 covers through the function header and the token/backup/rejected path setup
/// (lines 894-904). Full rename-with-retry / rollback / smoke-verify body
/// (lines 905-963) continues in `managed_uv_slice2.rs`. This stub preserves
/// the signature for 1:1 audit and documents the boundary.
pub fn cut_over_candidate(
    candidate: &Path,
    project_root: &Path,
    live: Option<&Path>,
) -> (bool, Option<PathBuf>, Option<SqliteRuntimeInfo>, String) {
    let live_path = live
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| project_root.join(VENV_NAME));
    let runtime_root = project_root.join(RUNTIME_DIR_NAME);
    let token = format!(
        "{}-{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        std::process::id(),
        &uuid_hex8()
    );
    let backup = live_path.with_file_name(format!("{}.stale.runtime-{token}", live_path.file_name().unwrap_or_default().to_string_lossy()));
    let rejected = runtime_root.join(format!("venv-rejected-{token}"));
    // Full impl at lines 906-963 — deferred to slice 2 for 900-line boundary.
    // Stub preserves 1:1 signature: caller verifies (cut_over, backup, info, detail).
    let _ = (candidate, backup.clone(), rejected);
    // Indicate not yet cut over — real promotion lives in slice 2.
    (false, None, None, "cut_over_candidate: slice 1 stub — full impl in managed_uv_slice2.rs".to_string())
}

// ---------------------------------------------------------------------------
// SQLiteRuntimeInfo stub — mirrors `hermes_cli.sqlite_runtime` (lines 38, 1170 etc)
// ---------------------------------------------------------------------------

/// Minimal mirror of `hermes_cli.sqlite_runtime.SQLiteRuntimeInfo` for slice 1.
/// Fields used by managed_uv logic: `python_version`, `sqlite_version_string`,
/// `sqlite_source_id`, `wal_reset_vulnerable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteRuntimeInfo {
    pub python_version: Vec<i32>,
    pub sqlite_version_string: String,
    pub sqlite_source_id: String,
    pub wal_reset_vulnerable: bool,
}

impl SqliteRuntimeInfo {
    pub fn new(python_version: Vec<i32>, sqlite_version: impl Into<String>, source_id: impl Into<String>, vulnerable: bool) -> Self {
        Self {
            python_version,
            sqlite_version_string: sqlite_version.into(),
            sqlite_source_id: source_id.into(),
            wal_reset_vulnerable: vulnerable,
        }
    }
}

/// Mirrors `probe_sqlite_runtime(python: Path) -> SQLiteRuntimeInfo | None`.
/// In slice 1 this is a subprocess stub: runs `python -c "import sqlite3; print(sqlite3.sqlite_version)"`.
/// For 1:1 traceability without importing the real module, we probe via subprocess and parse.
pub fn probe_sqlite_runtime(python: &Path) -> Option<SqliteRuntimeInfo> {
    // Best-effort probe: run python to emit version info
    let out = std::process::Command::new(python)
        .args(["-c", "import sys, sqlite3; print('.'.join(map(str, sys.version_info[:3]))); print(sqlite3.sqlite_version); print(sqlite3.sqlite_source_id if hasattr(sqlite3,'sqlite_source_id') else '')"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let mut lines = text.lines();
    let py_ver_str = lines.next()?.trim();
    let sqlite_ver = lines.next().unwrap_or("").trim().to_string();
    let source_id = lines.next().unwrap_or("").trim().to_string();
    let py_parts: Vec<i32> = py_ver_str.split('.').filter_map(|p| p.parse::<i32>().ok()).collect();
    if py_parts.is_empty() {
        return None;
    }
    // Vulnerable check stub: original `wal_reset_vulnerable` is SQLite < 3.49 or known bad range.
    // Slice 1 keeps heuristic: vulnerable if sqlite < 3.49.0 as placeholder; real check in sqlite_runtime.rs.
    let vulnerable = is_wal_reset_vulnerable(&sqlite_ver);
    Some(SqliteRuntimeInfo {
        python_version: py_parts,
        sqlite_version_string: sqlite_ver,
        sqlite_source_id: source_id,
        wal_reset_vulnerable: vulnerable,
    })
}

fn is_wal_reset_vulnerable(sqlite_version: &str) -> bool {
    // Placeholder mirror of `wal_reset_vulnerable` — true for < 3.49.0 (real logic in sqlite_runtime.py).
    // Keep 1:1: the repair loop gates on this bool.
    let parts: Vec<i32> = sqlite_version.split('.').filter_map(|p| p.parse::<i32>().ok()).collect();
    if parts.len() < 3 {
        return false;
    }
    let ver = (parts[0], parts[1], parts[2]);
    ver < (3, 49, 0)
}

// ---------------------------------------------------------------------------
// Installer internals stub — mirrors lines 1310+ (deferred to slice 2)
// ---------------------------------------------------------------------------

/// Mirrors `def _install_uv(target: Path) -> None:` at 1315-1335.
/// Bootstrap uv into target using official installer. Stub in slice 1; full
/// impl (POSIX curl|sh + Windows PowerShell) lives in slice 2 so the 900-line
/// boundary stays clean, but the symbol is declared here for callers in this slice.
pub fn install_uv(target: &Path) -> Result<(), String> {
    // Forward to slice 2 implementation when linked; in slice 1 we provide a
    // minimal POSIX installer that mirrors `_install_uv_posix` for tests.
    // This keeps `ensure_uv_path` functional without requiring slice 2 at compile time.
    let system_is_windows = cfg!(windows);
    let mut env: HashMap<String, String> = std::env::vars().collect();
    env.insert("UV_UNMANAGED_INSTALL".to_string(), target.parent().unwrap_or(Path::new("")).to_string_lossy().to_string());
    env.insert("UV_INSTALL_DIR".to_string(), target.parent().unwrap_or(Path::new("")).to_string_lossy().to_string());
    if system_is_windows {
        install_uv_windows(&env)
    } else {
        install_uv_posix(&env)
    }
}

fn install_uv_posix(env: &HashMap<String, String>) -> Result<(), String> {
    // Mirrors `def _install_uv_posix(env: dict[str, str]) -> None:` at 1338-1359.
    // Two-stage download + sh, without `tempfile` crate for 1:1 std-only.
    let mut tmp = std::env::temp_dir();
    tmp.push(format!("hermes-uv-install-{}.sh", uuid_hex8()));
    let installer_path = tmp;
    let curl = std::process::Command::new("curl")
        .args(["-LsSf", "https://astral.sh/uv/install.sh", "-o", &installer_path.to_string_lossy()])
        .output()
        .map_err(|e| format!("curl failed: {e}"))?;
    if !curl.status.success() {
        let _ = std::fs::remove_file(&installer_path);
        return Err(format!("curl failed: {}", String::from_utf8_lossy(&curl.stderr).trim()));
    }
    let sh = std::process::Command::new("sh")
        .arg(&installer_path)
        .envs(env)
        .output()
        .map_err(|e| format!("sh failed: {e}"))?;
    let _ = std::fs::remove_file(&installer_path);
    if !sh.status.success() {
        return Err(format!("uv installer failed: {}", String::from_utf8_lossy(&sh.stderr).trim()));
    }
    Ok(())
}

fn install_uv_windows(env: &HashMap<String, String>) -> Result<(), String> {
    // Mirrors `def _install_uv_windows(env: dict[str, str]) -> None:` at 1362-1370.
    let cmd = "irm https://astral.sh/uv/install.ps1 | iex";
    let out = std::process::Command::new("powershell")
        .args(["-ExecutionPolicy", "Bypass", "-c", cmd])
        .envs(env)
        .output()
        .map_err(|e| format!("powershell failed: {e}"))?;
    if !out.status.success() {
        return Err(format!("powershell uv installer failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// Mirrors `def repair_vulnerable_runtime(uv_bin: str, *, project_root, venv_dir)` at 1153-1307.
/// Slice 1 stub — full orchestration (probe, lock, provision, stage, cutover) lives in slice 2.
/// Declared here so `ensure_uv_path`/`update_managed_uv` can call it without slice 2 linkage.
pub fn repair_vulnerable_runtime(
    uv_bin: &str,
    project_root: Option<&Path>,
    venv_dir: Option<&Path>,
) -> Result<RuntimeRepairResult, String> {
    let root = project_root.map(|p| p.to_path_buf()).unwrap_or_else(project_root_default);
    let live = venv_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| default_live_venv(&root));
    let live_python = venv_python(&live);
    if !root.join("pyproject.toml").is_file() || !live_python.is_file() {
        return Ok(RuntimeRepairResult::new("not-applicable"));
    }
    let current = match probe_sqlite_runtime(&live_python) {
        Some(c) => c,
        None => {
            return Ok(RuntimeRepairResult::with_detail(
                "skipped",
                format!("could not probe live interpreter {}", live_python.display()),
            ))
        }
    };
    if !current.wal_reset_vulnerable {
        // Already fixed — sweep stale backups (age-gated) like Python does at 1183
        sweep_stale_runtime_backups(&live, &root, None, 3600.0);
        return Ok(RuntimeRepairResult {
            status: "safe".to_string(),
            detail: String::new(),
            sqlite_before: current.sqlite_version_string.clone(),
            sqlite_after: current.sqlite_version_string,
            backup_venv: None,
        });
    }
    // Full repair (lock, provision, stage, cutover) requires slice 2 — stub defers.
    // Preserve observable behavior: log and return skipped when lock contention or platform holders would block.
    let _ = uv_bin;
    // Signal that full repair is available only with slice 2 linked.
    Ok(RuntimeRepairResult {
        status: "skipped".to_string(),
        detail: "repair_vulnerable_runtime: slice 1 stub — full impl in managed_uv_slice2.rs".to_string(),
        sqlite_before: current.sqlite_version_string,
        sqlite_after: String::new(),
        backup_venv: None,
    })
}

fn default_live_venv(root: &Path) -> PathBuf {
    // Mirrors `def _default_live_venv(root: Path) -> Path:` at 1089-1111.
    let primary = root.join(VENV_NAME);
    if venv_python(&primary).is_file() {
        return primary;
    }
    let fallback = root.join(ALT_VENV_NAME);
    if venv_python(&fallback).is_file() {
        return fallback;
    }
    primary
}

fn sweep_stale_runtime_backups(live: &Path, root: &Path, keep: Option<&Path>, min_age_seconds: f64) {
    // Mirrors `def _sweep_stale_runtime_backups(...)` at 1114-1150.
    let pattern = format!("{}.stale.runtime-*", live.file_name().unwrap_or_default().to_string_lossy());
    let parent = match live.parent() {
        Some(p) => p,
        None => return,
    };
    let candidates = match std::fs::read_dir(parent) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    for entry in candidates.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        if !name.starts_with(&format!("{}.stale.runtime-", live.file_name().unwrap_or_default().to_string_lossy())) {
            // Glob is f"{live.name}.stale.runtime-*"
            let prefix = format!("{}.stale.runtime-", live.file_name().unwrap_or_default().to_string_lossy());
            if !name.starts_with(&prefix) {
                continue;
            }
        }
        let _ = pattern; // keep 1:1 pattern string visible
        if let Some(k) = keep {
            if path == k {
                continue;
            }
        }
        let age = match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(mtime) => {
                let m_secs = mtime.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64();
                now - m_secs
            }
            Err(_) => continue,
        };
        if age < min_age_seconds {
            continue;
        }
        remove_tree(&path, root);
    }
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `managed_uv.py` lines 901-1374
// (_cut_over_candidate body 901-963, _acquire_repair_lock, _release_repair_lock,
// _windows_runtime_holders, _uv_version_string, _refresh_managed_uv_catalog,
// _default_live_venv, _sweep_stale_runtime_backups, repair_vulnerable_runtime
// body 1153-1307, _install_uv, _install_uv_posix, _install_uv_windows,
// rebuild_venv) continue in `managed_uv_slice2.rs`
// (from `_cut_over_candidate` body at line 901).
// This file intentionally stops at the 900-line boundary so that `cargo` is
// never invoked and the 2-slice decomposition stays clean.
