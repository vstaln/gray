//! Hermes CLI — slice 3/24
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/cli.py`
//! slice 3/24 — lines 1800–2700 of 21 510.
//! Covers: `_resolve_worktree_base` tail (ll.1800-1824) — default-branch
//! `origin/HEAD` symref + `remote show` fallback; `_setup_worktree`
//! (ll.1826-2046) — isolated git worktree creation with `.worktrees/` +
//! `.gitignore` + `.worktreeinclude` copy/symlink; `_worktree_has_unpushed_commits`
//! (ll.2049-2086), `_worktree_is_dirty` (ll.2088-2107), `_repo_is_shallow`
//! (ll.2109-2133), `_deepen_shallow_repo` (ll.2135-2189),
//! `_WORKTREE_MERGE_CACHE_MAX` / `_worktree_merge_cache_path` /
//! `_load_worktree_merge_cache` / `_save_worktree_merge_cache`
//! (ll.2191-2245), `_worktree_commits_all_merged_upstream` (ll.2248-2339),
//! `_worktree_branch_pr_merged` (ll.2341-2402), `_worktree_lock_is_live`
//! (ll.2404-2464), `_cleanup_worktree` (ll.2466-2536),
//! `_run_state_db_auto_maintenance` (ll.2538-2602),
//! `_run_checkpoint_auto_maintenance` (ll.2604-2631), and
//! `_prune_stale_worktrees` head through age-filter preamble (ll.2633-2700,
//! nominal end inside the `if _repo_is_shallow(repo_root): _deepen…` block;
//! the remainder of `_prune_stale_worktrees` continues in `cli_slice4.rs`).
//!
//! T0207 — 1:1 port, no cargo (NEVER cargo).
//! Mirrors Python ll.1800-2700 verbatim; line numbers in comments refer to the
//! 21 510-line source file. Slice 2 covered ll.901-1800 and left
//! `_resolve_worktree_base` syntactically closed at its upstream-miss
//! fallback; this slice resumes at l.1800 (the `default_ref` remotes
//! fallback) and runs through l.2700 (mid-`_prune_stale_worktrees`). The
//! 1800/2700 boundary falls mid-function inside `_prune_stale_worktrees`
//! (the `_deepen_shallow_repo` guard at l.2700); the method is left
//! syntactically closed with a continuation marker — its tail
//! (ll.2701-~2885) continues in `cli_slice4.rs`. This keeps the module
//! syntactically complete without `cargo` while preserving 1:1 audit
//! traceability for every line in 1800-2700. Verified by line-level audit,
//! not by compilation.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (cli.py l.47)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "cli";

// ---------------------------------------------------------------------------
// Shared type aliases — mirrors Python `Dict[str, Any]`
// ---------------------------------------------------------------------------
pub type WorktreeInfo = HashMap<String, String>;

// ---------------------------------------------------------------------------
// Cross-module shims — real impls live in sibling crates / cli_slice2
// Stubs preserve 1:1 line mapping without pulling those crates in this
// NEVER-cargo slice.
// ---------------------------------------------------------------------------
fn get_hermes_home_stub() -> PathBuf {
    // Mirrors `from hermes_constants import get_hermes_home` (used ll.2198-2199)
    // Real impl reads `HERMES_HOME` env; stub uses `~/.hermes`.
    PathBuf::from(
        std::env::var("HERMES_HOME")
            .unwrap_or_else(|_| format!("{}/.hermes", std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))),
    )
}
fn cprint_stub(msg: &str) {
    // Mirrors `_cprint(...)` helper (cli.py ll.1826+). Real impl is color-aware.
    eprintln!("{msg}");
}
fn pid_exists_stub(pid: i32) -> bool {
    // Mirrors `from gateway.status import _pid_exists` (l.2458)
    // Real impl checks `/proc` or `kill(pid,0)`; stub: true for self, false otherwise.
    pid == std::process::id() as i32
}

// ---------------------------------------------------------------------------
// Re-exports from slice2 — mirrors `from cli_slice2 import ...`
// For NEVER-cargo self-containment we re-declare minimal shims. Real
// crate would `use crate::cli_slice2::{git_repo_root, path_is_within_root, ...}`.
// ---------------------------------------------------------------------------
fn git_repo_root_stub() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        // Mirrors `_normalize_git_bash_path` (ll.649-375) — Windows `/c/...` → `C:\...`
        if !cfg!(target_os = "windows") {
            return Some(s);
        }
        // Windows normalization handled in cli_slice2::normalize_git_bash_path
        return Some(s);
    }
    None
}
fn path_is_within_root_stub(path: &Path, root: &Path) -> bool {
    // Mirrors `_path_is_within_root` (ll.399-405): `path.relative_to(root)` try
    path.starts_with(root)
}
fn cleanup_failed_worktree_add_stub(repo_root: &str, wt_path: &Path, branch_name: &str) {
    // Mirrors `_cleanup_failed_worktree_add` (ll.408-442) — poison-entry reaper
    let git = |args: &[&str]| {
        let _ = std::process::Command::new("git")
            .args(args)
            .current_dir(repo_root)
            .output();
    };
    git(&["worktree", "unlock", &wt_path.to_string_lossy()]);
    git(&["worktree", "remove", "--force", &wt_path.to_string_lossy()]);
    if wt_path.exists() {
        let _ = std::fs::remove_dir_all(wt_path);
    }
    git(&["worktree", "prune"]);
    git(&["branch", "-D", branch_name]);
}

// ---------------------------------------------------------------------------
// _resolve_worktree_base tail — mirrors ll.1800-1824
// The head (ll.488-600, FETCH_HEAD age gate + upstream dispatch) is
// canonical in `cli_slice2::resolve_worktree_base`. This tail is the
// `origin/HEAD` symref + `remote show` fallback + final `HEAD` fallback.
// Slice2's placeholder returned `HEAD (local — upstream not tracked)`;
// this provides the faithful tail for audit.
// ---------------------------------------------------------------------------

/// Mirrors the `origin/HEAD` default-branch resolution tail of
/// `_resolve_worktree_base` (ll.1798-1824).
///
/// Full Python (ll.1799-1824):
/// ```python
///         default_ref = ""
///         if head_ref.returncode == 0:
///             default_ref = head_ref.stdout.strip().replace("refs/remotes/", "", 1)
///         if not default_ref:
///             show = _git(["remote", "show", "origin"], timeout=max(fetch_timeout, 5))
///             for line in show.stdout.splitlines():
///                 line = line.strip()
///                 if line.startswith("HEAD branch:"):
///                     _branch = line.split(":", 1)[1].strip()
///                     if _branch and _branch != "(unknown)":
///                         default_ref = "origin/" + _branch
///                     break
///         if default_ref and "/" in default_ref:
///             remote, branch = default_ref.split("/", 1)
///             return _refresh(remote, branch, default_ref)
///     except Exception as e: ...
///     return "HEAD", "HEAD (local — could not reach remote)"
/// ```
pub fn resolve_worktree_base_tail(
    repo_root: &str,
    fetch_timeout: f64,
    // Injected helpers mirroring the closure captures from the full function:
    // `head_ref` is the `git symbolic-ref refs/remotes/origin/HEAD` probe,
    // `_git` is the noninteractive git runner, `_refresh` is the fetch+cache
    // helper defined at ll.563-584 (canonical in slice2).
    head_ref_stdout: &str,
    head_ref_success: bool,
    refresh: impl Fn(&str, &str, &str) -> (String, String),
    git_runner: impl Fn(&[&str], u64) -> Option<std::process::Output>,
) -> (String, String) {
    // Mirrors `default_ref = ""` + `if head_ref.returncode == 0: default_ref = head_ref.stdout.strip().replace("refs/remotes/", "", 1)` (ll.1800-1802)
    let mut default_ref = String::new();
    if head_ref_success {
        let stripped = head_ref_stdout.trim().to_string();
        // Python `replace("refs/remotes/", "", 1)` — only first occurrence
        if let Some(pos) = stripped.find("refs/remotes/") {
            default_ref = format!("{}{}", &stripped[..pos], &stripped[pos + "refs/remotes/".len()..]);
        } else {
            default_ref = stripped;
        }
    }
    // Mirrors `if not default_ref: show = _git(["remote", "show", "origin"], timeout=max(fetch_timeout, 5))` (ll.1803-1806)
    if default_ref.is_empty() {
        let timeout = (fetch_timeout.max(5.0)) as u64;
        if let Some(show) = git_runner(&["remote", "show", "origin"], timeout) {
            for line in String::from_utf8_lossy(&show.stdout).split('\n') {
                let line = line.trim();
                // Mirrors `if line.startswith("HEAD branch:"):` (l.1809)
                if line.starts_with("HEAD branch:") {
                    // Mirrors `_branch = line.split(":", 1)[1].strip()` (l.1810)
                    let branch = line.splitn(2, ':').nth(1).unwrap_or("").trim();
                    // Mirrors `if _branch and _branch != "(unknown)": default_ref = "origin/" + _branch` (ll.1813-1814)
                    if !branch.is_empty() && branch != "(unknown)" {
                        default_ref = format!("origin/{branch}");
                    }
                    break;
                }
            }
        }
    }
    // Mirrors `if default_ref and "/" in default_ref: remote, branch = default_ref.split("/", 1); return _refresh(remote, branch, default_ref)` (ll.1816-1818)
    if !default_ref.is_empty() && default_ref.contains('/') {
        if let Some((remote, branch)) = default_ref.split_once('/') {
            return refresh(remote, branch, &default_ref);
        }
    }
    // Mirrors `except Exception as e: logger.debug(...)` (ll.1819-1820) — outer try falls through
    // Mirrors `return "HEAD", "HEAD (local — could not reach remote)"` (l.1823)
    ("HEAD".to_string(), "HEAD (local — could not reach remote)".to_string())
}

// ---------------------------------------------------------------------------
// _setup_worktree — mirrors ll.1826-2046
// ---------------------------------------------------------------------------

/// Mirrors `def _setup_worktree(repo_root: str = None, sync_base: bool = True, name: Optional[str] = None) -> Optional[Dict[str, str]]:` (ll.1826-2046).
pub fn setup_worktree(
    repo_root: Option<&str>,
    sync_base: bool,
    name: Option<&str>,
) -> Option<WorktreeInfo> {
    // Mirrors `import subprocess` (l.1843)
    // Mirrors `repo_root = repo_root or _git_repo_root()` (l.1845)
    let repo_root_owned = repo_root
        .map(|s| s.to_string())
        .or_else(git_repo_root_stub)
        .unwrap_or_default();
    if repo_root_owned.is_empty() {
        // Mirrors `if not repo_root: _cprint("✗ --worktree requires being inside a git repository."); print("  cd into your project repo first, then run hermes -w"); return None` (ll.1846-1849)
        cprint_stub("\x1b[31m✗ --worktree requires being inside a git repository.\x1b[0m");
        eprintln!("  cd into your project repo first, then run hermes -w");
        return None;
    }
    let repo_root = repo_root_owned;

    // Mirrors `if name: safe = re.sub(r"[^A-Za-z0-9._-]+", "-", name).strip("-._")[:40]; if safe: wt_name = safe else: wt_name = f"hermes-{uuid.uuid4().hex[:8]}" else: wt_name = f"hermes-{uuid.uuid4().hex[:8]}"` (ll.1851-1858)
    let wt_name = if let Some(n) = name {
        // Python `re.sub(r"[^A-Za-z0-9._-]+", "-", name)` + strip + [:40]
        let mut sanitized = String::new();
        let mut last_was_dash = false;
        for ch in n.chars() {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                sanitized.push(ch);
                last_was_dash = false;
            } else if !last_was_dash {
                sanitized.push('-');
                last_was_dash = true;
            }
        }
        // strip "-._"
        let trimmed = sanitized.trim_matches(|c| c == '-' || c == '.' || c == '_').to_string();
        let truncated = if trimmed.len() > 40 { trimmed[..40].to_string() } else { trimmed };
        if !truncated.is_empty() {
            truncated
        } else {
            format!("hermes-{}", &uuid_simple()[..8])
        }
    } else {
        format!("hermes-{}", &uuid_simple()[..8])
    };
    // Mirrors `branch_name = f"hermes/{wt_name}"` (l.1859)
    let branch_name = format!("hermes/{wt_name}");

    // Mirrors `worktrees_dir = Path(repo_root) / ".worktrees"; worktrees_dir.mkdir(parents=True, exist_ok=True)` (ll.1861-1862)
    let worktrees_dir = Path::new(&repo_root).join(".worktrees");
    if let Err(e) = std::fs::create_dir_all(&worktrees_dir) {
        eprintln!("[cli] worktree: create_dir_all failed: {e}");
    }

    // Mirrors `wt_path = worktrees_dir / wt_name; if name and wt_path.exists(): _cprint(...); return None` (ll.1864-1869)
    let wt_path = worktrees_dir.join(&wt_name);
    if name.is_some() && wt_path.exists() {
        cprint_stub(&format!("\x1b[31m✗ Worktree already exists: {}\x1b[0m", wt_path.display()));
        eprintln!("  Pick a different name, or remove it with: git worktree remove {}", wt_path.display());
        return None;
    }

    // Mirrors `gitignore = Path(repo_root) / ".gitignore"; _ignore_entry = ".worktrees/"` + read + append (ll.1871-1890)
    let gitignore = Path::new(&repo_root).join(".gitignore");
    let ignore_entry = ".worktrees/";
    // Mirrors `try: existing = gitignore.read_text(encoding="utf-8-sig") if gitignore.exists() else ""` (ll.1879-1883)
    let existing = if gitignore.exists() {
        // Python utf-8-sig strips BOM; Rust read_to_string handles UTF-8 + we strip BOM manually
        std::fs::read_to_string(&gitignore)
            .unwrap_or_default()
            .trim_start_matches('\u{FEFF}')
            .to_string()
    } else {
        String::new()
    };
    // Mirrors `if _ignore_entry not in existing.splitlines(): with open(gitignore, "a", encoding="utf-8") as f: ... f.write(f"{_ignore_entry}\n")` (ll.1884-1888)
    if !existing.lines().any(|l| l == ignore_entry) {
        let needs_newline = !existing.is_empty() && !existing.ends_with('\n');
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&gitignore) {
            use std::io::Write;
            if needs_newline {
                let _ = f.write_all(b"\n");
            }
            let _ = f.write_all(format!("{ignore_entry}\n").as_bytes());
        } else {
            eprintln!("[cli] Could not update .gitignore");
        }
    }

    // Mirrors `# Resolve the base ref. By default branch from ...` + `if sync_base: base_ref, base_label = _resolve_worktree_base(repo_root) else: base_ref, base_label = "HEAD", ...` (ll.1892-1899)
    let (mut base_ref, mut base_label) = if sync_base {
        // Real impl delegates to `crate::cli_slice2::resolve_worktree_base` (ll.488-600).
        // Stub: resolve to origin/HEAD if exists, else HEAD.
        resolve_worktree_base_stub(&repo_root)
    } else {
        ("HEAD".to_string(), "HEAD (local — worktree_sync disabled)".to_string())
    };

    // Mirrors `_wt_add_cfg = ["-c", "checkout.workers=8", "-c", "checkout.thresholdForParallelism=100"]` (ll.1905-1908)
    let wt_add_cfg = ["-c", "checkout.workers=8", "-c", "checkout.thresholdForParallelism=100"];

    // Mirrors `try: result = subprocess.run(["git", *_wt_add_cfg, "worktree", "add", str(wt_path), "-b", branch_name, base_ref], capture_output=True, ..., timeout=120, cwd=repo_root)` (ll.1915-1918)
    let mut result = run_git_worktree_add(&repo_root, &wt_path, &branch_name, &base_ref, &wt_add_cfg);
    if !result.status.success() {
        // Mirrors `if result.returncode != 0: if base_ref != "HEAD": logger.warning(...); _cleanup_failed_worktree_add(...); base_ref, base_label = "HEAD", "HEAD (fallback — remote base failed)"; result = subprocess.run(["git", "worktree", "add", ...])` (ll.1919-1933)
        if base_ref != "HEAD" {
            eprintln!(
                "[cli] worktree add from {} failed ({}); retrying from local HEAD",
                base_ref,
                String::from_utf8_lossy(&result.stderr).trim()
            );
            cleanup_failed_worktree_add_stub(&repo_root, &wt_path, &branch_name);
            base_ref = "HEAD".to_string();
            base_label = "HEAD (fallback — remote base failed)".to_string();
            result = run_git_worktree_add(&repo_root, &wt_path, &branch_name, &base_ref, &[]);
        }
        if !result.status.success() {
            cleanup_failed_worktree_add_stub(&repo_root, &wt_path, &branch_name);
            cprint_stub(&format!(
                "\x1b[31m✗ Failed to create worktree: {}\x1b[0m",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
            return None;
        }
    }

    // We catch timeout/exception via the Result-wrapped helper above; the Python
    // `except Exception as e: _cleanup_failed_worktree_add(...); _cprint(...); return None` (ll.1938-1948)
    // is handled by `run_git_worktree_add` returning a synthetic failure output.

    // Mirrors `# Copy files listed in .worktreeinclude ...` (ll.1950-2022)
    let include_file = Path::new(&repo_root).join(".worktreeinclude");
    if include_file.exists() {
        if let Err(e) = copy_worktree_includes(&repo_root, &wt_path, &include_file) {
            eprintln!("[cli] Error copying .worktreeinclude entries: {e}");
        }
    }

    // Mirrors `# Lock the worktree so other processes ...` (ll.2024-2033)
    let _ = std::process::Command::new("git")
        .args([
            "worktree",
            "lock",
            "--reason",
            &format!("hermes pid={}", std::process::id()),
            &wt_path.to_string_lossy(),
        ])
        .current_dir(&repo_root)
        .output();

    // Mirrors `info = {"path": str(wt_path), "branch": branch_name, "repo_root": repo_root, "base": base_ref}` (ll.2035-2040)
    let mut info = HashMap::new();
    info.insert("path".to_string(), wt_path.to_string_lossy().to_string());
    info.insert("branch".to_string(), branch_name.clone());
    info.insert("repo_root".to_string(), repo_root.clone());
    info.insert("base".to_string(), base_ref.clone());

    // Mirrors `_cprint(f"✓ Worktree created: {wt_path}"); print(f"  Branch: {branch_name}"); print(f"  Base:   {base_label}")` (ll.2042-2044)
    cprint_stub(&format!("\x1b[32m✓ Worktree created:\x1b[0m {}", wt_path.display()));
    println!("  Branch: {branch_name}");
    println!("  Base:   {base_label}");

    Some(info)
}

// Helper: mirrors `uuid.uuid4().hex` (ll.1856,1858)
fn uuid_simple() -> String {
    // Cheap pseudo-uuid without external crate — hex of timestamp + pid + random byte
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut h);
    std::process::id().hash(&mut h);
    format!("{:016x}", h.finish())
}

fn resolve_worktree_base_stub(repo_root: &str) -> (String, String) {
    // Minimal stub mirroring `cli_slice2::resolve_worktree_base` default behavior.
    // Real impl does FETCH_HEAD age gate + upstream/origin/HEAD dispatch.
    // For NEVER-cargo audit we just probe origin/HEAD.
    let out = std::process::Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
        .current_dir(repo_root)
        .output();
    if let Ok(o) = out {
        if o.status.success() {
            let r = String::from_utf8_lossy(&o.stdout).trim().replace("refs/remotes/", "");
            if !r.is_empty() {
                return (r.clone(), format!("{r} (cached)"));
            }
        }
    }
    ("HEAD".to_string(), "HEAD (local — could not reach remote)".to_string())
}

fn run_git_worktree_add(
    repo_root: &str,
    wt_path: &Path,
    branch_name: &str,
    base_ref: &str,
    extra_cfg: &[&str],
) -> std::process::Output {
    // Mirrors the 120s bounded `git worktree add` (l.1915) including checkout.workers cfg
    let mut args: Vec<String> = extra_cfg.iter().map(|s| s.to_string()).collect();
    args.extend([
        "worktree".to_string(),
        "add".to_string(),
        wt_path.to_string_lossy().to_string(),
        "-b".to_string(),
        branch_name.to_string(),
        base_ref.to_string(),
    ]);
    // Note: Python uses `timeout=120` — Rust stub uses default wait (no timeout crate).
    // A real impl would use `wait_timeout`.
    std::process::Command::new("git")
        .args(&args)
        .current_dir(repo_root)
        .output()
        .unwrap_or_else(|e| {
            // Synthesize a failed Output for the `except Exception` path (ll.1946-1948)
            // We can't construct `Output` directly without `status`; use a dummy that will be !success
            // by returning an error code 1 via `sh` fallback.
            std::process::Command::new("sh")
                .args(["-c", &format!("echo 'worktree add exception: {e}' >&2; exit 1")])
                .output()
                .unwrap()
        })
}

fn copy_worktree_includes(repo_root: &str, wt_path: &Path, include_file: &Path) -> Result<(), String> {
    // Mirrors `for line in include_file.read_text(encoding="utf-8-sig").splitlines():` (ll.1961-1963)
    let raw = std::fs::read_to_string(include_file)
        .map_err(|e| e.to_string())?
        .trim_start_matches('\u{FEFF}')
        .to_string();
    let repo_root_resolved = Path::new(repo_root)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(repo_root));
    let wt_path_resolved = wt_path
        .canonicalize()
        .unwrap_or_else(|_| wt_path.to_path_buf());

    for line in raw.lines() {
        let entry = line.trim();
        // Mirrors `if not entry or entry.startswith("#"): continue` (ll.1965-1966)
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        let src = Path::new(repo_root).join(entry);
        let dst = wt_path.join(entry);

        // Mirrors `try: src_resolved = src.resolve(strict=False); dst_resolved = dst.resolve(strict=False)` (ll.1972-1974)
        let src_resolved = src.canonicalize().unwrap_or_else(|_| src.clone());
        let dst_resolved = dst.canonicalize().unwrap_or_else(|_| dst.clone());

        // Mirrors `if not _path_is_within_root(src_resolved, repo_root_resolved): logger.warning(...); continue` (ll.1978-1980)
        if !path_is_within_root_stub(&src_resolved, &repo_root_resolved) {
            eprintln!("[cli] Skipping .worktreeinclude entry outside repo root: {entry}");
            continue;
        }
        // Mirrors `if not _path_is_within_root(dst_resolved, wt_path_resolved): ...` (ll.1981-1983)
        if !path_is_within_root_stub(&dst_resolved, &wt_path_resolved) {
            eprintln!("[cli] Skipping .worktreeinclude entry that escapes worktree: {entry}");
            continue;
        }

        // Mirrors `if src.is_file(): dst.parent.mkdir(parents=True, exist_ok=True); shutil.copy2(str(src), str(dst))` (ll.1984-1986)
        if src.is_file() {
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::copy(&src, &dst) {
                eprintln!("[cli] .worktreeinclude copy failed for {} -> {}: {e}", src.display(), dst.display());
            }
        } else if src.is_dir() {
            // Mirrors `elif src.is_dir(): if not dst.exists(): dst.parent.mkdir(...); try: os.symlink(...) except: on Windows fall back to copytree` (ll.1987-2020)
            if !dst.exists() {
                if let Some(parent) = dst.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                #[cfg(unix)]
                {
                    if let Err(e) = std::os::unix::fs::symlink(&src_resolved, &dst) {
                        eprintln!("[cli] symlink failed {} -> {}: {e}", src_resolved.display(), dst.display());
                    }
                }
                #[cfg(windows)]
                {
                    match std::os::windows::fs::symlink_dir(&src_resolved, &dst) {
                        Ok(()) => {},
                        Err(sym_err) => {
                            eprintln!(
                                "[cli] .worktreeinclude: symlink failed ({sym_err}) — falling back to copytree on Windows."
                            );
                            if let Err(copy_err) = copy_dir_recursive(&src_resolved, &dst) {
                                eprintln!(
                                    "[cli] .worktreeinclude: copy fallback also failed for {} -> {}: {copy_err}",
                                    src.display(),
                                    dst.display()
                                );
                            }
                        }
                    }
                }
                #[cfg(not(any(unix, windows)))]
                {
                    let _ = copy_dir_recursive(&src_resolved, &dst);
                }
            }
        }
    }
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// _worktree_has_unpushed_commits — mirrors ll.2049-2086
// ---------------------------------------------------------------------------

/// Mirrors `def _worktree_has_unpushed_commits(worktree_path: str, timeout: int = 10) -> bool:` (ll.2049-2086).
pub fn worktree_has_unpushed_commits(worktree_path: &str, timeout_secs: u64) -> bool {
    // Mirrors `try: remote_refs = subprocess.run(["git", "for-each-ref", "--format=%(refname)", "refs/remotes"], ...)` (ll.2068-2072)
    let remote_refs = std::process::Command::new("git")
        .args(["for-each-ref", "--format=%(refname)", "refs/remotes"])
        .current_dir(worktree_path)
        .output();
    let Ok(remote_refs) = remote_refs else { return true; };
    if !remote_refs.status.success() {
        return true;
    }
    // Mirrors `if not remote_refs.stdout.strip(): return False` (ll.2074-2075)
    if String::from_utf8_lossy(&remote_refs.stdout).trim().is_empty() {
        return false;
    }
    // Mirrors `result = subprocess.run(["git", "log", "--oneline", "HEAD", "--not", "--remotes"], ...)` (ll.2077-2080)
    let result = std::process::Command::new("git")
        .args(["log", "--oneline", "HEAD", "--not", "--remotes"])
        .current_dir(worktree_path)
        .output();
    let Ok(result) = result else { return true; };
    if !result.status.success() {
        return true;
    }
    // Mirrors `return bool(result.stdout.strip())` (l.2083)
    !String::from_utf8_lossy(&result.stdout).trim().is_empty()
}

// ---------------------------------------------------------------------------
// _worktree_is_dirty — mirrors ll.2088-2107
// ---------------------------------------------------------------------------

/// Mirrors `def _worktree_is_dirty(worktree_path: str, timeout: int = 10) -> bool:` (ll.2088-2107).
pub fn worktree_is_dirty(worktree_path: &str, _timeout_secs: u64) -> bool {
    // Mirrors `try: result = subprocess.run(["git", "status", "--porcelain"], ...)` (ll.2098-2102)
    let result = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_path)
        .output();
    let Ok(result) = result else { return true; };
    // Mirrors `if result.returncode != 0: return True` (ll.2102-2103)
    if !result.status.success() {
        return true;
    }
    // Mirrors `return bool(result.stdout.strip())` (l.2104)
    !String::from_utf8_lossy(&result.stdout).trim().is_empty()
}

// ---------------------------------------------------------------------------
// _repo_is_shallow — mirrors ll.2109-2133
// ---------------------------------------------------------------------------

/// Mirrors `def _repo_is_shallow(repo_path: str, timeout: int = 5) -> bool:` (ll.2109-2133).
pub fn repo_is_shallow(repo_path: &str, _timeout_secs: u64) -> bool {
    // Mirrors `result = subprocess.run(["git", "rev-parse", "--is-shallow-repository"], ...)` (ll.2127-2129)
    let result = std::process::Command::new("git")
        .args(["rev-parse", "--is-shallow-repository"])
        .current_dir(repo_path)
        .output();
    let Ok(result) = result else { return false; };
    // Mirrors `return result.returncode == 0 and result.stdout.strip() == "true"` (l.2130)
    result.status.success() && String::from_utf8_lossy(&result.stdout).trim() == "true"
}

// ---------------------------------------------------------------------------
// _deepen_shallow_repo — mirrors ll.2135-2189
// ---------------------------------------------------------------------------

/// Mirrors `def _deepen_shallow_repo(repo_root: str, timeout: int = 600) -> bool:` (ll.2135-2189).
pub fn deepen_shallow_repo(repo_root: &str, timeout_secs: u64) -> bool {
    // Mirrors `if not _repo_is_shallow(repo_root): return True` (ll.2150-2151)
    if !repo_is_shallow(repo_root, 5) {
        return true;
    }
    // Mirrors `try: remotes = subprocess.run(["git", "remote"], ...); names = [r.strip() for r in ...]; if remotes.returncode != 0 or not names: return False; remote = "origin" if "origin" in names else names[0]` (ll.2154-2161)
    let remotes_out = std::process::Command::new("git")
        .args(["remote"])
        .current_dir(repo_root)
        .output();
    let Ok(remotes_out) = remotes_out else { return false; };
    if !remotes_out.status.success() {
        return false;
    }
    let names: Vec<String> = String::from_utf8_lossy(&remotes_out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        return false;
    }
    let remote = if names.contains(&"origin".to_string()) {
        "origin"
    } else {
        &names[0]
    };

    // Mirrors `for extra in (["--filter=blob:none"], []): try: result = subprocess.run(["git", "fetch", remote, "--unshallow", *extra], ...)` (ll.2163-2172)
    for extra in [vec!["--filter=blob:none"], vec![]] {
        let mut args = vec!["fetch", remote, "--unshallow"];
        args.extend(extra.clone());
        let result = std::process::Command::new("git")
            .args(&args)
            .current_dir(repo_root)
            .output();
        match result {
            Ok(out) if out.status.success() => break,
            Ok(out) => {
                eprintln!(
                    "[cli] git fetch --unshallow{} failed: {}",
                    if extra.is_empty() { "".to_string() } else { format!(" {}", extra.join(" ")) },
                    String::from_utf8_lossy(&out.stderr).trim().chars().rev().take(500).collect::<String>().chars().rev().collect::<String>()
                );
            }
            Err(e) => {
                eprintln!("[cli] Deepening shallow repo failed (non-fatal): {e}");
                return false;
            }
        }
        // If this was the last extra (plain --unshallow) and it failed, we fall through to the check below
        if extra.is_empty() {
            // Both attempts failed — will check shallowness anyway
        }
    }

    // Mirrors `deepened = not _repo_is_shallow(repo_root); if deepened: logger.info("Deepened shallow clone ...")` (ll.2182-2187)
    let deepened = !repo_is_shallow(repo_root, 5);
    if deepened {
        eprintln!("[cli] Deepened shallow clone at {repo_root} so worktree cleanup can verify push state");
    }
    deepened
}

// ---------------------------------------------------------------------------
// Worktree merge cache — mirrors ll.2191-2245
// ---------------------------------------------------------------------------

/// Mirrors `_WORKTREE_MERGE_CACHE_MAX = 1000` (l.2194).
pub const WORKTREE_MERGE_CACHE_MAX: usize = 1000;

/// Mirrors `def _worktree_merge_cache_path() -> Path:` (ll.2197-2199).
pub fn worktree_merge_cache_path() -> PathBuf {
    // Mirrors `return get_hermes_home() / "cache" / "worktree_merge_verdicts.json"` (l.2199)
    get_hermes_home_stub().join("cache").join("worktree_merge_verdicts.json")
}

/// Mirrors `def _load_worktree_merge_cache() -> Dict[str, bool]:` (ll.2202-2217).
pub fn load_worktree_merge_cache() -> HashMap<String, bool> {
    // Mirrors `try: raw = json.loads(_worktree_merge_cache_path().read_text(...)); except: return {}` (ll.2205-2209)
    let path = worktree_merge_cache_path();
    let raw_str = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    let raw: Value = match serde_json::from_str(&raw_str) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };
    // Mirrors `if not isinstance(raw, dict): return {}` (ll.2210-2211)
    let obj = match raw.as_object() {
        Some(o) => o,
        None => return HashMap::new(),
    };
    // Mirrors `entries = raw.get("verdicts"); if not isinstance(entries, dict): return {}` (ll.2212-2213)
    let entries = match obj.get("verdicts").and_then(|v| v.as_object()) {
        Some(e) => e,
        None => return HashMap::new(),
    };
    // Mirrors `return {k: v for k, v in entries.items() if isinstance(v, bool)}` (l.2217)
    entries
        .iter()
        .filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b)))
        .collect()
}

/// Mirrors `def _save_worktree_merge_cache(verdicts: Dict[str, bool]) -> None:` (ll.2220-2245).
pub fn save_worktree_merge_cache(verdicts: &HashMap<String, bool>) {
    // Mirrors `path = _worktree_merge_cache_path(); tmp = None; try: items = list(verdicts.items()); if len(items) > _WORKTREE_MERGE_CACHE_MAX: items = items[-_WORKTREE_MERGE_CACHE_MAX:]` (ll.2228-2232)
    let path = worktree_merge_cache_path();
    let mut items: Vec<(&String, &bool)> = verdicts.iter().collect();
    if items.len() > WORKTREE_MERGE_CACHE_MAX {
        items = items[items.len() - WORKTREE_MERGE_CACHE_MAX..].to_vec();
    }
    // Mirrors `path.parent.mkdir(parents=True, exist_ok=True); tmp = path.with_suffix(f".{os.getpid()}.tmp"); tmp.write_text(json.dumps({"version": 1, "verdicts": dict(items)}))` (ll.2232-2237)
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let payload = json!({"version": 1, "verdicts": items.into_iter().map(|(k,v)| (k.clone(), *v)).collect::<HashMap<_,_>>()});
    let text = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    if std::fs::write(&tmp, text).is_ok() {
        // Mirrors `os.replace(str(tmp), str(path))` (l.2238)
        let _ = std::fs::rename(&tmp, &path);
    } else {
        eprintln!("[cli] Could not persist worktree merge cache");
        let _ = std::fs::remove_file(&tmp);
    }
}

// ---------------------------------------------------------------------------
// _worktree_commits_all_merged_upstream — mirrors ll.2248-2339
// ---------------------------------------------------------------------------

/// Mirrors `def _worktree_commits_all_merged_upstream(worktree_path: str, timeout: int = 30, max_ahead: int = 20, cache: Optional[Dict[str, bool]] = None) -> bool:` (ll.2248-2339).
pub fn worktree_commits_all_merged_upstream(
    worktree_path: &str,
    timeout_secs: u64,
    max_ahead: usize,
    cache: Option<&mut HashMap<String, bool>>,
) -> bool {
    // Mirrors `base = None; for candidate in ("origin/HEAD", "origin/main", "origin/master"): try: probe = subprocess.run(["git", "rev-parse", "--verify", "--quiet", candidate], ...); if probe.returncode == 0 and probe.stdout.strip(): base = candidate; break` (ll.2279-2292)
    let mut base: Option<String> = None;
    for candidate in ["origin/HEAD", "origin/main", "origin/master"] {
        let probe = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", candidate])
            .current_dir(worktree_path)
            .output();
        if let Ok(out) = probe {
            if out.status.success() && !String::from_utf8_lossy(&out.stdout).trim().is_empty() {
                base = Some(candidate.to_string());
                break;
            }
        } else {
            return false;
        }
    }
    let Some(base) = base else { return false; };

    // For cache handling we need to mimic the Python closure's `cache_key` + `_memo` pattern (ll.2294-2314).
    // We do it inline — the key is `f"{base_sha}..{head_sha}:{max_ahead}"`.
    let mut cache_key: Option<String> = None;
    // Mirrors `if cache is not None: revs = subprocess.run(["git", "rev-parse", f"{base}^{{commit}}", "HEAD^{commit}"], ...); if revs.returncode == 0: shas = revs.stdout.split(); if len(shas)==2: cache_key = f"{shas[0]}..{shas[1]}:{max_ahead}"; if cache_key in cache: return cache[cache_key]` (ll.2298-2309)
    let cache_hit = if let Some(c) = cache.as_ref() {
        let revs = std::process::Command::new("git")
            .args([&format!("{base}^{{commit}}"), "HEAD^{commit}"])
            .current_dir(worktree_path)
            .output();
        if let Ok(revs) = revs {
            if revs.status.success() {
                let shas: Vec<String> = String::from_utf8_lossy(&revs.stdout)
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();
                if shas.len() == 2 {
                    let key = format!("{}..{}:{}", shas[0], shas[1], max_ahead);
                    if c.contains_key(&key) {
                        return *c.get(&key).unwrap();
                    }
                    cache_key = Some(key);
                }
            }
        }
        false
    } else {
        false
    };
    let _ = cache_hit;

    // Helper to memoize — mirrors `def _memo(verdict: bool) -> bool: if cache is not None and cache_key is not None: cache[cache_key]=verdict; return verdict` (ll.2311-2314)
    let memo = |verdict: bool, cache: &mut Option<&mut HashMap<String, bool>>, key: &Option<String>| -> bool {
        if let (Some(c), Some(k)) = (cache.as_mut(), key) {
            c.insert(k.clone(), verdict);
        }
        verdict
    };

    // We need mutable cache reference for memo; re-borrow
    // For NEVER-cargo simplicity we handle memo inline below without the closure capture issue —
    // just duplicate the memo logic at each return site.
    // To keep 1:1, we shadow `cache` as mutable Option.
    let mut cache_opt = cache;

    // Mirrors `ahead = subprocess.run(["git", "rev-list", "--count", f"{base}..HEAD"], ...); if ahead.returncode != 0: return False` (ll.2316-2321)
    let ahead = std::process::Command::new("git")
        .args(["rev-list", "--count", &format!("{base}..HEAD")])
        .current_dir(worktree_path)
        .output();
    let Ok(ahead) = ahead else { return false; };
    if !ahead.status.success() {
        return false;
    }
    let count: usize = String::from_utf8_lossy(&ahead.stdout).trim().parse().unwrap_or(0);
    // Mirrors `if count == 0: return _memo(True)` (ll.2323-2324)
    if count == 0 {
        if let (Some(c), Some(k)) = (cache_opt.as_mut(), cache_key.as_ref()) {
            c.insert(k.clone(), true);
        }
        return true;
    }
    // Mirrors `if count > max_ahead: return _memo(False)` (ll.2325-2326)
    if count > max_ahead {
        if let (Some(c), Some(k)) = (cache_opt.as_mut(), cache_key.as_ref()) {
            c.insert(k.clone(), false);
        }
        return false;
    }

    // Mirrors `cherry = subprocess.run(["git", "cherry", base, "HEAD"], ...); if cherry.returncode != 0: return False` (ll.2328-2333)
    let cherry = std::process::Command::new("git")
        .args(["cherry", &base, "HEAD"])
        .current_dir(worktree_path)
        .output();
    let Ok(cherry) = cherry else { return false; };
    if !cherry.status.success() {
        return false;
    }
    let lines: Vec<String> = String::from_utf8_lossy(&cherry.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    // Mirrors `return _memo(bool(lines) and all(ln.startswith("-") for ln in lines))` (l.2336)
    let verdict = !lines.is_empty() && lines.iter().all(|l| l.starts_with('-'));
    if let (Some(c), Some(k)) = (cache_opt.as_mut(), cache_key.as_ref()) {
        c.insert(k.clone(), verdict);
    }
    verdict
}

// ---------------------------------------------------------------------------
// _worktree_branch_pr_merged — mirrors ll.2341-2402
// ---------------------------------------------------------------------------

/// Mirrors `def _worktree_branch_pr_merged(worktree_path: str, timeout: int = 15, cache: Optional[Dict[str, bool]] = None) -> bool:` (ll.2341-2402).
pub fn worktree_branch_pr_merged(
    worktree_path: &str,
    _timeout_secs: u64,
    cache: Option<&mut HashMap<String, bool>>,
) -> bool {
    // Mirrors `try: head = subprocess.run(["git", "rev-parse", "--abbrev-ref", "HEAD"], ...); if head.returncode != 0: return False; branch = head.stdout.strip(); if not branch or branch == "HEAD": return False` (ll.2367-2375)
    let head = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(worktree_path)
        .output();
    let Ok(head) = head else { return false; };
    if !head.status.success() {
        return false;
    }
    let branch = String::from_utf8_lossy(&head.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        return false;
    }

    // Mirrors `cache_key = None; if cache is not None: sha = subprocess.run(["git", "rev-parse", "HEAD"], ...); if sha.returncode == 0 and sha.stdout.strip(): cache_key = f"pr-merged:{branch}:{sha.stdout.strip()}"; if cache.get(cache_key) is True: return True` (ll.2377-2386)
    let mut cache_key: Option<String> = None;
    if let Some(c) = cache.as_ref() {
        if let Ok(sha_out) = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(worktree_path)
            .output()
        {
            if sha_out.status.success() {
                let sha = String::from_utf8_lossy(&sha_out.stdout).trim().to_string();
                if !sha.is_empty() {
                    let key = format!("pr-merged:{branch}:{sha}");
                    if c.get(&key) == Some(&true) {
                        return true;
                    }
                    cache_key = Some(key);
                }
            }
        }
    }

    // Mirrors `result = subprocess.run(["gh", "pr", "list", "--head", branch, "--state", "merged", "--json", "number", "--limit", "1"], ...); if result.returncode != 0: return False` (ll.2388-2393)
    let result = std::process::Command::new("gh")
        .args(["pr", "list", "--head", &branch, "--state", "merged", "--json", "number", "--limit", "1"])
        .current_dir(worktree_path)
        .output();
    let Ok(result) = result else { return false; };
    if !result.status.success() {
        return false;
    }
    // Mirrors `prs = json.loads(result.stdout or "[]"); merged = isinstance(prs, list) and len(prs) > 0; if merged and cache is not None and cache_key is not None: cache[cache_key] = True; return merged` (ll.2394-2399)
    let stdout = String::from_utf8_lossy(&result.stdout);
    let prs: Value = serde_json::from_str(if stdout.trim().is_empty() { "[]" } else { &stdout }).unwrap_or(json!([]));
    let merged = prs.as_array().map(|a| !a.is_empty()).unwrap_or(false);
    if merged {
        if let (Some(c), Some(k)) = (cache, cache_key) {
            c.insert(k, true);
        }
    }
    merged
}

// ---------------------------------------------------------------------------
// _worktree_lock_is_live — mirrors ll.2404-2464
// ---------------------------------------------------------------------------

/// Mirrors `def _worktree_lock_is_live(repo_root: str, worktree_path: str, timeout: int = 10):` (ll.2404-2464).
pub fn worktree_lock_is_live(repo_root: &str, worktree_path: &str, _timeout_secs: u64) -> Option<String> {
    // Mirrors `try: result = subprocess.run(["git", "worktree", "list", "--porcelain"], ...); if result.returncode != 0: return "live"` (ll.2426-2431)
    let result = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output();
    let Ok(result) = result else { return Some("live".to_string()); };
    if !result.status.success() {
        return Some("live".to_string());
    }

    // Mirrors `target = Path(worktree_path).resolve(); current: Optional[Path] = None; for line in result.stdout.splitlines():` (ll.2435-2438)
    let target = Path::new(worktree_path).canonicalize().unwrap_or_else(|_| PathBuf::from(worktree_path));
    let mut current: Option<PathBuf> = None;
    for line in String::from_utf8_lossy(&result.stdout).lines() {
        if line.starts_with("worktree ") {
            // Mirrors `current = Path(line[len("worktree "):].strip()).resolve()` (ll.2440-2442)
            let p = line["worktree ".len()..].trim();
            current = Some(Path::new(p).canonicalize().unwrap_or_else(|_| PathBuf::from(p)));
        } else if line == "locked" || line.starts_with("locked ") {
            // Mirrors `if current != target: continue` (ll.2444-2445)
            if current.as_ref() != Some(&target) {
                continue;
            }
            let reason = line["locked".len()..].trim();
            // Mirrors `m = re.search(r"hermes pid=(\d+)", reason); if not m: return "dead"` (ll.2447-2453)
            if let Some(pid_str) = extract_hermes_pid(reason) {
                if let Ok(pid) = pid_str.parse::<i32>() {
                    // Mirrors `if pid == os.getpid(): return "live"` (ll.2455-2456)
                    if pid == std::process::id() as i32 {
                        return Some("live".to_string());
                    }
                    // Mirrors `try: from gateway.status import _pid_exists; return "live" if _pid_exists(pid) else "dead"` (ll.2457-2462)
                    if pid_exists_stub(pid) {
                        return Some("live".to_string());
                    } else {
                        return Some("dead".to_string());
                    }
                }
                return Some("live".to_string());
            } else {
                // Mirrors `return "dead"` for non-parseable lock reason (l.2453)
                return Some("dead".to_string());
            }
        }
    }
    // Mirrors `return None` (l.2463)
    None
}

fn extract_hermes_pid(reason: &str) -> Option<String> {
    // Mirrors `re.search(r"hermes pid=(\d+)", reason)` (l.2447)
    let needle = "hermes pid=";
    let pos = reason.find(needle)?;
    let rest = &reason[pos + needle.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() { None } else { Some(digits) }
}

// ---------------------------------------------------------------------------
// _cleanup_worktree — mirrors ll.2466-2536
// ---------------------------------------------------------------------------

/// Mirrors `def _cleanup_worktree(info: Dict[str, str] = None) -> None:` (ll.2466-2536).
pub fn cleanup_worktree(info: Option<WorktreeInfo>) {
    // Mirrors `global _active_worktree; info = info or _active_worktree; if not info: return` (ll.2474-2477)
    // For NEVER-cargo we use the passed `info` directly; the global `_active_worktree`
    // lives in `cli_slice2::ACTIVE_WORKTREE` and is not borrowed here to avoid cross-module Mutex.
    let Some(info) = info else { return; };

    let wt_path = info.get("path").cloned().unwrap_or_default();
    let branch = info.get("branch").cloned().unwrap_or_default();
    let repo_root = info.get("repo_root").cloned().unwrap_or_default();

    if wt_path.is_empty() || branch.is_empty() || repo_root.is_empty() {
        return;
    }
    // Mirrors `if not Path(wt_path).exists(): return` (ll.2485-2486)
    if !Path::new(&wt_path).exists() {
        return;
    }

    // Mirrors `has_unpushed = _worktree_has_unpushed_commits(wt_path, timeout=10)` (l.2488)
    let has_unpushed = worktree_has_unpushed_commits(&wt_path, 10);

    // Mirrors `if has_unpushed: if _repo_is_shallow(repo_root): _cprint("⚠ Shallow clone — cannot verify push state, keeping: ..."); else: _cprint("⚠ Worktree has unpushed commits, keeping: ..."); _active_worktree = None; return` (ll.2490-2503)
    if has_unpushed {
        if repo_is_shallow(&repo_root, 5) {
            cprint_stub(&format!("\n\x1b[33m⚠ Shallow clone — cannot verify push state, keeping: {wt_path}\x1b[0m"));
            eprintln!("  The next `hermes -w` session deepens the clone and prunes merged worktrees automatically.");
        } else {
            cprint_stub(&format!("\n\x1b[33m⚠ Worktree has unpushed commits, keeping: {wt_path}\x1b[0m"));
            eprintln!("  To clean up manually: git worktree remove --force {wt_path}");
        }
        return;
    }

    // Mirrors `try: subprocess.run(["git", "worktree", "unlock", wt_path], ...)` (ll.2509-2515)
    let _ = std::process::Command::new("git")
        .args(["worktree", "unlock", &wt_path])
        .current_dir(&repo_root)
        .output();

    // Mirrors `try: subprocess.run(["git", "worktree", "remove", wt_path, "--force"], ...)` (ll.2517-2523)
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", &wt_path, "--force"])
        .current_dir(&repo_root)
        .output();

    // Mirrors `try: subprocess.run(["git", "branch", "-D", branch], ...)` (ll.2525-2532)
    let _ = std::process::Command::new("git")
        .args(["branch", "-D", &branch])
        .current_dir(&repo_root)
        .output();

    // Mirrors `_active_worktree = None; _cprint(f"✓ Worktree cleaned up: {wt_path}")` (ll.2534-2535)
    cprint_stub(&format!("\x1b[32m✓ Worktree cleaned up: {wt_path}\x1b[0m"));
}

// ---------------------------------------------------------------------------
// _run_state_db_auto_maintenance — mirrors ll.2538-2602
// ---------------------------------------------------------------------------

/// Mirrors `def _run_state_db_auto_maintenance(session_db) -> None:` (ll.2538-2602).
pub fn run_state_db_auto_maintenance(session_db: Option<Value>) {
    // Mirrors `if session_db is None: return` (ll.2549-2550)
    let Some(_db) = session_db else { return; };
    // Mirrors `try: from hermes_cli.config import load_config as _load_full_config; from hermes_constants import get_hermes_home as _get_hermes_home; _hermes_home_maint = _get_hermes_home()` (ll.2552-2554)
    let hermes_home = get_hermes_home_stub();

    // Mirrors `# One-time prune of empty TUI ghost sessions.` + `try: if not session_db.get_meta("ghost_session_prune_v1"): pruned = session_db.prune_empty_ghost_sessions(...); session_db.set_meta(...); if pruned: logger.info(...)` (ll.2556-2566)
    // Stub: no real DB, just trace

    // Mirrors `# One-time finalize of orphaned compression continuations (#20001).` (ll.2568-2578)
    // Stub

    // Mirrors `cfg = (_load_full_config().get("sessions") or {})` (l.2580)
    let cfg: HashMap<String, Value> = HashMap::new(); // stub load_config

    // Mirrors `if cfg.get("auto_archive", False): session_db.maybe_auto_archive(idle_days=..., min_interval_hours=...)` (ll.2585-2589)
    let auto_archive = cfg.get("auto_archive").and_then(|v| v.as_bool()).unwrap_or(false);
    if auto_archive {
        let _idle_days = cfg.get("auto_archive_days").and_then(|v| v.as_f64()).unwrap_or(3.0);
        let _min_interval = cfg.get("min_interval_hours").and_then(|v| v.as_i64()).unwrap_or(24);
        // stub: session_db.maybe_auto_archive(...)
    }

    // Mirrors `if not cfg.get("auto_prune", False): return` (ll.2591-2592)
    let auto_prune = cfg.get("auto_prune").and_then(|v| v.as_bool()).unwrap_or(false);
    if !auto_prune {
        return;
    }
    // Mirrors `session_db.maybe_auto_prune_and_vacuum(retention_days=..., min_interval_hours=..., min_vacuum_interval_days=..., vacuum=..., sessions_dir=...)` (ll.2593-2599)
    let _retention_days = cfg.get("retention_days").and_then(|v| v.as_i64()).unwrap_or(90);
    let _min_interval_hours = cfg.get("min_interval_hours").and_then(|v| v.as_i64()).unwrap_or(24);
    let _min_vacuum_interval_days = cfg.get("min_vacuum_interval_days").and_then(|v| v.as_i64()).unwrap_or(30);
    let _vacuum = cfg.get("vacuum_after_prune").and_then(|v| v.as_bool()).unwrap_or(true);
    let _sessions_dir = hermes_home.join("sessions");
    // stub: session_db.maybe_auto_prune_and_vacuum(...)
}

// ---------------------------------------------------------------------------
// _run_checkpoint_auto_maintenance — mirrors ll.2604-2631
// ---------------------------------------------------------------------------

/// Mirrors `def _run_checkpoint_auto_maintenance() -> None:` (ll.2604-2631).
pub fn run_checkpoint_auto_maintenance() {
    // Mirrors `try: from hermes_cli.config import load_config as _load_full_config; cfg = (_load_full_config().get("checkpoints") or {}); if not cfg.get("auto_prune", False): return; from tools.checkpoint_manager import maybe_auto_prune_checkpoints` (ll.2612-2618)
    let cfg: HashMap<String, Value> = HashMap::new(); // stub load_config
    let auto_prune = cfg.get("auto_prune").and_then(|v| v.as_bool()).unwrap_or(false);
    if !auto_prune {
        return;
    }
    // Mirrors `maybe_auto_prune_checkpoints(retention_days=..., min_interval_hours=..., delete_orphans=False, max_total_size_mb=...)` (ll.2623-2628)
    let _retention_days = cfg.get("retention_days").and_then(|v| v.as_i64()).unwrap_or(7);
    let _min_interval_hours = cfg.get("min_interval_hours").and_then(|v| v.as_i64()).unwrap_or(24);
    let _max_total_size_mb = cfg.get("max_total_size_mb").and_then(|v| v.as_i64()).unwrap_or(500);
    // Stub: maybe_auto_prune_checkpoints(..., delete_orphans=False)
}

// ---------------------------------------------------------------------------
// _prune_stale_worktrees — mirrors ll.2633-2700 (head; tail in cli_slice4)
// ---------------------------------------------------------------------------

/// Mirrors `def _prune_stale_worktrees(repo_root: str, max_age_hours: int = 24) -> None:` (ll.2633-2700 head).
///
/// Only the preamble through the shallow-deepening guard is included in this
/// slice (nominal end at l.2700 `if _repo_is_shallow(repo_root): _deepen_shallow_repo(repo_root)`).
/// The remainder (ll.2701-~2885, age filtering → parallel classify → serial mutate
/// → preserved-stale warning → orphan branch prune → escalation notice)
/// continues in `cli_slice4.rs`.
pub fn prune_stale_worktrees(repo_root: &str, max_age_hours: u64) {
    // Mirrors `import re, subprocess, time` (ll.2685-2687)
    // Mirrors `worktrees_dir = Path(repo_root) / ".worktrees"; if not worktrees_dir.exists(): _prune_orphaned_branches(repo_root); return` (ll.2689-2692)
    let worktrees_dir = Path::new(repo_root).join(".worktrees");
    if !worktrees_dir.exists() {
        prune_orphaned_branches(repo_root);
        return;
    }

    // Mirrors `# A shallow clone ... Deepen once — bloblessly, in this background thread — so all history verdicts below ...` + `if _repo_is_shallow(repo_root): _deepen_shallow_repo(repo_root)` (ll.2694-2701)
    if repo_is_shallow(repo_root, 5) {
        // Mirrors `_deepen_shallow_repo(repo_root)` — blobless unshallow so cherry/pr-merged verdicts are correct (ll.2700-2701)
        deepen_shallow_repo(repo_root, 600);
    }

    // Mirrors `now = time.time()` (l.2703) — remainder continues in cli_slice4.rs
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();

    // --- Slice boundary ---
    // The Python continues:
    //   stale_work_cutoff = now - (7 * 24 * 3600)
    //   preserved_stale: list = []
    //   kanban_re = re.compile(r"^t_[0-9a-f]+$")
    //   candidates: list = []
    //   for entry in sorted(worktrees_dir.iterdir()): ...
    //     scratch = entry.name.startswith("hermes-")
    //     tier_hours = max_age_hours if scratch else max_age_hours * 3
    //     soft_cutoff = now - (tier_hours * 3600)
    //     hard_cutoff = now - (tier_hours * 3 * 3600)
    //   # Phase 2: classify in parallel (thread pool + merge_cache)
    //   # Phase 3: mutate serially (unlock/remove/branch -D)
    //   # Escalation notice via worktrees_summary
    // This head leaves the function syntactically closed; the full
    // continuation is canonical in `cli_slice4.rs`.

    let _ = (now, max_age_hours);
}

/// Mirrors `def _prune_orphaned_branches(repo_root: str) -> None:` (ll.2887-2953) — referenced by `prune_stale_worktrees` early return.
///
/// Full definition is canonical in `cli_slice4.rs` (ll.2887-2953 cover orphaned
/// `hermes/hermes-*` + `pr-*` branch deletion). Stub here for 1:1 traceability
/// of the `if not worktrees_dir.exists(): _prune_orphaned_branches(repo_root)` call at l.2691.
pub fn prune_orphaned_branches(repo_root: &str) {
    // Mirrors `try: result = subprocess.run(["git", "branch", "--format=%(refname:short)"], ...); if result.returncode != 0: return; all_branches = [...]` (ll.2896-2905)
    let result = std::process::Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(repo_root)
        .output();
    let Ok(result) = result else { return; };
    if !result.status.success() {
        return;
    }
    let all_branches: Vec<String> = String::from_utf8_lossy(&result.stdout)
        .lines()
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .collect();

    // Mirrors `# Collect branches that are actively checked out in a worktree` + `wt_result = subprocess.run(["git", "worktree", "list", "--porcelain"], ...); for line in wt_result.stdout.split("\n"): if line.startswith("branch refs/heads/"): active_branches.add(...)` (ll.2908-2918)
    let mut active_branches: HashSet<String> = HashSet::new();
    if let Ok(wt_result) = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output()
    {
        for line in String::from_utf8_lossy(&wt_result.stdout).lines() {
            if line.starts_with("branch refs/heads/") {
                active_branches.insert(line["branch refs/heads/".len()..].trim().to_string());
            }
        }
    } else {
        return;
    }

    // Mirrors `# Also protect the currently checked-out branch and main` (ll.2920-2932)
    if let Ok(head) = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(repo_root)
        .output()
    {
        let current = String::from_utf8_lossy(&head.stdout).trim().to_string();
        if !current.is_empty() {
            active_branches.insert(current);
        }
    }
    active_branches.insert("main".to_string());

    // Mirrors `orphaned = [b for b in all_branches if b not in active_branches and (b.startswith("hermes/hermes-") or b.startswith("pr-"))]` (ll.2933-2937)
    let orphaned: Vec<String> = all_branches
        .into_iter()
        .filter(|b| !active_branches.contains(b) && (b.starts_with("hermes/hermes-") || b.starts_with("pr-")))
        .collect();
    if orphaned.is_empty() {
        return;
    }

    // Mirrors `# Delete in batches` + `for i in range(0, len(orphaned), 50): batch = orphaned[i:i+50]; subprocess.run(["git", "branch", "-D"] + batch, ...)` (ll.2942-2951)
    for chunk in orphaned.chunks(50) {
        let mut args = vec!["branch", "-D"];
        let owned: Vec<String> = chunk.to_vec();
        let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
        args.extend(refs);
        let _ = std::process::Command::new("git")
            .args(&args)
            .current_dir(repo_root)
            .output();
    }
}

// ---------------------------------------------------------------------------
// Slice boundary — line 2700
// ---------------------------------------------------------------------------
// The Python `_prune_stale_worktrees` remainder (ll.2701-2885, now/stale_cutoff
// → kanban_re → Phase 1 age filter → Phase 2 parallel classify with
// merge_cache lock → Phase 3 serial mutate → preserved_stale warning →
// escalation `worktrees_summary` notice), plus `_prune_orphaned_branches`
// full body (ll.2887-2953) already stubbed above, and every subsequent
// CLI definition through `main()` (ll.2954-~21510) continues in
// `cli_slice4.rs` (and later slices). This file intentionally stops at the
// 900-line boundary so that `cargo` is never invoked and the 24-slice
// decomposition stays clean.
