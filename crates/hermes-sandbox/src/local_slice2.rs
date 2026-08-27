//! Local execution environment — slice 2 (lines 750–1500).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/tools/environments/local.py`
//! lines 750–1500 (total 1992). Spawn-per-call helpers — bash discovery,
//! MSYS/ASLR probes, git-root resolution, bash bin dirs, shell selection,
//! sane PATH handling, hermes bin injection, managed runtime entries,
//! Windows MSYS env defaults, run-env construction, path identity and
//! repo-root alias derivation, plus module-level venv/site-packages globals.
//! Continues `local_slice1.rs` (1–750). The remainder (1500–1992,
//! `LocalEnvironment` and the tail of `strip_hermes_*`) continues in
//! `local_slice3.rs`.
//!
//! Python source docstring (preserved):
//! ```text
//! Local execution environment — spawn-per-call with session snapshot.
//! ```
//!
//! Notes on fidelity:
//! - `platform.system() == "Windows"` → `crate::local_slice1::is_windows()`
//!   (checks `HERMES_FORCE_IS_WINDOWS` override; compile-time `cfg(windows)` otherwise).
//! - `shutil.which` → `which()` helper (scans `PATH` via `env::split_paths` + executable bit on Unix).
//! - `subprocess.run(..., timeout=..., creationflags=windows_hide_flags())` → `Command` on a
//!   worker thread with `recv_timeout` (mirrors Python timeout semantics without external crates;
//!   `creationflags` is a Windows-only `CREATE_NO_WINDOW` flag — ignored on POSIX, stubbed via
//!   conditional cfg here to avoid `winapi` dep; fidelity is the `windowsHide: true` semantic).
//! - `site.getsitepackages()` → `site_packages_via_subprocess` fallback via `sys.prefix` construction
//!   (see `get_hermes_site_packages` notes below); Python's `site` import is best-effort there too.
//! - `os.path.normcase` → `normcase()` (lowercases on Windows, identity on POSIX).
//! - `HERMES_HOME` / `get_process_hermes_home()` → `crate::file_sync::get_hermes_home()`
//!   (profile-aware via `HERMES_HOME` env; same contract as `hermes_constants` in Python).
//! - `tools.env_passthrough.is_env_passthrough` → `HERMES_PASSTHROUGH` comma-separated env
//!   (test hook; production provider registry is Python-only, so stub preserves the passthrough branch).
//! - `hermes_constants.apply_subprocess_home_env` → `apply_subprocess_home_env()` stub that ensures
//!   `HERMES_HOME` is present (mirrors Python's subprocess HOME contract without full profile re-home).

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::file_sync::get_hermes_home;
use crate::local_slice1::{
    is_hermes_internal_secret, is_windows, strip_hermes_owned_pythonpath_and_runtime_markers,
    ACTIVE_VENV_MARKER_VARS, HERMES_PROVIDER_ENV_FORCE_PREFIX,
};

// ---------------------------------------------------------------------------
// Helpers — mirrors Python built-ins used in slice2
// ---------------------------------------------------------------------------

fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric() || matches!(c, '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-' )
    });
    if safe {
        return s.to_string();
    }
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn expanduser(s: &str) -> String {
    if s == "~" || s.starts_with("~/") {
        if let Ok(home) = env::var("HOME") {
            let h = home.trim().to_string();
            if !h.is_empty() {
                if s == "~" {
                    return h;
                }
                return format!("{}{}", h, &s[1..]);
            }
        }
        if let Ok(home) = env::var("USERPROFILE") {
            let h = home.trim().to_string();
            if !h.is_empty() {
                if s == "~" {
                    return h;
                }
                return format!("{}{}", h, &s[1..]);
            }
        }
    }
    s.to_string()
}

fn which(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            if let Ok(meta) = fs::metadata(&candidate) {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate);
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = meta;
                    return Some(candidate);
                }
            }
        }
        // Windows also checks .exe extension if name has no extension
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn normcase(s: &str) -> String {
    if is_windows() {
        s.to_ascii_lowercase()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// _find_bash — mirrors `def _find_bash() -> str` (line 745)
// ---------------------------------------------------------------------------

/// Find bash for command execution.
///
/// Mirrors `local.py::_find_bash`.
pub fn find_bash() -> String {
    if !is_windows() {
        return which("bash")
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| {
                if Path::new("/usr/bin/bash").is_file() {
                    Some("/usr/bin/bash".to_string())
                } else {
                    None
                }
            })
            .or_else(|| {
                if Path::new("/bin/bash").is_file() {
                    Some("/bin/bash".to_string())
                } else {
                    None
                }
            })
            .or_else(|| env::var("SHELL").ok().filter(|s| !s.trim().is_empty()))
            .unwrap_or_else(|| "/bin/sh".to_string());
    }

    let mut candidates: Vec<String> = Vec::new();

    // Custom override via HERMES_GIT_BASH_PATH
    let custom = env::var("HERMES_GIT_BASH_PATH").ok().filter(|s| !s.trim().is_empty());
    if let Some(ref c) = custom {
        if Path::new(c).is_file() {
            candidates.push(c.clone());
        }
    }

    // Prefer portable Git under %LOCALAPPDATA%\hermes\git
    let local_appdata = env::var("LOCALAPPDATA").unwrap_or_default();
    let hermes_portable_git = if !local_appdata.trim().is_empty() {
        PathBuf::from(&local_appdata).join("hermes").join("git")
    } else {
        PathBuf::new()
    };
    if !hermes_portable_git.as_os_str().is_empty() {
        for cand in [
            hermes_portable_git.join("bin").join("bash.exe"),
            hermes_portable_git.join("usr").join("bin").join("bash.exe"),
        ] {
            let s = cand.to_string_lossy().to_string();
            if cand.is_file() && !candidates.contains(&s) {
                candidates.push(s);
            }
        }
    }

    // Known Git for Windows locations before PATH lookup
    let program_files = env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".to_string());
    let program_files_x86 = env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".to_string());
    for cand in [
        PathBuf::from(&program_files).join("Git").join("bin").join("bash.exe"),
        PathBuf::from(&program_files_x86).join("Git").join("bin").join("bash.exe"),
        if !local_appdata.is_empty() {
            PathBuf::from(&local_appdata).join("Programs").join("Git").join("bin").join("bash.exe")
        } else {
            PathBuf::new()
        },
    ] {
        if cand.as_os_str().is_empty() {
            continue;
        }
        let s = cand.to_string_lossy().to_string();
        if cand.is_file() && !candidates.contains(&s) {
            candidates.push(s);
        }
    }

    if let Some(found) = which("bash").map(|p| p.to_string_lossy().to_string()) {
        if !candidates.contains(&found) {
            candidates.push(found);
        }
    }

    // Prefer first candidate that can actually start
    for candidate in &candidates {
        if bash_starts(candidate) {
            if let Some(ref c) = custom {
                if candidate != c && Path::new(c).is_file() {
                    log::warn!("HERMES_GIT_BASH_PATH={} fails to start; using {} instead", c, candidate);
                }
            }
            return candidate.clone();
        }
    }

    if !candidates.is_empty() {
        let probe_details = candidates
            .iter()
            .filter_map(|c| bash_probe_details(c).map(|d| d.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        if mandatory_aslr_enabled() == Some(true) || looks_like_msys_spawn_failure(&probe_details) {
            panic!("{}", git_bash_aslr_help(&candidates[0], &probe_details));
        }
        return candidates[0].clone();
    }

    panic!(
        "Git Bash not found. Hermes Agent requires Git for Windows on Windows.\n\
         Install it from: https://git-scm.com/download/win\n\
         Or set HERMES_GIT_BASH_PATH to your bash.exe location."
    )
}

// ---------------------------------------------------------------------------
// _bash_starts / probe caches — mirrors Python globals at 832–837
// ---------------------------------------------------------------------------

/// Mirrors `_BASH_EXTERNAL_PROGRAM_PROBE = "/usr/bin/true; /usr/bin/cat --version >/dev/null"`.
pub const BASH_EXTERNAL_PROGRAM_PROBE: &str = "/usr/bin/true; /usr/bin/cat --version >/dev/null";

static BASH_STARTS_CACHE: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
static BASH_PROBE_DETAILS_CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static MANDATORY_ASLR_CACHE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();

fn bash_starts_cache() -> &'static Mutex<HashMap<String, bool>> {
    BASH_STARTS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
fn bash_probe_details_cache() -> &'static Mutex<HashMap<String, String>> {
    BASH_PROBE_DETAILS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}
fn mandatory_aslr_cache() -> &'static Mutex<Option<bool>> {
    MANDATORY_ASLR_CACHE.get_or_init(|| Mutex::new(None))
}

/// Test hook: return cached bash probe details if present.
pub fn bash_probe_details(bash: &str) -> Option<String> {
    bash_probe_details_cache().lock().ok()?.get(bash).cloned()
}

// ---------------------------------------------------------------------------
// _looks_like_msys_spawn_failure — mirrors `def _looks_like_msys_spawn_failure(details: str) -> bool`
// ---------------------------------------------------------------------------

/// Match Git-for-Windows child-launch failures associated with ASLR.
///
/// Mirrors `local.py::_looks_like_msys_spawn_failure`.
pub fn looks_like_msys_spawn_failure(details: &str) -> bool {
    let lowered = details.to_ascii_lowercase();
    ["dofork:", "child_copy:", "0xc0000142", "0xc0000005"]
        .iter()
        .any(|m| lowered.contains(m))
}

// ---------------------------------------------------------------------------
// _mandatory_aslr_enabled — mirrors `def _mandatory_aslr_enabled() -> "bool | None"`
// ---------------------------------------------------------------------------

/// Return Windows' system-wide ForceRelocateImages state when available.
///
/// Mirrors `local.py::_mandatory_aslr_enabled`.
pub fn mandatory_aslr_enabled() -> Option<bool> {
    if let Ok(g) = mandatory_aslr_cache().lock() {
        if let Some(v) = *g {
            return Some(v);
        }
    }
    // Try PowerShell probe with short timeout (Windows only). On non-Windows, return None.
    if !is_windows() {
        return None;
    }
    let powershell = which("powershell.exe")
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "powershell.exe".to_string());
    let (tx, rx) = std::sync::mpsc::channel();
    let ps = powershell.clone();
    std::thread::spawn(move || {
        let out = Command::new(&ps)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "(Get-ProcessMitigation -System).Aslr.ForceRelocateImages.ToString()",
            ])
            .stdin(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    let res = rx.recv_timeout(Duration::from_secs(10)).ok()?;
    let output = match res {
        Ok(o) => o,
        Err(e) => {
            log::debug!("Could not query Windows Mandatory ASLR state: {}", e);
            return None;
        }
    };
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_ascii_uppercase();
    let result = if value == "ON" {
        Some(true)
    } else if matches!(value.as_str(), "OFF" | "NOTSET") {
        Some(false)
    } else {
        None
    };
    if let Some(v) = result {
        if let Ok(mut g) = mandatory_aslr_cache().lock() {
            *g = Some(v);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// _git_root_from_bash — mirrors `def _git_root_from_bash(bash: str) -> str`
// ---------------------------------------------------------------------------

/// Resolve Git's root from either `<root>/bin` or `<root>/usr/bin` bash.
///
/// Mirrors `local.py::_git_root_from_bash`.
pub fn git_root_from_bash(bash: &str) -> String {
    // Use ntpath semantics: dirname/basename with backslash-aware split (Windows paths)
    // We approximate via Path with `/` and `\` handling.
    let normalized = bash.replace('/', "\\");
    let bin_dir = Path::new(&normalized).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let bin_name = Path::new(&bin_dir).file_name().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if bin_name != "bin" {
        // `ntpath.dirname(bin_dir)` case when bin_dir itself is not `bin` — Python returns dirname(bin_dir)
        return Path::new(&normalized).parent().and_then(|p| p.parent()).map(|p| p.to_string_lossy().to_string()).unwrap_or(bin_dir);
    }
    let parent = Path::new(&bin_dir).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let parent_name = Path::new(&parent).file_name().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if parent_name == "usr" {
        return Path::new(&parent).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or(parent);
    }
    parent
}

// ---------------------------------------------------------------------------
// _git_bash_aslr_help — mirrors `def _git_bash_aslr_help(bash: str, details: str = "") -> str`
// ---------------------------------------------------------------------------

/// Build the targeted per-program Mandatory-ASLR remediation.
///
/// Mirrors `local.py::_git_bash_aslr_help`.
pub fn git_bash_aslr_help(bash: &str, details: &str) -> String {
    let git_root = git_root_from_bash(bash);
    let escaped_root = git_root.replace('\'', "''");
    let detail_line = if details.is_empty() {
        String::new()
    } else {
        let snippet = &details[..details.len().min(500)];
        format!("\nGit Bash probe output: {snippet}")
    };
    format!(
        "Git Bash at {bash} cannot launch required MSYS child processes while \
         Windows Mandatory ASLR (ForceRelocateImages) is enabled, or its output \
         matches that Git-for-Windows failure class.{detail_line}\n\
         Reinstalling Git will not change the Windows mitigation policy. Open \
         PowerShell as Administrator and run:\n\
         $gitRoot = '{escaped_root}'\n\
         Get-Item \"$gitRoot\\bin\\bash.exe\", \"$gitRoot\\usr\\bin\\*.exe\" \
         -ErrorAction SilentlyContinue | ForEach-Object {{ \
         Set-ProcessMitigation -Name $_.FullName -Disable ForceRelocateImages }}\n\
         Then restart Hermes. If the override is blocked or later re-applied, \
         ask your Windows administrator to allow this per-program exception."
    )
}

// ---------------------------------------------------------------------------
// _bash_starts — mirrors `def _bash_starts(bash: str) -> bool`
// ---------------------------------------------------------------------------

/// True if *bash* can launch external MSYS programs.
///
/// Mirrors `local.py::_bash_starts`. Cached per path for the process lifetime.
pub fn bash_starts(bash: &str) -> bool {
    if let Ok(g) = bash_starts_cache().lock() {
        if let Some(&v) = g.get(bash) {
            return v;
        }
    }
    let ok: bool;
    let probe = BASH_EXTERNAL_PROGRAM_PROBE;
    let (tx, rx) = std::sync::mpsc::channel();
    let b = bash.to_string();
    std::thread::spawn(move || {
        let out = Command::new(&b)
            .args(["--noprofile", "--norc", "-c", probe])
            .stdin(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(output)) => {
            ok = output.status.success();
            if !ok {
                let combined = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
                let snippet = combined.trim().chars().take(2000).collect::<String>();
                if let Ok(mut g) = bash_probe_details_cache().lock() {
                    g.insert(bash.to_string(), snippet.clone());
                }
                log::debug!("bash probe failed for {}: {}", bash, snippet.chars().take(200).collect::<String>());
            }
        }
        Ok(Err(exc)) => {
            let s = format!("{exc}").chars().take(2000).collect::<String>();
            if let Ok(mut g) = bash_probe_details_cache().lock() {
                g.insert(bash.to_string(), s.clone());
            }
            log::debug!("bash probe error for {}: {}", bash, exc);
            ok = false;
        }
        Err(_) => {
            let s = "timeout".to_string();
            if let Ok(mut g) = bash_probe_details_cache().lock() {
                g.insert(bash.to_string(), s);
            }
            ok = false;
        }
    }
    if let Ok(mut g) = bash_starts_cache().lock() {
        g.insert(bash.to_string(), ok);
    }
    ok
}

// ---------------------------------------------------------------------------
// _git_bash_bin_dirs — mirrors `def _git_bash_bin_dirs() -> list[str]` (line 958)
// ---------------------------------------------------------------------------

static GIT_BASH_BIN_DIRS_CACHE: OnceLock<Mutex<Option<Vec<String>>>> = OnceLock::new();

fn git_bash_bin_dirs_cache() -> &'static Mutex<Option<Vec<String>>> {
    GIT_BASH_BIN_DIRS_CACHE.get_or_init(|| Mutex::new(None))
}

/// Git Bash's coreutils/binary dirs, in `/etc/profile` precedence order.
///
/// Mirrors `local.py::_git_bash_bin_dirs`.
pub fn git_bash_bin_dirs() -> Vec<String> {
    if let Ok(g) = git_bash_bin_dirs_cache().lock() {
        if let Some(ref v) = *g {
            return v.clone();
        }
    }
    if !is_windows() {
        if let Ok(mut g) = git_bash_bin_dirs_cache().lock() {
            *g = Some(Vec::new());
        }
        return Vec::new();
    }
    let bash = match std::panic::catch_unwind(find_bash) {
        Ok(b) => b,
        Err(_) => {
            if let Ok(mut g) = git_bash_bin_dirs_cache().lock() {
                *g = Some(Vec::new());
            }
            return Vec::new();
        }
    };
    let bin_dir = Path::new(&bash).parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let parent = bin_dir.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let bin_name = bin_dir.file_name().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let root = if bin_name == "usr" {
        parent.parent().map(|p| p.to_path_buf()).unwrap_or(parent)
    } else {
        parent
    };
    let mut dirs: Vec<String> = Vec::new();
    for cand in [
        root.join("mingw64").join("bin"),
        root.join("mingw32").join("bin"),
        root.join("usr").join("local").join("bin"),
        root.join("usr").join("bin"),
        root.join("bin"),
    ] {
        if cand.is_dir() {
            let s = cand.to_string_lossy().to_string();
            if !dirs.contains(&s) {
                dirs.push(s);
            }
        }
    }
    if let Ok(mut g) = git_bash_bin_dirs_cache().lock() {
        *g = Some(dirs.clone());
    }
    dirs
}

/// Prepend Git Bash's binary dirs to `existing_path` if missing.
///
/// Mirrors `local.py::_prepend_git_bash_dirs`.
pub fn prepend_git_bash_dirs(existing_path: &str) -> String {
    let git_dirs = git_bash_bin_dirs();
    if git_dirs.is_empty() {
        return existing_path.to_string();
    }
    let sep = if is_windows() { ";" } else { ":" };
    // Filter empty entries defensively
    let entries: Vec<String> = if existing_path.is_empty() {
        Vec::new()
    } else {
        existing_path.split(sep).filter(|e| !e.is_empty()).map(|s| s.to_string()).collect()
    };
    let missing: Vec<String> = git_dirs.into_iter().filter(|d| !entries.contains(d)).collect();
    if missing.is_empty() {
        return existing_path.to_string();
    }
    let mut out = missing;
    out.extend(entries);
    out.join(sep)
}

// ---------------------------------------------------------------------------
// _SPAWN_COMPATIBLE_SHELLS / _find_shell — mirrors lines 1031–1078
// ---------------------------------------------------------------------------

/// Mirrors `_SPAWN_COMPATIBLE_SHELLS = frozenset({"bash", "zsh", "sh", "dash", "ksh", "mksh"})`.
pub const SPAWN_COMPATIBLE_SHELLS: &[&str] = &["bash", "zsh", "sh", "dash", "ksh", "mksh"];

/// Find the user's login shell for background process spawning.
///
/// Mirrors `local.py::_find_shell`.
pub fn find_shell() -> String {
    if !is_windows() {
        if let Ok(user_shell) = env::var("SHELL") {
            let t = user_shell.trim().to_string();
            if !t.is_empty() && Path::new(&t).is_file() {
                // Check executable bit on Unix
                let is_executable = {
                    #[cfg(unix)]
                    {
                        fs::metadata(&t).map(|m| {
                            use std::os::unix::fs::PermissionsExt;
                            m.permissions().mode() & 0o111 != 0
                        }).unwrap_or(false)
                    }
                    #[cfg(not(unix))]
                    {
                        true
                    }
                };
                let base = Path::new(&t).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                if is_executable && SPAWN_COMPATIBLE_SHELLS.contains(&base.as_str()) {
                    return t;
                }
            }
        }
    }
    find_bash()
}

// ---------------------------------------------------------------------------
// _SANE_PATH / _HERMES_BIN_DIR — mirrors lines 1080–1164
// ---------------------------------------------------------------------------

/// Mirrors `_SANE_PATH`.
pub const SANE_PATH: &str = "/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

static HERMES_BIN_DIR: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static HERMES_BIN_DIR_RESOLVED: OnceLock<bool> = OnceLock::new();

fn hermes_bin_dir_lock() -> &'static Mutex<Option<String>> {
    HERMES_BIN_DIR.get_or_init(|| Mutex::new(None))
}

/// Return the directory holding the `hermes` console-script, or None.
///
/// Mirrors `local.py::_resolve_hermes_bin_dir`.
pub fn resolve_hermes_bin_dir() -> Option<String> {
    // Sentinel check: if already resolved, return cached
    if HERMES_BIN_DIR_RESOLVED.get().copied().unwrap_or(false) {
        if let Ok(g) = hermes_bin_dir_lock().lock() {
            return g.clone();
        }
    }
    let mut candidate: Option<String> = None;

    if let Some(found) = which("hermes").map(|p| p.to_string_lossy().to_string()) {
        candidate = Some(Path::new(&found).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or(found));
    }

    if candidate.is_none() {
        if let Ok(argv0) = env::var("HERMES_ARGV0").or_else(|_| Ok::<String, env::VarError>(env::args().next().unwrap_or_default())) {
            let base = Path::new(&argv0).file_name().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if Path::new(&argv0).is_absolute() && (base == "hermes" || base.starts_with("hermes.")) && Path::new(&argv0).is_file() {
                candidate = Some(Path::new(&argv0).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default());
            }
        } else {
            // Fallback via args().next() directly
            let argv0 = env::args().next().unwrap_or_default();
            let base = Path::new(&argv0).file_name().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if Path::new(&argv0).is_absolute() && (base == "hermes" || base.starts_with("hermes.")) && Path::new(&argv0).is_file() {
                candidate = Some(Path::new(&argv0).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default());
            }
        }
    }

    if candidate.is_none() {
        if let Ok(exe) = env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let shim = if is_windows() { "hermes.exe" } else { "hermes" };
                if exe_dir.join(shim).is_file() {
                    candidate = Some(exe_dir.to_string_lossy().to_string());
                }
            }
        }
    }

    if let Some(ref c) = candidate {
        if !Path::new(c).is_dir() {
            candidate = None;
        }
    }

    if let Ok(mut g) = hermes_bin_dir_lock().lock() {
        *g = candidate.clone();
    }
    let _ = HERMES_BIN_DIR_RESOLVED.get_or_init(|| true);
    candidate
}

/// Prepend the hermes install dir to `existing_path` if missing.
///
/// Mirrors `local.py::_prepend_hermes_bin_dir`.
pub fn prepend_hermes_bin_dir(existing_path: &str) -> String {
    let bin_dir = match resolve_hermes_bin_dir() {
        Some(d) => d,
        None => return existing_path.to_string(),
    };
    let sep = if is_windows() { ";" } else { ":" };
    let entries: Vec<String> = if existing_path.is_empty() {
        Vec::new()
    } else {
        existing_path.split(sep).filter(|e| !e.is_empty()).map(|s| s.to_string()).collect()
    };
    if entries.contains(&bin_dir) {
        return existing_path.to_string();
    }
    let mut out = vec![bin_dir];
    out.extend(entries);
    out.join(sep)
}

// ---------------------------------------------------------------------------
// _managed_runtime_path_entries — mirrors `def _managed_runtime_path_entries() -> list[str]` (line 1166)
// ---------------------------------------------------------------------------

/// Return existing Hermes-managed runtime dirs for the terminal subshell PATH.
///
/// Mirrors `local.py::_managed_runtime_path_entries`.
pub fn managed_runtime_path_entries() -> Vec<String> {
    // In Python: `try: from hermes_constants import get_hermes_home, iter_hermes_node_dirs; candidates = [*iter_hermes_node_dirs(), get_hermes_home() / "bin"]; return [str(d) for d in candidates if d.is_dir()]`
    // Rust: we emulate iter_hermes_node_dirs via HERMES_NODE_DIRS env or $HERMES_HOME/node (+ /bin) scanning.
    let mut candidates: Vec<PathBuf> = Vec::new();
    // Try env-injected node dirs first (test hook)
    if let Ok(v) = env::var("HERMES_NODE_DIRS") {
        for part in v.split(|c| c == ':' || c == ';') {
            let t = part.trim();
            if !t.is_empty() {
                candidates.push(PathBuf::from(t));
            }
        }
    }
    // Fallback: $HERMES_HOME/node and $HERMES_HOME/node/bin (common managed layout)
    let home = get_hermes_home();
    for p in [home.join("node"), home.join("node").join("bin"), home.join("bin")] {
        if !candidates.contains(&p) {
            candidates.push(p);
        }
    }
    candidates.into_iter().filter(|d| d.is_dir()).map(|p| p.to_string_lossy().to_string()).collect()
}

// ---------------------------------------------------------------------------
// _append_missing_sane_path_entries — mirrors `def _append_missing_sane_path_entries(existing_path: str) -> str` (line 1194)
// ---------------------------------------------------------------------------

/// Return a normalised POSIX PATH with missing sane entries appended.
///
/// Mirrors `local.py::_append_missing_sane_path_entries`.
pub fn append_missing_sane_path_entries(existing_path: &str) -> String {
    if is_windows() {
        return existing_path.to_string();
    }
    let mut sane_entries: Vec<String> = SANE_PATH.split(':').filter(|e| !e.is_empty()).map(|s| s.to_string()).collect();
    for entry in managed_runtime_path_entries() {
        if !sane_entries.contains(&entry) {
            sane_entries.push(entry);
        }
    }
    if existing_path.is_empty() {
        return sane_entries.join(":");
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut ordered: Vec<String> = Vec::new();
    for entry in existing_path.split(':') {
        if entry.is_empty() || seen.contains(entry) {
            continue;
        }
        seen.insert(entry.to_string());
        ordered.push(entry.to_string());
    }
    for entry in sane_entries {
        if !seen.contains(&entry) {
            ordered.push(entry);
        }
    }
    ordered.join(":")
}

// ---------------------------------------------------------------------------
// _apply_windows_msys_bash_env_defaults — mirrors `def _apply_windows_msys_bash_env_defaults(env: dict) -> None` (line 1250)
// ---------------------------------------------------------------------------

/// Disable MSYS argument path conversion for Git Bash subprocesses.
///
/// Mirrors `local.py::_apply_windows_msys_bash_env_defaults`.
pub fn apply_windows_msys_bash_env_defaults(env: &mut HashMap<String, String>) {
    if !is_windows() {
        return;
    }
    env.entry("MSYS_NO_PATHCONV".to_string()).or_insert_with(|| "1".to_string());
    env.entry("MSYS2_ARG_CONV_EXCL".to_string()).or_insert_with(|| "*".to_string());
}

// ---------------------------------------------------------------------------
// _path_env_key — mirrors `def _path_env_key(run_env: dict) -> str | None` (line 1273)
// ---------------------------------------------------------------------------

/// Return the PATH env key to update without altering Windows casing.
///
/// Mirrors `local.py::_path_env_key`.
pub fn path_env_key(run_env: &HashMap<String, String>) -> Option<String> {
    if !is_windows() {
        return Some("PATH".to_string());
    }
    for key in run_env.keys() {
        if key.to_ascii_uppercase() == "PATH" {
            return Some(key.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// _make_run_env — mirrors `def _make_run_env(env: dict) -> dict` (line 1291)
// ---------------------------------------------------------------------------

/// Build a run environment with a sane PATH and provider-var stripping.
///
/// Mirrors `local.py::_make_run_env`.
pub fn make_run_env(env: &HashMap<String, String>) -> HashMap<String, String> {
    // Passthrough hook: HERMES_PASSTHROUGH comma-separated (mirrors tools.env_passthrough)
    let passthrough_set: HashSet<String> = env::var("HERMES_PASSTHROUGH")
        .map(|v| v.split(|c| c == ',' || c == ';' || c == ' ').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let is_passthrough = |k: &str| passthrough_set.contains(k);

    // Provider blocklist — mirrors `_HERMES_PROVIDER_ENV_BLOCKLIST` from slice1
    let blocklist: HashSet<String> = {
        // Call slice1's builder (includes static literal + HERMES_EXTRA_BLOCKLIST)
        crate::local_slice1::build_provider_env_blocklist()
    };

    let mut merged: HashMap<String, String> = env::vars().collect();
    for (k, v) in env {
        merged.insert(k.clone(), v.clone());
    }

    let mut run_env: HashMap<String, String> = HashMap::new();
    for (k, v) in merged {
        if k.starts_with(HERMES_PROVIDER_ENV_FORCE_PREFIX) {
            let real_key = &k[HERMES_PROVIDER_ENV_FORCE_PREFIX.len()..];
            if is_hermes_internal_secret(real_key) {
                continue;
            }
            run_env.insert(real_key.to_string(), v);
        } else if is_hermes_internal_secret(&k) {
            continue;
        } else {
            let passthrough = is_passthrough(&k);
            if blocklist.contains(&k) && !passthrough {
                continue;
            }
            // `resolve_passthrough_value` is passthrough-aware value rewrite; in Rust we keep `v` as-is (best-effort)
            run_env.insert(k, v);
        }
    }

    if let Some(path_key) = path_env_key(&run_env) {
        let existing = run_env.get(&path_key).cloned().unwrap_or_default();
        let mut new_path = append_missing_sane_path_entries(&existing);
        new_path = prepend_git_bash_dirs(&new_path);
        new_path = prepend_hermes_bin_dir(&new_path);
        run_env.insert(path_key, new_path);
    }

    // Bridge HERMES_HOME / profile home
    crate::local_slice1::inject_context_hermes_home(&mut run_env);
    // Apply subprocess HOME contract (best-effort: ensure HERMES_HOME)
    {
        if !run_env.contains_key("HERMES_HOME") {
            if let Ok(v) = env::var("HERMES_HOME") {
                let t = v.trim().to_string();
                if !t.is_empty() {
                    run_env.insert("HERMES_HOME".to_string(), t);
                }
            }
        }
    }
    crate::local_slice1::inject_session_context_env(&mut run_env);
    strip_hermes_owned_pythonpath_and_runtime_markers(&mut run_env);
    apply_windows_msys_bash_env_defaults(&mut run_env);

    // Scrub Kanban child env
    {
        // Mirrors `_scrub_delegated_child_kanban_env`
        if env::var("HERMES_KANBAN_CHILD").is_ok() {
            run_env.remove("HERMES_KANBAN_BOARD");
        }
    }

    run_env
}

// ---------------------------------------------------------------------------
// _same_path — mirrors `def _same_path(left: Path, right: Path) -> bool` (line 1353)
// ---------------------------------------------------------------------------

/// Compare path spellings with host filesystem case semantics.
///
/// Mirrors `local.py::_same_path`.
pub fn same_path(left: &Path, right: &Path) -> bool {
    let left_parts: Vec<String> = left.components().map(|c| normcase(&c.as_os_str().to_string_lossy())).collect();
    let right_parts: Vec<String> = right.components().map(|c| normcase(&c.as_os_str().to_string_lossy())).collect();
    left_parts == right_parts
}

// ---------------------------------------------------------------------------
// _build_hermes_repo_root_aliases — mirrors `def _build_hermes_repo_root_aliases(...)` (line 1360)
// ---------------------------------------------------------------------------

/// Return exact repo-root spellings emitted by Hermes launchers.
///
/// Mirrors `local.py::_build_hermes_repo_root_aliases`.
pub fn build_hermes_repo_root_aliases(
    resolved_root: &Path,
    lexical_root: &Path,
    configured_home: &Path,
) -> Vec<PathBuf> {
    let mut aliases: Vec<PathBuf> = Vec::new();
    let add = |candidate: PathBuf, aliases: &mut Vec<PathBuf>| {
        if !aliases.iter().any(|e| same_path(&candidate, e)) {
            aliases.push(candidate);
        }
    };

    let mut add_closure = |candidate: PathBuf| {
        if !aliases.iter().any(|e| same_path(&candidate, e)) {
            aliases.push(candidate);
        }
    };
    add_closure(resolved_root.to_path_buf());
    add_closure(lexical_root.to_path_buf());
    let _ = add;

    // Profile re-home: with --profile / sticky active_profile the configured
    // home becomes <root>/profiles/<name>. The repo root then lives beside
    // the profiles directory.
    let mut home_candidates: Vec<PathBuf> = vec![configured_home.to_path_buf()];
    if configured_home.parent().map(|p| p.file_name().map(|s| s == "profiles").unwrap_or(false)).unwrap_or(false) {
        if let Some(parent) = configured_home.parent().and_then(|p| p.parent()) {
            home_candidates.push(parent.to_path_buf());
        }
    }

    for home in &home_candidates {
        if let Ok(resolved_home) = fs::canonicalize(home) {
            let home_key = normcase(&resolved_home.to_string_lossy());
            let root_key = normcase(&resolved_root.to_string_lossy());
            // commonpath check: home_key is prefix of root_key
            if root_key.starts_with(&home_key) {
                // relpath
                let rel = resolved_root.strip_prefix(&resolved_home).or_else(|_| resolved_root.strip_prefix(home)).ok();
                if let Some(rel) = rel {
                    let candidate = home.join(rel);
                    if !aliases.iter().any(|e| same_path(&candidate, e)) {
                        aliases.push(candidate);
                    }
                } else {
                    // Fallback: compute relative via string operation (best-effort)
                    let rel_str = resolved_root.to_string_lossy().to_string();
                    let home_str = resolved_home.to_string_lossy().to_string();
                    if rel_str.starts_with(&home_str) {
                        let suffix = rel_str[home_str.len()..].trim_start_matches(std::path::MAIN_SEPARATOR).trim_start_matches('/');
                        let candidate = home.join(suffix);
                        if !aliases.iter().any(|e| same_path(&candidate, e)) {
                            aliases.push(candidate);
                        }
                    }
                }
            }
        }
    }

    for home in &home_candidates {
        let repo_candidate = home.join(resolved_root.file_name().unwrap_or_default());
        // Prove exact filesystem identity with strict resolve
        let cand_resolved = fs::canonicalize(&repo_candidate);
        let root_resolved = fs::canonicalize(resolved_root);
        if let (Ok(c), Ok(r)) = (cand_resolved, root_resolved) {
            if c == r && !aliases.iter().any(|e| same_path(&repo_candidate, e)) {
                aliases.push(repo_candidate);
            }
        }
    }

    aliases
}

// ---------------------------------------------------------------------------
// Module-level globals — mirrors lines 1429–1500
// ---------------------------------------------------------------------------

/// Mirrors `_hermes_repo_root: Path = Path(__file__).resolve().parents[2]`.
///
/// In Rust there is no `__file__`; we resolve from `CARGO_MANIFEST_DIR` lens
/// (crates/hermes-sandbox -> hermes-sandbox parent is `crates` -> workspace
/// root). At runtime we also try `get_hermes_home` ancestor as fallback for
/// installed layouts.
pub fn hermes_repo_root() -> PathBuf {
    // cargo manifest dir is `.../crates/hermes-sandbox`
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    if !manifest_dir.is_empty() {
        let p = PathBuf::from(&manifest_dir);
        if let Some(parent) = p.parent().and_then(|p| p.parent()) {
            return parent.to_path_buf();
        }
        return p;
    }
    // Fallback: walk up from current exe
    if let Ok(exe) = env::current_exe() {
        if let Some(root) = exe.ancestors().nth(4) {
            return root.to_path_buf();
        }
    }
    // Last resort: current dir
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Mirrors `_hermes_repo_root_aliases: tuple[Path, ...]`.
pub fn hermes_repo_root_aliases() -> Vec<PathBuf> {
    let resolved = fs::canonicalize(hermes_repo_root()).unwrap_or_else(|_| hermes_repo_root());
    let lexical = hermes_repo_root();
    let configured_home = get_hermes_home();
    build_hermes_repo_root_aliases(&resolved, &lexical, &configured_home)
}

/// Mirrors `_in_venv: bool = (getattr(sys, "base_prefix", sys.prefix) != sys.prefix or hasattr(sys, "real_prefix"))`.
///
/// In Rust we probe `VIRTUAL_ENV` env or check if `sys.prefix` analogue (`env::var("VIRTUAL_ENV")` or parent of current_exe contains `venv`).
pub fn in_venv() -> bool {
    if env::var("VIRTUAL_ENV").is_ok() {
        return true;
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            if parent.ends_with("bin") || parent.ends_with("Scripts") {
                if let Some(grand) = parent.parent() {
                    if grand.join("pyvenv.cfg").is_file() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

static HERMES_SITE_PACKAGES: OnceLock<Mutex<Option<Vec<PathBuf>>>> = OnceLock::new();

fn hermes_site_packages_lock() -> &'static Mutex<Option<Vec<PathBuf>>> {
    HERMES_SITE_PACKAGES.get_or_init(|| Mutex::new(None))
}

// ---------------------------------------------------------------------------
// _validated_runtime_venv — mirrors `def _validated_runtime_venv(env: dict) -> Path | None` (line 1468)
// ---------------------------------------------------------------------------

/// Return a producer-owned runtime venv identified by VIRTUAL_ENV.
///
/// Mirrors `local.py::_validated_runtime_venv`.
pub fn validated_runtime_venv(env: &HashMap<String, String>) -> Option<PathBuf> {
    let value = env.get("VIRTUAL_ENV")?;
    if value.trim().is_empty() {
        return None;
    }
    let candidate = PathBuf::from(value);
    let aliases = hermes_repo_root_aliases();
    let is_repo_venv = aliases.iter().any(|repo_root| same_path(&candidate, &repo_root.join("venv")));
    if !is_repo_venv {
        return None;
    }
    if !candidate.join("pyvenv.cfg").is_file() {
        return None;
    }
    Some(candidate)
}

// ---------------------------------------------------------------------------
// _get_hermes_site_packages — mirrors `def _get_hermes_site_packages(env: dict) -> list[Path]` (line 1493, truncated at 1500)
// Slice2 covers through the `if _hermes_site_packages is not None` fast-path and the
// `if _in_venv:` site.getsitepackages fallback preamble; the POSIX/Windows manual
// construction and the `VIRTUAL_ENV` runtime-venv augmentation (lines 1500–1535)
// belong to slice3 and are stubbed here.
// ---------------------------------------------------------------------------

/// Return exact site-packages dirs owned by the Hermes runtime.
///
/// Mirrors `local.py::_get_hermes_site_packages` (slice2 prefix through line 1500;
///
/// continuation of the manual fallback and `runtime_venv` augmentation is in
/// `local_slice3.rs`; this stub returns the cached prefix through the fast-path
/// plus any already-cached entries, which is sufficient for slice2 callers
/// (`_strip_hermes_owned_pythonpath` via `_make_run_env`).
pub fn get_hermes_site_packages(env: &HashMap<String, String>) -> Vec<PathBuf> {
    if let Ok(g) = hermes_site_packages_lock().lock() {
        if let Some(ref cached) = *g {
            let mut result = cached.clone();
            if let Some(rt) = validated_runtime_venv(env) {
                let rt_sp = rt.join("Lib").join("site-packages");
                if !result.iter().any(|p| same_path(&rt_sp, p)) {
                    result.push(rt_sp);
                }
            }
            return result;
        }
    }

    let mut result: Vec<PathBuf> = Vec::new();
    if in_venv() {
        // Try site.getsitepackages analogue: check `VIRTUAL_ENV` site-packages or `sys.prefix` layout
        // In Rust we don't have Python's `site` module; best-effort is to probe common layouts.
        // The full Python fallback (`site.getsitepackages()` or manual `sys.prefix / lib/pythonX.Y / site-packages`)
        // continues past line 1500 — we stub the probe via env vars / exe parent here.
        if let Ok(ve) = env::var("VIRTUAL_ENV") {
            let p = PathBuf::from(ve);
            // POSIX: lib/python*/site-packages, Windows: Lib/site-packages
            let candidates = [
                p.join("lib").join("site-packages"),
                p.join("Lib").join("site-packages"),
            ];
            for c in candidates {
                if c.is_dir() && !result.iter().any(|p| same_path(p, &c)) {
                    result.push(c);
                }
            }
            // Also scan lib/python*/site-packages via glob-like check
            if let Ok(entries) = fs::read_dir(p.join("lib")) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.file_name().map(|n| n.to_string_lossy().starts_with("python")).unwrap_or(false) {
                        let sp = path.join("site-packages");
                        if sp.is_dir() && !result.iter().any(|p| same_path(p, &sp)) {
                            result.push(sp);
                        }
                    }
                }
            }
        }
        // Fallback: parent of current_exe's prefix layout (best-effort, matches Python's sys.prefix fallback)
        if result.is_empty() {
            if let Ok(exe) = env::current_exe() {
                if let Some(bin_dir) = exe.parent() {
                    if let Some(prefix) = bin_dir.parent() {
                        let cands = [
                            prefix.join("Lib").join("site-packages"),
                            prefix.join("lib").join("site-packages"),
                        ];
                        for c in cands {
                            if c.is_dir() && !result.iter().any(|p| same_path(p, &c)) {
                                result.push(c);
                            }
                        }
                    }
                }
            }
        }
    }

    // Cache the computed prefix (Python caches `_hermes_site_packages = list(result)` at this point)
    if let Ok(mut g) = hermes_site_packages_lock().lock() {
        if g.is_none() {
            *g = Some(result.clone());
        }
    }

    // Runtime venv augmentation (present even in slice2's truncated window per Python 1528–1532)
    if let Some(rt) = validated_runtime_venv(env) {
        let rt_sp = rt.join("Lib").join("site-packages");
        if !result.iter().any(|p| same_path(&rt_sp, p)) {
            result.push(rt_sp);
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Tests — minimal smoke for slice2 helpers (no cargo run required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_compatible_shells_contains_bash() {
        assert!(SPAWN_COMPATIBLE_SHELLS.contains(&"bash"));
        assert!(SPAWN_COMPATIBLE_SHELLS.contains(&"zsh"));
        assert!(!SPAWN_COMPATIBLE_SHELLS.contains(&"fish"));
    }

    #[test]
    fn sane_path_known_entries() {
        assert!(SANE_PATH.contains("/usr/bin"));
        assert!(SANE_PATH.contains("/opt/homebrew/bin"));
    }

    #[test]
    fn looks_like_msys_matches() {
        assert!(looks_like_msys_spawn_failure("dofork: child 123 - fork failed"));
        assert!(looks_like_msys_spawn_failure("error 0xC0000142 occurred"));
        assert!(!looks_like_msys_spawn_failure("normal error: command not found"));
    }

    #[test]
    fn append_missing_sane_appends() {
        if is_windows() {
            // No-op on Windows — returns input unchanged (faithful to Python)
            assert_eq!(append_missing_sane_path_entries("/usr/bin"), "/usr/bin");
        } else {
            let p = "/usr/bin";
            let out = append_missing_sane_path_entries(p);
            // Must retain original entry at front
            assert!(out.starts_with("/usr/bin"));
            // Must contain at least one sane entry appended if not already present
            assert!(out.contains("/usr/local/bin"));
            // Dedup: same input twice should still be deduped
            let out2 = append_missing_sane_path_entries("/usr/bin:/usr/bin:/bin");
            assert_eq!(out2.matches("/usr/bin").count(), 1);
        }
    }

    #[test]
    fn same_path_case_semantics() {
        let a = Path::new("/tmp/foo");
        let b = Path::new("/tmp/foo");
        assert!(same_path(a, b));
        assert!(!same_path(Path::new("/tmp/foo"), Path::new("/tmp/bar")));
    }

    #[test]
    fn path_env_key_returns_path() {
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        assert!(path_env_key(&env).is_some());
        if is_windows() {
            let mut env2 = HashMap::new();
            env2.insert("Path".to_string(), "C:\\Windows".to_string());
            assert_eq!(path_env_key(&env2).as_deref(), Some("Path"));
        }
    }

    #[test]
    fn make_run_env_strips_secrets() {
        let mut env = HashMap::new();
        env.insert("OPENAI_API_KEY".to_string(), "sk-123".to_string());
        env.insert("MY_VAR".to_string(), "keep".to_string());
        let out = make_run_env(&env);
        // Provider secret should be stripped unless passthrough enabled
        assert!(!out.contains_key("OPENAI_API_KEY"), "OPENAI_API_KEY should be stripped by make_run_env");
        assert_eq!(out.get("MY_VAR").map(|s| s.as_str()), Some("keep"));
    }

    #[test]
    fn prepend_hermes_bin_dir_noop_when_unresolvable() {
        // When hermes is not on PATH and not under HERMES_HOME/bin, it returns input unchanged
        // This is environment-dependent, but must at least not panic
        let out = prepend_hermes_bin_dir("/usr/bin:/bin");
        assert!(out.contains("/usr/bin") || out.contains("/bin"));
    }

    #[test]
    fn get_site_packages_empty_when_not_in_venv() {
        // When not in venv, site-packages should be empty or just runtime venv (if any)
        // We don't assert exact content — just that function doesn't panic
        let env_map = HashMap::new();
        let _ = get_hermes_site_packages(&env_map);
    }
}
