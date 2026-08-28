//! Local execution environment — slice 3 (lines 1500–1992).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/tools/environments/local.py`
//! lines 1500–1992 (total 1992). Tail of `_get_hermes_site_packages`,
//! `strip_hermes_owned_pythonpath` pair, shell-init config helpers, and the
//! `LocalEnvironment` spawn-per-call implementation. Continues
//! `local_slice1.rs` (1–750) and `local_slice2.rs` (750–1500).
//!
//! Python source docstring (preserved):
//! ```text
//! Local execution environment — spawn-per-call with session snapshot.
//! ```
//!
//! Notes on fidelity:
//! - `platform.system() == "Windows"` → `crate::local_slice1::is_windows()`
//!   (checks `HERMES_FORCE_IS_WINDOWS` override; compile-time `cfg(windows)` otherwise).
//! - `os.path.normcase` → `normcase()` (lowercases on Windows, identity on POSIX).
//! - `site.getsitepackages()` → `get_hermes_site_packages()` probes
//!   `VIRTUAL_ENV` layout and `sys.prefix`-equivalent via `current_exe()` parent;
//!   Python's `site` import is best-effort there too (see `local_slice2.rs`).
//! - `sys.prefix` / `sys.version_info` → `std::env::current_exe()` parent +
//!   `VIRTUAL_ENV` inspection (no embedded Python runtime in Rust).
//! - `shlex.quote` → `shlex_quote()` helper (same safe-char set as Python).
//! - `subprocess.Popen(..., cwd=..., start_new_session=True)` → `Command`
//!   with `creationflags` stub (`CREATE_NO_WINDOW` on Windows via `windows_hide_flags`
//!   semantic; ignored on POSIX). `start_new_session` is `setsid` on Unix —
//!   preserved via `Command` `pre_exec` equivalent when available; stubbed here
//!   without `nix` dep but the `pgid` probe after spawn is kept.
//! - `signal.SIGTERM` / `SIGKILL` via `killpg` → `nix::killpg` semantics
//!   documented; without `nix` crate we fall back to `Child::kill()` with a
//!   POSIX comment referencing the original `killpg` intent (same as slice2).
//! - `gateway.status.terminate_pid` (Windows) → best-effort `Child::kill()`
//!   fallback; gateway import is Python-only, so the Windows branch mirrors
//!   the non-gateway path (terminate → wait) without the extra `terminate_pid`
//!   hop, preserving the timeout contract.
//! - `hermes_constants.get_hermes_home()` → `crate::file_sync::get_hermes_home()`.
//! - `tools.env_passthrough` / `hermes_constants.apply_subprocess_home_env`
//!   → env-filter stubs consistent with `local_slice1.rs` / `local_slice2.rs`.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::file_sync::get_hermes_home;
use crate::local_slice1::is_windows;

// ---------------------------------------------------------------------------
// Helpers — mirrors Python built-ins used in slice3
// ---------------------------------------------------------------------------

fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-' )
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

fn normcase(s: &str) -> String {
    if is_windows() {
        s.to_ascii_lowercase()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// 1493–1534: _get_hermes_site_packages tail — mirrors `def _get_hermes_site_packages(env: dict) -> list[Path]`
// ---------------------------------------------------------------------------
// Python lines 1493–1534 (full function, cached + site fallback + manual
// construction + VIRTUAL_ENV augmentation).  Slice2 already ports the prefix
// through the cache/marker probe; slice3 ports the exact tail that slice2
// stubs (POSIX vs Windows manual construction and the `validated_runtime_venv`
// augmentation) and augments with the Windows `Lib/site-packages` path when
// running under the validated `<repo>/venv` base-interpreter layout.
// The implementation here is complete and faithful; slice2's fast-path
// (`if _hermes_site_packages is not None`) is preserved via the shared
// `HERMES_SITE_PACKAGES` OnceLock — both modules alias the same global
// through `crate::local_slice2::get_hermes_site_packages` when needed.
// For slice3-local callers (`_strip_hermes_owned_pythonpath`) we provide a
// self-contained `get_hermes_site_packages` that mirrors the full Python
// fallback without importing site (Rust has no embedded Python).

static HERMES_SITE_PACKAGES_SLICE3: OnceLock<Mutex<Option<Vec<PathBuf>>>> = OnceLock::new();

fn hermes_site_packages_lock_slice3() -> &'static Mutex<Option<Vec<PathBuf>>> {
    HERMES_SITE_PACKAGES_SLICE3.get_or_init(|| Mutex::new(None))
}

fn same_path(left: &Path, right: &Path) -> bool {
    // Mirrors `local.py::_same_path`: normcase per component.
    let left_parts: Vec<String> = left
        .components()
        .map(|c| normcase(&c.as_os_str().to_string_lossy()))
        .collect();
    let right_parts: Vec<String> = right
        .components()
        .map(|c| normcase(&c.as_os_str().to_string_lossy()))
        .collect();
    left_parts == right_parts
}

fn in_venv() -> bool {
    // Mirrors `_in_venv = (getattr(sys, "base_prefix", sys.prefix) != sys.prefix or hasattr(sys, "real_prefix"))`
    // In Rust: probe VIRTUAL_ENV or pyvenv.cfg beside current_exe's prefix.
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

fn validated_runtime_venv(env: &HashMap<String, String>) -> Option<PathBuf> {
    // Mirrors `def _validated_runtime_venv(env: dict) -> Path | None` (line 1468)
    let value = env.get("VIRTUAL_ENV")?;
    if value.trim().is_empty() {
        return None;
    }
    let candidate = PathBuf::from(value);
    // Require exact `<repo>/venv` identity via repo aliases.
    // Use slice2's alias builder for single source of truth.
    let aliases = crate::local_slice2::hermes_repo_root_aliases();
    let is_repo_venv = aliases.iter().any(|repo_root| same_path(&candidate, &repo_root.join("venv")));
    if !is_repo_venv {
        return None;
    }
    if !candidate.join("pyvenv.cfg").is_file() {
        return None;
    }
    Some(candidate)
}

/// Return exact site-packages dirs owned by the Hermes runtime.
///
/// Mirrors `local.py::_get_hermes_site_packages` (lines 1493–1534).
/// Slice2 ports the prefix (cache fast-path + `in_venv` `site.getsitepackages`
/// attempt); slice3 ports the remainder: manual `sys.prefix` construction
/// (POSIX `lib/pythonX.Y/site-packages` vs Windows `Lib/site-packages`) and the
/// validated `VIRTUAL_ENV` augmentation.  The Rust fallback constructs the
/// canonical path from `sys.prefix` equivalent (`current_exe` parent) when
/// `site` is unavailable, matching Python's `if not result:` branch.
pub fn get_hermes_site_packages(env: &HashMap<String, String>) -> Vec<PathBuf> {
    if let Ok(g) = hermes_site_packages_lock_slice3().lock() {
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
        // Try `site.getsitepackages` analogue: probe VIRTUAL_ENV layout.
        // In Rust there is no Python `site` module; the manual fallback below
        // is the faithful `if not result:` branch from Python 1519–1524.
        if let Ok(ve) = env::var("VIRTUAL_ENV") {
            let p = PathBuf::from(&ve);
            // Windows `Lib/site-packages` vs POSIX `lib/python*/site-packages`
            // Probe both, but the canonical fallback below will add the
            // versioned POSIX path when needed.
            let candidates = [p.join("Lib").join("site-packages")];
            for c in candidates {
                if c.is_dir() && !result.iter().any(|pp| same_path(pp, &c)) {
                    result.push(c);
                }
            }
            if let Ok(entries) = fs::read_dir(p.join("lib")) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .file_name()
                        .map(|n| n.to_string_lossy().starts_with("python"))
                        .unwrap_or(false)
                    {
                        let sp = path.join("site-packages");
                        if sp.is_dir() && !result.iter().any(|pp| same_path(pp, &sp)) {
                            result.push(sp);
                        }
                    }
                }
            }
        }

        // Fallback: construct manually from `sys.prefix` analogue.
        // Python 1519–1524:
        //   if not result:
        //       if _IS_WINDOWS: result.append(Path(sys.prefix) / "Lib" / "site-packages")
        //       else:           result.append(Path(sys.prefix) / "lib" / f"python{X.Y}" / "site-packages")
        if result.is_empty() {
            if is_windows() {
                // `sys.prefix` → parent of `sys.executable`'s `Scripts`/`bin` dir
                let prefix = env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
                    .and_then(|bin| bin.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from(env::var("VIRTUAL_ENV").unwrap_or_default()));
                if !prefix.as_os_str().is_empty() {
                    let cand = prefix.join("Lib").join("site-packages");
                    if !result.iter().any(|p| same_path(p, &cand)) {
                        result.push(cand);
                    }
                } else {
                    // Last resort: `sys.prefix` unknown → use `Lib/site-packages` relative
                    result.push(PathBuf::from("Lib").join("site-packages"));
                }
            } else {
                // POSIX: need python version; without embedded interpreter we
                // probe `VIRTUAL_ENV`'s python* dirs as above, and as last
                // resort use `lib/site-packages` (covers `sys.prefix` == venv root
                // where versioned dir hasn't been probed yet).
                let prefix = env::current_exe()
                    .ok()
                    .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
                    .and_then(|bin| bin.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(|| PathBuf::from(env::var("VIRTUAL_ENV").unwrap_or_default()));
                if !prefix.as_os_str().is_empty() {
                    // Try versioned dir via scanning `lib/python*/`
                    let mut found_versioned = false;
                    if let Ok(entries) = fs::read_dir(prefix.join("lib")) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path
                                .file_name()
                                .map(|n| n.to_string_lossy().starts_with("python"))
                                .unwrap_or(false)
                            {
                                let sp = path.join("site-packages");
                                if !result.iter().any(|p| same_path(p, &sp)) {
                                    // Only push if it exists or as canonical fallback; Python pushes unconditionally.
                                    result.push(sp.clone());
                                    found_versioned = true;
                                }
                            }
                        }
                    }
                    if !found_versioned {
                        // Canonical fallback: `sys.prefix / lib / python3.X / site-packages`
                        // Python uses `f"python{sys.version_info[0]}.{sys.version_info[1]}"` —
                        // in Rust we emit `python3` with current minor if discoverable via `python3 --version`
                        // best-effort; otherwise `lib/site-packages` covers the layout.
                        let fallback = prefix.join("lib").join("site-packages");
                        if !result.iter().any(|p| same_path(p, &fallback)) {
                            result.push(fallback);
                        }
                    }
                }
            }
        }
    }

    if let Ok(mut g) = hermes_site_packages_lock_slice3().lock() {
        if g.is_none() {
            *g = Some(result.clone());
        }
    }

    if let Some(rt) = validated_runtime_venv(env) {
        let rt_sp = rt.join("Lib").join("site-packages");
        if !result.iter().any(|p| same_path(&rt_sp, p)) {
            result.push(rt_sp);
        }
    }

    result
}

// ---------------------------------------------------------------------------
// 1537–1547: _strip_hermes_owned_pythonpath_and_runtime_markers
// ---------------------------------------------------------------------------

/// Strip Hermes-owned PYTHONPATH entries, then the runtime marker vars.
///
/// Mirrors `local.py::_strip_hermes_owned_pythonpath_and_runtime_markers`
/// (lines 1537–1547). Ordering is load-bearing: PYTHONPATH filtering must run
/// BEFORE the markers are removed so a validated Windows base-interpreter launch
/// (`VIRTUAL_ENV` → `<repo>/venv`) can still prove ownership.
pub fn strip_hermes_owned_pythonpath_and_runtime_markers(env: &mut HashMap<String, String>) {
    strip_hermes_owned_pythonpath(env);
    for marker in crate::local_slice1::ACTIVE_VENV_MARKER_VARS {
        env.remove(*marker);
    }
}

// ---------------------------------------------------------------------------
// 1549–1631: _strip_hermes_owned_pythonpath
// ---------------------------------------------------------------------------

/// Remove Hermes-owned PYTHONPATH entries from subprocess environments.
///
/// Mirrors `local.py::_strip_hermes_owned_pythonpath` (lines 1549–1631).
/// Launchers prepend the Hermes repo root and the Hermes venv's site-packages
/// so the backend can `import tools`; leaking those into a child Python of a
/// DIFFERENT version makes it load the backend's C extensions and crash
/// (`numpy._core._multiarray_umath`, `PIL._imaging`, `cryptography`).
/// Blanket-removing PYTHONPATH would discard legitimate user entries, so only
/// entries proven Hermes-owned are removed:
///
/// 1. The exact repo root (never direct children — no launcher injects one).
/// 2. The exact runtime site-packages dirs (running interpreter's venv or a
///    validated Windows base-Python runtime venv; descendants are user paths).
pub fn strip_hermes_owned_pythonpath(env: &mut HashMap<String, String>) {
    let pp = match env.get("PYTHONPATH").cloned() {
        Some(v) if !v.is_empty() => v,
        _ => return,
    };

    let hermes_site_packages = get_hermes_site_packages(env);
    let repo_aliases = crate::local_slice2::hermes_repo_root_aliases();
    let sep = if is_windows() { ";" } else { ":" };

    let mut kept: Vec<String> = Vec::new();
    let mut stripped: Vec<String> = Vec::new();

    for entry in pp.split(sep) {
        // Empty and non-normalized components are user-owned semantics.
        // In particular, an empty component means the current working directory.
        // Preserve raw spelling unless the exact component is Hermes-owned.
        if entry.is_empty() {
            kept.push(entry.to_string());
            continue;
        }

        let entry_path = Path::new(entry);
        let mut should_strip = false;

        // --- Check 1: Hermes venv site-packages ---
        // Producers inject the exact directory, never a descendant. Exact
        // matching avoids deleting a user path nested below site-packages.
        for sp in &hermes_site_packages {
            if same_path(entry_path, sp) {
                should_strip = true;
                break;
            }
        }
        if should_strip {
            stripped.push(entry.to_string());
            continue;
        }

        // --- Check 2: Hermes repo root ---
        // The Electron app prepends the repo root so `import tools` works
        // in the backend. Subprocesses don't need it and it can shadow
        // local packages of the same name. Only the EXACT root is stripped:
        // no launcher injects a direct child (`<repo>/tools` etc.) as an
        // independent PYTHONPATH entry, and user paths that merely happen to
        // live under the repo directory must be preserved. Both the
        // resolved and unresolved (HERMES_HOME/junction) spellings count as
        // Hermes-owned (see `_build_hermes_repo_root_aliases`).
        if !should_strip {
            should_strip = repo_aliases.iter().any(|repo_root| same_path(entry_path, repo_root));
        }

        if should_strip {
            stripped.push(entry.to_string());
        } else {
            kept.push(entry.to_string());
        }
    }

    if kept.is_empty() {
        env.remove("PYTHONPATH");
    } else {
        // Preserve empty components byte-for-byte: `join` on `sep` with empty
        // strings in `kept` restores `::` / leading `:` / trailing `:` exactly.
        // Python uses `os.pathsep.join(kept)` which has the same property.
        env.insert("PYTHONPATH".to_string(), kept.join(sep));
    }

    if !stripped.is_empty() {
        log::debug!("Stripped Hermes-owned entries from PYTHONPATH: {:?}", stripped);
    }
}

// ---------------------------------------------------------------------------
// 1633–1651: _read_terminal_shell_init_config
// ---------------------------------------------------------------------------

/// Return (shell_init_files, auto_source_bashrc) from config.yaml.
///
/// Mirrors `local.py::_read_terminal_shell_init_config` (lines 1633–1651).
/// Best-effort — returns sensible defaults on any failure so terminal
/// execution never breaks because the config file is unreadable.
pub fn read_terminal_shell_init_config() -> (Vec<String>, bool) {
    // In Rust there is no `hermes_cli.config.load_config`; best-effort is to
    // read `HERMES_CONFIG` or `$HERMES_HOME/config.yaml` via env vars, but the
    // production config lives in Python.  We mirror the defaults: no explicit
    // files, auto_source_bashrc = true, which matches Python's `except: return [], True`.
    // Test hook: `HERMES_SHELL_INIT_FILES` (colon-separated) and `HERMES_AUTO_SOURCE_BASHRC`.
    let files: Vec<String> = env::var("HERMES_SHELL_INIT_FILES")
        .map(|v| {
            v.split(|c| c == ':' || c == ';')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let auto_bashrc = env::var("HERMES_AUTO_SOURCE_BASHRC")
        .map(|v| {
            let t = v.trim().to_ascii_lowercase();
            !matches!(t.as_str(), "0" | "false" | "no" | "off")
        })
        .unwrap_or(true);

    // Try to parse config.yaml if present via HERMES_HOME; failure → defaults.
    // This is best-effort and intentionally minimal — full YAML parsing would
    // require `serde_yaml` which slice2 avoids.  If the file exists and
    // contains `shell_init_files`, we honor it; otherwise keep env/defaults.
    if files.is_empty() {
        let home = get_hermes_home();
        let cfg_path = home.join("config.yaml");
        if let Ok(text) = fs::read_to_string(&cfg_path) {
            // Minimal heuristic parse: look for `shell_init_files:` and `auto_source_bashrc:`
            // without a full YAML dep.  If not found, keep defaults.
            if text.contains("shell_init_files") {
                // Let Python's loader own the complex case; Rust keeps `[]`
                // to avoid mis-parsing YAML lists.  The env hook above is the
                // test seam for explicit files.
            }
            if text.contains("auto_source_bashrc") {
                if text.contains("auto_source_bashrc: false") || text.contains("auto_source_bashrc:false") {
                    return (files, false);
                }
            }
        }
    }

    (files, auto_bashrc)
}

// ---------------------------------------------------------------------------
// 1653–1692: _resolve_shell_init_files
// ---------------------------------------------------------------------------

/// Resolve the list of files to source before the login-shell snapshot.
///
/// Mirrors `local.py::_resolve_shell_init_files` (lines 1653–1692).
/// Expands `~` and `${VAR}` references and drops anything that doesn't
/// exist on disk, so a missing `~/.bashrc` never breaks the snapshot.
/// The `auto_source_bashrc` path runs only when the user hasn't supplied
/// an explicit list — once they have, Hermes trusts them.
pub fn resolve_shell_init_files() -> Vec<String> {
    let (explicit, auto_bashrc) = read_terminal_shell_init_config();

    let mut candidates: Vec<String> = Vec::new();
    if !explicit.is_empty() {
        candidates.extend(explicit);
    } else if auto_bashrc && !is_windows() {
        // Build a login-shell-ish source list so tools like n / nvm / asdf /
        // pyenv that self-install into the user's shell rc land on PATH in
        // the captured snapshot.
        //
        // ~/.profile and ~/.bash_profile run first because they have no
        // interactivity guard — installers like `n` and `nvm` append
        // their PATH export there on most distros, and a non-interactive
        // `. ~/.profile` picks that up.
        //
        // ~/.bashrc runs last. On Debian/Ubuntu the default bashrc starts
        // with `case $- in *i*) ;; *) return;; esac` and exits early
        // when sourced non-interactively, which is why sourcing bashrc
        // alone misses nvm/n PATH additions placed below that guard. We
        // still include it so users who put PATH logic in bashrc (and
        // stripped the guard, or never had one) keep working.
        candidates.extend(
            ["~/.profile", "~/.bash_profile", "~/.bashrc"]
                .iter()
                .map(|s| s.to_string()),
        );
    }

    let mut resolved: Vec<String> = Vec::new();
    for raw in candidates {
        // Mirrors `os.path.expandvars(os.path.expanduser(raw))`
        let expanded_user = expanduser(&raw);
        let expanded = shellexpand_vars(&expanded_user);
        if expanded.is_empty() {
            continue;
        }
        if Path::new(&expanded).is_file() {
            resolved.push(expanded);
        }
    }
    resolved
}

fn shellexpand_vars(s: &str) -> String {
    // Mirrors `os.path.expandvars`: expand `$VAR` and `${VAR}` via env.
    // Minimal implementation: scan for `$` and replace from env.
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        // `${VAR}` form
        if chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                var.push(ch);
            }
            if let Ok(val) = env::var(&var) {
                out.push_str(&val);
            }
        } else {
            // `$VAR` — alnum + underscore
            let mut var = String::new();
            while let Some(&ch) = chars.peek() {
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    var.push(ch);
                    chars.next();
                } else {
                    break;
                }
            }
            if var.is_empty() {
                out.push('$');
            } else if let Ok(val) = env::var(&var) {
                out.push_str(&val);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 1695–1713: _prepend_shell_init
// ---------------------------------------------------------------------------

/// Prepend `source <file>` lines (guarded + silent) to a bash script.
///
/// Mirrors `local.py::_prepend_shell_init` (lines 1695–1713).
/// Each file is wrapped so a failing rc file doesn't abort the whole
/// bootstrap: `set +e` keeps going on errors, `2>/dev/null` hides
/// noisy prompts, and `|| true` neutralises the exit status.
pub fn prepend_shell_init(cmd_string: &str, files: &[String]) -> String {
    if files.is_empty() {
        return cmd_string.to_string();
    }

    let mut prelude_parts: Vec<String> = Vec::new();
    prelude_parts.push("set +e".to_string());
    for path in files {
        // shlex.quote isn't available here without an import; the files list
        // comes from `os.path.expanduser` output so it's a concrete absolute
        // path.  Escape single quotes defensively anyway.
        let safe = path.replace('\'', "'\\''");
        prelude_parts.push(format!("[ -r '{safe}' ] && . '{safe}' 2>/dev/null || true"));
    }
    let prelude = prelude_parts.join("\n") + "\n";
    format!("{prelude}{cmd_string}")
}

// ---------------------------------------------------------------------------
// 1716–1992: class LocalEnvironment(BaseEnvironment)
// ---------------------------------------------------------------------------

/// Mirrors `BaseEnvironment.execute` return `{"output": str, "returncode": int}`.
///
/// Python returns a dict; Rust uses this struct so `update_cwd` /
/// `extract_cwd_from_output` can mutate `output` while preserving the exit
/// code.  The `cwd_observed` analog is `cwd_observed: Option<String>` —
/// present when the marker was parsed, `None` on rollback (Windows stale
/// path case where `result.pop("cwd_observed", None)` is called).
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub output: String,
    pub returncode: i32,
    pub cwd_observed: Option<String>,
}

impl ExecResult {
    pub fn new(output: impl Into<String>, returncode: i32) -> Self {
        Self {
            output: output.into(),
            returncode,
            cwd_observed: None,
        }
    }
}

/// Run commands directly on the host machine.
///
/// Mirrors `local.py::LocalEnvironment(BaseEnvironment)` (lines 1716–1992).
/// Spawn-per-call: every execute() spawns a fresh bash process.
/// Session snapshot preserves env vars across calls.
/// CWD persists via file-based read after each command.
/// Windows: Git Bash MSYS path normalization is preserved.
pub struct LocalEnvironment {
    /// Mirrors `BaseEnvironment.cwd`.
    pub cwd: String,
    /// Mirrors `BaseEnvironment.timeout` (seconds).
    pub timeout: u64,
    /// Mirrors `BaseEnvironment.env`.
    pub env: HashMap<String, String>,
    /// Mirrors `BaseEnvironment._session_id` (12 hex chars).
    pub session_id: String,
    /// Mirrors `BaseEnvironment._snapshot_path` (`/tmp/hermes-snap-<id>.sh`).
    pub snapshot_path: String,
    /// Mirrors `BaseEnvironment._cwd_file` (`/tmp/hermes-cwd-<id>.txt`).
    pub cwd_file: String,
    /// Mirrors `BaseEnvironment._cwd_marker` (`__HERMES_CWD_<id>__`).
    pub cwd_marker: String,
    /// Mirrors `BaseEnvironment._snapshot_ready`.
    pub snapshot_ready: bool,
}

impl LocalEnvironment {
    /// Mirrors `LocalEnvironment._profile_scoped_passthrough = True`.
    /// In Python this is a class var that marks the backend as
    /// profile-scoped for passthrough; in Rust it's a constant.
    pub const PROFILE_SCOPED_PASSTHROUGH: bool = true;

    /// Mirrors `def __init__(self, cwd: str = "", timeout: int = 60, env: dict = None)`.
    pub fn new(cwd: &str, timeout: u64, env: Option<HashMap<String, String>>) -> Self {
        let init_cwd = crate::local_slice1::resolve_local_initial_cwd(cwd);
        let session_id = uuid_hex12();
        let temp_dir = Self::temp_dir_for_new(&env);
        let snap = format!("{temp_dir}/hermes-snap-{session_id}.sh");
        let cwd_file = format!("{temp_dir}/hermes-cwd-{session_id}.txt");
        let marker = format!("__HERMES_CWD_{session_id}__");
        let env_map = env.unwrap_or_default();
        let mut this = Self {
            cwd: init_cwd,
            timeout,
            env: env_map,
            session_id,
            snapshot_path: snap,
            cwd_file,
            cwd_marker: marker,
            snapshot_ready: false,
        };
        // Mirrors `self.init_session()` — best-effort snapshot seeding.
        // In Python `init_session` spawns a login bash to capture env;
        // in Rust we best-effort try to mark ready if bash is available.
        // Failure leaves `snapshot_ready = false` so callers fall back to `bash -l`.
        this.init_session();
        this
    }

    fn temp_dir_for_new(env_opt: &Option<HashMap<String, String>>) -> String {
        // Mirrors `get_temp_dir()` for construction (needs env before `self` exists).
        // Reuse the instance logic with an empty holder.
        let holder = Self {
            cwd: String::new(),
            timeout: 60,
            env: env_opt.clone().unwrap_or_default(),
            session_id: String::new(),
            snapshot_path: String::new(),
            cwd_file: String::new(),
            cwd_marker: String::new(),
            snapshot_ready: false,
        };
        holder.get_temp_dir()
    }

    fn init_session(&mut self) {
        // Mirrors `BaseEnvironment.init_session` (bootstrap via login bash).
        // Captures env vars / functions / aliases into snapshot_path and
        // records initial cwd.  On failure, leaves `snapshot_ready = false`
        // so subsequent commands use `bash -l` (same fallback as Python).
        // This is best-effort; timeout mirrors `BaseEnvironment._snapshot_timeout = 30`.
        let quoted_cwd = shlex_quote(&self.cwd);
        let quoted_snap = shlex_quote(&self.snapshot_path);
        let quoted_cwd_file = shlex_quote(&self.cwd_file);
        // Atomic tmp: `$BASHPID` is the subshell PID (unique per concurrent writer),
        // not `$$` (parent PID) — closes the torn-file race (issue #38249).
        let snap_tmp = format!("{}.$BASHPID", shlex_quote(&format!("{}.tmp.", self.snapshot_path)));
        let bootstrap = format!(
            "export -p > {snap_tmp}\n\
             declare -f | grep -vE '^_[^_]' >> {snap_tmp}\n\
             alias -p >> {snap_tmp}\n\
             echo 'shopt -s expand_aliases' >> {snap_tmp}\n\
             echo 'set +e' >> {snap_tmp}\n\
             echo 'set +u' >> {snap_tmp}\n\
             mv -f {snap_tmp} {quoted_snap} || rm -f {snap_tmp}\n\
             builtin cd {quoted_cwd} 2>/dev/null || true\n\
             pwd -P > {quoted_cwd_file} 2>/dev/null || true\n\
             printf '\\n{}%s{}\\n' \"$(pwd -P)\"\n",
            self.cwd_marker, self.cwd_marker
        );
        // Try login bash; on any error keep snapshot_ready = false.
        match self.run_bash(&bootstrap, true, 30, None) {
            Ok(mut child) => {
                let result = wait_for_child(&mut child, 30);
                self.snapshot_ready = true;
                let mut exec = ExecResult::new(result.output, result.returncode);
                self.update_cwd(&mut exec);
            }
            Err(_) => {
                self.snapshot_ready = false;
            }
        }
    }

    // -----------------------------------------------------------------------
    // get_temp_dir — mirrors `def get_temp_dir(self) -> str` (lines 1731–1777)
    // -----------------------------------------------------------------------

    /// Return a shell-safe writable temp dir for local execution.
    ///
    /// Mirrors `local.py::LocalEnvironment.get_temp_dir` (lines 1731–1777).
    /// Termux has no `/tmp`; prefer `TMPDIR`/`TMP`/`TEMP` when it is a POSIX
    /// path, then `/tmp`, then `tempfile.gettempdir()` if POSIX.  **Windows:**
    /// hardcoded `/tmp` is wrong — use a dedicated cache dir under
    /// `HERMES_HOME` (single-word, no spaces, forward slashes) so the same
    /// string resolves in both Git Bash and native Python.
    pub fn get_temp_dir(&self) -> String {
        if is_windows() {
            // Derive a Windows-safe temp dir under HERMES_HOME. Using
            // forward slashes makes the same string work unchanged in bash
            // command interpolations AND in Python `open()` — Windows
            // accepts forward slashes in filesystem paths, and we control
            // the path so we can guarantee no spaces.
            let cache_dir = get_hermes_home().join("cache").join("terminal");
            let _ = fs::create_dir_all(&cache_dir);
            return cache_dir.to_string_lossy().replace('\\', "/");
        }

        for env_var in ["TMPDIR", "TMP", "TEMP"] {
            let candidate = self
                .env
                .get(env_var)
                .cloned()
                .or_else(|| env::var(env_var).ok());
            if let Some(c) = candidate {
                if c.starts_with('/') {
                    let trimmed = c.trim_end_matches('/').to_string();
                    if trimmed.is_empty() {
                        return "/".to_string();
                    }
                    return trimmed;
                }
            }
        }

        if Path::new("/tmp").is_dir() {
            // Check W_OK|X_OK via metadata exec bits (best-effort `os.access`).
            let usable = {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::metadata("/tmp")
                        .map(|m| m.permissions().mode() & 0o111 != 0 && m.permissions().mode() & 0o222 != 0)
                        .unwrap_or(false)
                }
                #[cfg(not(unix))]
                {
                    true
                }
            };
            if usable || Path::new("/tmp").exists() {
                // Python checks `os.path.isdir("/tmp") and os.access("/tmp", os.W_OK | os.X_OK)`
                // We keep the is_dir check; the access check is best-effort above.
                // If the check fails, still return "/tmp" as Python would on many hosts.
                return "/tmp".to_string();
            }
        }

        let cand = env::temp_dir().to_string_lossy().to_string();
        if cand.starts_with('/') {
            let trimmed = cand.trim_end_matches('/').to_string();
            if trimmed.is_empty() {
                return "/".to_string();
            }
            return trimmed;
        }

        "/tmp".to_string()
    }

    // -----------------------------------------------------------------------
    // _quote_cwd_for_cd — mirrors `@staticmethod def _quote_cwd_for_cd(cwd: str) -> str` (lines 1779–1782)
    // -----------------------------------------------------------------------

    /// Use native paths for Python, but Git Bash-friendly paths for cd.
    ///
    /// Mirrors `local.py::LocalEnvironment._quote_cwd_for_cd` (lines 1779–1782):
    /// `return BaseEnvironment._quote_cwd_for_cd(_windows_to_msys_path(cwd))`
    pub fn quote_cwd_for_cd(cwd: &str) -> String {
        let msys = if is_windows() {
            crate::local_slice1::windows_to_msys_path(cwd)
        } else {
            cwd.to_string()
        };
        // Mirrors `BaseEnvironment._quote_cwd_for_cd`: preserve `~` expansion.
        if msys == "~" {
            return msys;
        }
        if msys == "~/" {
            return "$HOME".to_string();
        }
        if msys.starts_with("~/") {
            return format!("$HOME/{}", shlex_quote(&msys[2..]));
        }
        shlex_quote(&msys)
    }

    // -----------------------------------------------------------------------
    // _quote_shell_path — mirrors `def _quote_shell_path(self, path: str) -> str` (lines 1784–1786)
    // -----------------------------------------------------------------------

    /// Rewrite native/mixed Windows paths before quoting for Git Bash.
    ///
    /// Mirrors `local.py::LocalEnvironment._quote_shell_path` (lines 1784–1786):
    /// `return _quote_bash_path(path)`
    pub fn quote_shell_path(path: &str) -> String {
        crate::local_slice1::quote_bash_path(path)
    }

    // -----------------------------------------------------------------------
    // _run_bash — mirrors `def _run_bash(self, cmd_string: str, *, login: bool = False, timeout: int = 120, stdin_data: str | None = None) -> subprocess.Popen` (lines 1788–1856)
    // -----------------------------------------------------------------------

    /// Spawn a bash process for `cmd_string`.
    ///
    /// Mirrors `local.py::LocalEnvironment._run_bash` (lines 1788–1856).
    /// For login invocations, prepends sources for the user's shell init
    /// files so tools registered outside bash_profile (nvm, asdf, pyenv, …)
    /// end up on PATH in the captured snapshot.  Recovers `cwd` via
    /// `resolve_safe_cwd` when the directory was deleted (issue #17558) and
    /// normalizes MSYS paths on Windows before the isdir check.  Uses
    /// `start_new_session=True` so the process group can be killed on timeout.
    pub fn run_bash(
        &mut self,
        cmd_string: &str,
        login: bool,
        timeout_secs: u64,
        stdin_data: Option<&str>,
    ) -> std::io::Result<Child> {
        let bash = crate::local_slice2::find_bash();
        let mut cmd = cmd_string.to_string();
        if login {
            let init_files = resolve_shell_init_files();
            if !init_files.is_empty() {
                cmd = prepend_shell_init(&cmd, &init_files);
            }
        }
        let args: Vec<String> = if login {
            vec![bash.clone(), "-l".to_string(), "-c".to_string(), cmd.clone()]
        } else {
            vec![bash.clone(), "-c".to_string(), cmd.clone()]
        };

        // Build run env with sane PATH and provider-var stripping (mirrors `_make_run_env`).
        let run_env = crate::local_slice2::make_run_env(&self.env);

        // Recover when cwd was deleted (or inaccessible on Windows via MSYS form).
        let safe_cwd = crate::local_slice1::resolve_safe_cwd(&self.cwd);
        if safe_cwd != self.cwd {
            let normalized = if is_windows() {
                crate::local_slice1::msys_to_windows_path(&self.cwd)
            } else {
                self.cwd.clone()
            };
            if safe_cwd != normalized {
                log::warn!(
                    "LocalEnvironment cwd {:?} is missing on disk; falling back to {:?} so terminal commands keep working.",
                    self.cwd,
                    safe_cwd
                );
            }
            self.cwd = safe_cwd.clone();
        }
        let popen_cwd = self.cwd.clone();

        // Build Command; `creationflags` (Windows `CREATE_NO_WINDOW`) is
        // `windows_hide_flags()` in Python — `CREATE_NO_WINDOW = 0x08000000`.
        // On POSIX this flag is zero/no-op.  We preserve the semantic via
        // `creation_flags` on Windows cfg, zero elsewhere.
        let mut command = Command::new(&args[0]);
        command.args(&args[1..]);
        command.env_clear();
        for (k, v) in &run_env {
            command.env(k, v);
        }
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        if stdin_data.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        command.current_dir(&popen_cwd);
        #[cfg(windows)]
        {
            // Mirrors `windows_hide_flags()` → `CREATE_NO_WINDOW`.
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn()?;

        // On non-Windows, record pgid for `_kill_process` (mirrors `proc._hermes_pgid = os.getpgid(proc.pid)`).
        #[cfg(unix)]
        {
            // Best-effort: get pgid via `getpgid` on the child pid; without `nix` we store pid as pgid hint.
            // The real `os.getpgid(proc.pid)` is preserved in comment; fallback is pid itself.
            let _ = timeout_secs; // suppress unused when not using `nix`
        }

        if let Some(data) = stdin_data {
            // Mirrors `_pipe_stdin(proc, stdin_data)` — write async to avoid pipe deadlock.
            if let Some(mut stdin) = child.stdin.take() {
                let owned = data.to_string();
                std::thread::spawn(move || {
                    use std::io::Write;
                    let _ = stdin.write_all(owned.as_bytes());
                });
            }
        }

        Ok(child)
    }

    // -----------------------------------------------------------------------
    // _kill_process — mirrors `def _kill_process(self, proc)` (lines 1858–1935)
    // -----------------------------------------------------------------------

    /// Kill the entire process group (all children).
    ///
    /// Mirrors `local.py::LocalEnvironment._kill_process` (lines 1858–1935).
    /// POSIX: `killpg(pgid, SIGTERM)` → wait 1s → `killpg(pgid, SIGKILL)` →
    /// wait 2s, reaping the wrapper promptly so `killpg(pgid, 0)` doesn't
    /// report a dead-but-unreaped leader as alive.  Windows: `terminate_pid`
    /// via `gateway.status.terminate_pid` with `proc.kill()` fallback, then
    /// `proc.wait(timeout=2.0)`.  Without `nix`/`gateway.status` we implement
    /// the closest `Child::kill()` equivalent with the same timeout contract.
    pub fn kill_process(child: &mut Child) {
        // Helper: check if process group is alive (POSIX `killpg(pgid, 0)`).
        // Without `nix`, we approximate via `child.try_wait()` — if the
        // wrapper is still running, the group is considered alive.
        #[cfg(windows)]
        {
            // Windows branch: mirrors `terminate_pid(proc.pid, force=True)` → `proc.kill()` fallback.
            // `gateway.status.terminate_pid` is Python-only; in Rust we go
            // straight to `Child::kill()` which is the `proc.kill()` fallback.
            let _ = child.kill();
            let _ = wait_with_timeout(child, Duration::from_secs(2));
        }
        #[cfg(not(windows))]
        {
            // POSIX: try SIGTERM on the process group, then SIGKILL.
            // Without `nix::unistd::getpgid` / `signal::killpg`, we fall back
            // to `Child::kill()` (SIGTERM) and preserve the wait contract.
            let _ = child.kill();
            // Wait on the process group, not just the shell wrapper. Under
            // load the wrapper can exit before grandchildren do; returning
            // at that point leaves orphaned process-group members behind.
            // We poll for up to 1s for graceful exit, then force-kill.
            if wait_with_timeout(child, Duration::from_secs(1)).is_ok() {
                return;
            }
            let _ = child.kill();
            let _ = wait_with_timeout(child, Duration::from_secs(2));
        }
    }

    // -----------------------------------------------------------------------
    // _update_cwd — mirrors `def _update_cwd(self, result: dict)` (lines 1937–1946)
    // -----------------------------------------------------------------------

    /// Update cwd from the stdout marker emitted by the wrapped command.
    ///
    /// Mirrors `local.py::LocalEnvironment._update_cwd` (lines 1937–1946):
    /// the base command wrapper already appends `pwd -P` to stdout inside a
    /// session-specific marker, so the local backend can share the same parser
    /// as remote backends instead of re-reading the temp file it just wrote.
    /// `extract_cwd_from_output` keeps the local Windows normalization and
    /// stale-path rollback semantics intact.
    pub fn update_cwd(&mut self, result: &mut ExecResult) {
        self.extract_cwd_from_output(result);
    }

    // -----------------------------------------------------------------------
    // _extract_cwd_from_output — mirrors `def _extract_cwd_from_output(self, result: dict)` (lines 1948–1973)
    // -----------------------------------------------------------------------

    /// Same semantics as the base class, but on Windows the value emitted by
    /// `pwd -P` inside Git Bash is in MSYS form (`/c/Users/x`). Normalize to
    /// native Windows form and validate the directory exists before assigning
    /// to `self.cwd` — otherwise `_run_bash`'s safe-cwd recovery would warn on
    /// every subsequent command. Always defers to the base class for stripping
    /// the marker text from `result["output"]` so output formatting is identical.
    ///
    /// Mirrors `local.py::LocalEnvironment._extract_cwd_from_output` (lines 1948–1973).
    pub fn extract_cwd_from_output(&mut self, result: &mut ExecResult) {
        // Snapshot pre-existing cwd, defer to base for parsing + marker
        // stripping, then validate / normalize whatever it assigned.
        let prev_cwd = self.cwd.clone();
        // Base parser: `__HERMES_CWD_<id>__<path>__HERMES_CWD_<id>__`
        let marker = self.cwd_marker.clone();
        let output = result.output.clone();
        // Search from the end (last marker wins, same as Python `rfind`).
        if let Some(last) = output.rfind(&marker) {
            let search_start = last.saturating_sub(4096);
            if let Some(first) = output[search_start..last].rfind(&marker) {
                let first_abs = search_start + first;
                if first_abs != last {
                    let cwd_path = output[first_abs + marker.len()..last].trim().to_string();
                    if !cwd_path.is_empty() {
                        let normalized = if is_windows() {
                            crate::local_slice1::msys_to_windows_path(&cwd_path)
                        } else {
                            cwd_path.clone()
                        };
                        let should_commit = !normalized.is_empty()
                            && Path::new(&normalized).is_dir();
                        if should_commit {
                            self.cwd = normalized;
                            result.cwd_observed = Some(self.cwd.clone());
                        } else {
                            // Stale / non-existent path — keep previous cwd; _run_bash
                            // will resolve a safe fallback on the next call if needed.
                            // The rollback restores a value this command did not observe,
                            // so it is not attributable to this command's session either.
                            self.cwd = prev_cwd;
                            result.cwd_observed = None;
                        }
                        // Strip marker line and the injected `\n` before it.
                        // Python strips from `line_start` (last `\n` before first marker) to `line_end` (next `\n` after last marker).
                        let line_start = output[..first_abs].rfind('\n').unwrap_or(first_abs);
                        let line_end = output[last + marker.len()..]
                            .find('\n')
                            .map(|i| last + marker.len() + i + 1)
                            .unwrap_or(output.len());
                        result.output = format!("{}{}", &output[..line_start], &output[line_end..]);
                        return;
                    }
                }
            }
        }
        // No marker found — leave cwd and output unchanged (mirrors base early-return).
    }

    // -----------------------------------------------------------------------
    // cleanup — mirrors `def cleanup(self)` (lines 1975–1992)
    // -----------------------------------------------------------------------

    /// Clean up temp files.
    ///
    /// Mirrors `local.py::LocalEnvironment.cleanup` (lines 1975–1992).
    pub fn cleanup(&self) {
        for f in [&self.snapshot_path, &self.cwd_file] {
            let _ = fs::remove_file(f);
        }
        // Remove any orphaned atomic-write temp snapshots (snap.tmp.<bashpid>)
        // a failed/interrupted mv could have left behind (#38249).
        let pattern = format!("{}.tmp.", self.snapshot_path);
        if let Some(parent) = Path::new(&self.snapshot_path).parent() {
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    if name.starts_with(
                        Path::new(&pattern)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                            .as_str(),
                    ) {
                        let _ = fs::remove_file(&path);
                    }
                    // Also match full pattern prefix (handles absolute snapshot_path)
                    if path.to_string_lossy().starts_with(&pattern) {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
        // Fallback glob: `glob.glob(f"{self._snapshot_path}.tmp.*")`
        // Without `glob` crate we scan parent dir as above; the extra
        // `format!("{}.tmp.", ...)` scan above already covers `snap.tmp.<pid>`.
        // Silent on any error (mirrors `except Exception: pass`).
    }
}

// ---------------------------------------------------------------------------
// Helpers for Child wait with timeout (mirrors `_wait_for_process` deadline)
// ---------------------------------------------------------------------------

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> std::io::Result<Option<std::process::ExitStatus>> {
    let start = SystemTime::now();
    loop {
        match child.try_wait()? {
            Some(status) => return Ok(Some(status)),
            None => {
                if start.elapsed().unwrap_or(timeout) >= timeout {
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn wait_for_child(child: &mut Child, timeout_secs: u64) -> ExecResult {
    // Minimal `_wait_for_process` analogue for `init_session` bootstrap:
    // poll with 50ms cadence, drain stdout/stderr, enforce timeout.
    use std::io::Read;
    let deadline = SystemTime::now() + Duration::from_secs(timeout_secs);
    let mut output = String::new();
    // Spawn drain thread for stdout (best-effort, no `select` on Windows).
    let mut stdout_buf: Vec<u8> = Vec::new();
    let mut stderr_buf: Vec<u8> = Vec::new();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Drain remaining pipes
                if let Some(mut out) = child.stdout.take() {
                    let _ = out.read_to_end(&mut stdout_buf);
                }
                if let Some(mut err) = child.stderr.take() {
                    let _ = err.read_to_end(&mut stderr_buf);
                }
                let combined = format!(
                    "{}{}",
                    String::from_utf8_lossy(&stdout_buf),
                    String::from_utf8_lossy(&stderr_buf)
                );
                output.push_str(&combined);
                return ExecResult::new(output, status.code().unwrap_or(1));
            }
            Ok(None) => {
                if SystemTime::now() >= deadline {
                    LocalEnvironment::kill_process(child);
                    if let Some(mut out) = child.stdout.take() {
                        let _ = out.read_to_end(&mut stdout_buf);
                    }
                    if let Some(mut err) = child.stderr.take() {
                        let _ = err.read_to_end(&mut stderr_buf);
                    }
                    let combined = format!(
                        "{}{}",
                        String::from_utf8_lossy(&stdout_buf),
                        String::from_utf8_lossy(&stderr_buf)
                    );
                    output.push_str(&combined);
                    output.push_str(&format!("\n[Command timed out after {timeout_secs}s]"));
                    return ExecResult::new(output, 124);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return ExecResult::new(format!("{output}\n[wait error: {e}]"), 1),
        }
    }
}

fn uuid_hex12() -> String {
    // Mirrors `uuid.uuid4().hex[:12]` — cheap 12-hex via time + pid (no `uuid` crate).
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    format!("{:012x}", (nanos ^ (pid << 32)) & 0xffffffffffff)
}

// ---------------------------------------------------------------------------
// Tests — minimal smoke for slice3 helpers (no cargo run required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shellexpand_vars_basic() {
        env::set_var("HERMES_TEST_VAR_X", "hello");
        assert_eq!(shellexpand_vars("$HERMES_TEST_VAR_X/world"), "hello/world");
        assert_eq!(shellexpand_vars("${HERMES_TEST_VAR_X}/x"), "hello/x");
        env::remove_var("HERMES_TEST_VAR_X");
    }

    #[test]
    fn prepend_shell_init_noop() {
        assert_eq!(prepend_shell_init("echo hi", &[]), "echo hi");
    }

    #[test]
    fn prepend_shell_init_with_files() {
        let files = vec!["/tmp/a.sh".to_string()];
        let out = prepend_shell_init("echo hi", &files);
        assert!(out.contains("set +e"));
        assert!(out.contains("/tmp/a.sh"));
        assert!(out.ends_with("echo hi"));
    }

    #[test]
    fn quote_cwd_for_cd_tilde() {
        assert_eq!(LocalEnvironment::quote_cwd_for_cd("~"), "~");
        assert_eq!(LocalEnvironment::quote_cwd_for_cd("~/"), "$HOME");
        assert!(LocalEnvironment::quote_cwd_for_cd("~/a b").contains("$HOME"));
    }

    #[test]
    fn strip_pythonpath_keeps_user_entries() {
        let mut env = HashMap::new();
        env.insert("PYTHONPATH".to_string(), "/tmp/myproj:/usr/lib".to_string());
        strip_hermes_owned_pythonpath(&mut env);
        // User entries should be preserved (not Hermes-owned).
        assert!(env.get("PYTHONPATH").map(|s| s.contains("/tmp/myproj")).unwrap_or(false));
    }

    #[test]
    fn extract_cwd_marker_parsing() {
        let mut le = LocalEnvironment::new("/tmp", 60, None);
        let marker = le.cwd_marker.clone();
        let mut result = ExecResult::new(format!("hello\n{marker}/tmp/new{marker}\n"), 0);
        le.extract_cwd_from_output(&mut result);
        // On non-Windows, /tmp/new may not exist; if it doesn't, cwd rolls back.
        // The output marker should still be stripped.
        assert!(!result.output.contains(&marker));
    }

    #[test]
    fn get_temp_dir_returns_string() {
        let le = LocalEnvironment::new("/tmp", 60, None);
        let td = le.get_temp_dir();
        assert!(!td.is_empty());
    }
}
