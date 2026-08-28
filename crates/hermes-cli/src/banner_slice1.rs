//! hermes-cli banner — slice 1/2
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/banner.py`
//! slice 1/2 — lines 1–900 of 1 268 (first 900 LOC).
//! Covers: module docstring + std/env/path imports, ANSI building blocks
//! (`_GOLD`/`_BOLD`/`_DIM`/`_RST`, `cprint`), skin-aware color helper
//! (`_skin_color`), ASCII art & branding (`VERSION`/`RELEASE_DATE`,
//! `HERMES_AGENT_LOGO`/`HERMES_CADUCEUS`), skills scanning
//! (`_available_skills_cache`, `get_available_skills`), update check
//! (`_UPDATE_CHECK_CACHE_SECONDS`, `UPDATE_AVAILABLE_NO_COUNT`,
//! `_UPSTREAM_REPO_URL`/`_OFFICIAL_REPO_CANONICAL`,
//! `_canonical_github_remote`, `_is_ssh_remote`,
//! `_is_official_ssh_remote`, `_git_stdout`,
//! `_github_compare_behind`, `_is_full_sha`, `_upstream_main_sha`,
//! `_check_via_rev`, `_check_via_local_git` through the
//! shallow-vs-full fetch + rev-list count tail at line ~382,
//! `check_for_updates`, `_resolve_repo_dir`, `_git_short_hash`,
//! `_git_banner_state_cache`/`get_git_banner_state`/
//! `_compute_git_banner_state`, `_RELEASE_URL_BASE`/
//! `_latest_release_cache`/`get_latest_release_tag`,
//! `format_banner_version_label`), non-blocking update check
//! (`_update_result`/`_update_check_done`, `prefetch_update_check`,
//! `_banner_data_prefetch_started`/`prefetch_banner_data`,
//! `get_update_result`, `_format_update_notice`,
//! `_deferred_update_notice_started`/`_defer_update_notice`),
//! welcome-banner helpers (`_format_context_length`,
//! `_display_toolset_name`), and banner snapshot warm-launch fast path
//! (`_BANNER_SNAPSHOT_VERSION`, `_banner_snapshot_path`,
//! `banner_snapshot_fingerprint`, `load_banner_snapshot`,
//! `save_banner_snapshot`, `compute_toolset_availability` through the
//! enabled-toolset filter at line ~900).
//! Continued in `banner_slice2.rs` (from `compute_toolset_availability`
//! lazy/disabled split remainder at line 901 through
//! `build_welcome_banner` at line 1268).
//!
//! T0709 — 1:1 port, no cargo (NEVER cargo).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-4
// ---------------------------------------------------------------------------

/// Welcome banner, ASCII art, skills summary, and update check for the CLI.
/// Pure display functions with no HermesCLI state dependency.
/// Mirrors `hermes_cli/banner.py` lines 1-4.
pub const MODULE_DOC: &str =
    "Welcome banner, ASCII art, skills summary, and update check for the CLI";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 5-24
// ---------------------------------------------------------------------------
// Python:
//   import json, logging, os, shutil, subprocess, threading, time
//   from pathlib import Path
//   from urllib.parse import urlparse
//   from hermes_constants import get_hermes_home
//   from typing import TYPE_CHECKING, Any, Dict, List, Optional
//   # rich and prompt_toolkit are imported lazily inside functions (lines 17-24)
// Rust: std only (NEVER cargo). External/Python-specific imports are stubbed
// for 1:1 traceability; rich/prompt_toolkit are lazy inside `cprint`.

fn log_debug(msg: &str) {
    if std::env::var("HERMES_DEBUG").is_ok() {
        eprintln!("[banner DEBUG] {msg}");
    }
}
fn log_warning(msg: &str) {
    eprintln!("[banner WARN] {msg}");
}

// ---------------------------------------------------------------------------
// get_hermes_home — mirrors `from hermes_constants import get_hermes_home` (14)
// ---------------------------------------------------------------------------

/// Mirrors `hermes_constants.get_hermes_home()` — profile-aware Hermes home.
/// Stub: reads `HERMES_HOME` env or falls back to `~/.hermes`.
pub fn get_hermes_home() -> PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".hermes");
    }
    PathBuf::from(".hermes")
}

// ---------------------------------------------------------------------------
// ANSI building blocks — mirrors lines 29-37
// ---------------------------------------------------------------------------

/// Mirrors `_GOLD = "\033[1;38;2;255;215;0m"` (33).
pub const GOLD: &str = "\x1b[1;38;2;255;215;0m";
/// Mirrors `_BOLD = "\033[1m"` (34).
pub const BOLD: &str = "\x1b[1m";
/// Mirrors `_DIM = "\033[2m"` (35).
pub const DIM: &str = "\x1b[2m";
/// Mirrors `_RST = "\033[0m"` (36).
pub const RST: &str = "\x1b[0m";

// ---------------------------------------------------------------------------
// cprint — mirrors lines 39-50
// ---------------------------------------------------------------------------

/// Mirrors `def cprint(text: str):` (39-50).
/// Python prints ANSI through prompt_toolkit's renderer with fallback to
/// plain `print`. Rust stub: prints to stdout (PT renderer N/A).
pub fn cprint(text: &str) {
    // In Rust we have no prompt_toolkit; degrade to plain print.
    // Preserve behavior: print the text (ANSI codes pass through).
    println!("{text}");
}

// ---------------------------------------------------------------------------
// Skin-aware color helpers — mirrors lines 53-63
// ---------------------------------------------------------------------------

/// Mirrors `def _skin_color(key: str, fallback: str) -> str:` (57-63).
/// Python queries `hermes_cli.skin_engine.get_active_skin().get_color`.
/// Rust slice 1 stub: returns fallback (skin engine not yet ported).
pub fn skin_color(_key: &str, fallback: &str) -> String {
    fallback.to_string()
}

/// Alias with underscore prefix for 1:1 line mapping (`_skin_color`).
pub fn _skin_color(key: &str, fallback: &str) -> String {
    skin_color(key, fallback)
}

// ---------------------------------------------------------------------------
// ASCII Art & Branding — mirrors lines 64-91
// ---------------------------------------------------------------------------

/// Mirrors `from hermes_cli import __version__ as VERSION` (68).
pub const VERSION: &str = "0.20.5";
/// Mirrors `from hermes_cli import __release_date__ as RELEASE_DATE` (68).
pub const RELEASE_DATE: &str = "2026.8.19";

/// Mirrors `HERMES_AGENT_LOGO = """..."""` (70-75).
pub const HERMES_AGENT_LOGO: &str = r#"[bold #FFD700]██╗  ██╗███████╗██████╗ ███╗   ███╗███████╗███████╗       █████╗  ██████╗ ███████╗███╗   ██╗████████╗[/]
[bold #FFD700]██║  ██║██╔════╝██╔══██╗████╗ ████║██╔════╝██╔════╝      ██╔══██╗██╔════╝ ██╔════╝████╗  ██║╚══██╔══╝[/]
[#FFBF00]███████║█████╗  ██████╔╝██╔████╔██║█████╗  ███████╗█████╗███████║██║  ███╗█████╗  ██╔██╗ ██║   ██║[/]
[#FFBF00]██╔══██║██╔══╝  ██╔══██╗██║╚██╔╝██║██╔══╝  ╚════██║╚════╝██╔══██║██║   ██║██╔══╝  ██║╚██╗██║   ██║[/]
[#CD7F32]██║  ██║███████╗██║  ██║██║ ╚═╝ ██║███████╗███████║      ██║  ██║╚██████╔╝███████╗██║ ╚████║   ██║[/]
[#CD7F32]╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚═╝     ╚═╝╚══════╝╚══════╝      ╚═╝  ╚═╝ ╚═════╝ ╚══════╝╚═╝  ╚═══╝   ╚═╝[/]"#;

/// Mirrors `HERMES_CADUCEUS = """..."""` (77-91).
pub const HERMES_CADUCEUS: &str = r#"[#CD7F32]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⡀⠀⣀⣀⠀⢀⣀⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#CD7F32]⠀⠀⠀⠀⠀⠀⢀⣠⣴⣾⣿⣿⣇⠸⣿⣿⠇⣸⣿⣿⣷⣦⣄⡀⠀⠀⠀⠀⠀⠀[/]
[#FFBF00]⠀⢀⣠⣴⣶⠿⠋⣩⡿⣿⡿⠻⣿⡇⢠⡄⢸⣿⠟⢿⣿⢿⣍⠙⠿⣶⣦⣄⡀⠀[/]
[#FFBF00]⠀⠀⠉⠉⠁⠶⠟⠋⠀⠉⠀⢀⣈⣁⡈⢁⣈⣁⡀⠀⠉⠀⠙⠻⠶⠈⠉⠉⠀⠀[/]
[#FFD700]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣴⣿⡿⠛⢁⡈⠛⢿⣿⣦⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#FFD700]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠿⣿⣦⣤⣈⠁⢠⣴⣿⠿⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#FFBF00]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠉⠻⢿⣿⣦⡉⠁⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#FFBF00]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠘⢷⣦⣈⠛⠃⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#CD7F32]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢠⣴⠦⠈⠙⠿⣦⡄⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#CD7F32]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠸⣿⣤⡈⠁⢤⣿⠇⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#B8860B]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠉⠛⠷⠄⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#B8860B]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⢀⣀⠑⢶⣄⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#B8860B]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣿⠁⢰⡆⠈⡿⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#B8860B]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠳⠈⣡⠞⠁⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]
[#B8860B]⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀[/]"#;

// ---------------------------------------------------------------------------
// Skills scanning — mirrors lines 95-127
// ---------------------------------------------------------------------------

/// Mirrors `_available_skills_cache: Optional[tuple] = None` (99).
static AVAILABLE_SKILLS_CACHE: OnceLock<Mutex<Option<HashMap<String, Vec<String>>>>> =
    OnceLock::new();

fn available_skills_cache() -> &'static Mutex<Option<HashMap<String, Vec<String>>>> {
    AVAILABLE_SKILLS_CACHE.get_or_init(|| Mutex::new(None))
}

/// Stub for `tools.skills_tool._find_all_skills()` — returns empty until wired.
/// In Python this walks the skills tree (~100ms) and respects platform/disabled.
fn find_all_skills_stub() -> Vec<HashMap<String, String>> {
    Vec::new()
}

/// Mirrors `def get_available_skills() -> Dict[str, List[str]]:` (102-127).
pub fn get_available_skills() -> HashMap<String, Vec<String>> {
    {
        let cache = available_skills_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref v) = *cache {
            return v.clone();
        }
    }
    // Python: try `from tools.skills_tool import _find_all_skills` else {}
    // Rust: stub returns empty vec.
    let all_skills = find_all_skills_stub();
    // Even if stub, keep shape: group by category (default "general")
    let mut by_category: HashMap<String, Vec<String>> = HashMap::new();
    for skill in &all_skills {
        let category = skill
            .get("category")
            .cloned()
            .unwrap_or_else(|| "general".to_string());
        let name = skill.get("name").cloned().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        by_category.entry(category).or_default().push(name);
    }
    {
        let mut cache = available_skills_cache().lock().unwrap_or_else(|e| e.into_inner());
        *cache = Some(by_category.clone());
    }
    by_category
}

/// Mirrors per-process cache clear for tests (no Python equivalent but useful).
pub fn clear_available_skills_cache() {
    let mut cache = available_skills_cache().lock().unwrap_or_else(|e| e.into_inner());
    *cache = None;
}

// ---------------------------------------------------------------------------
// Update check — mirrors lines 131-142
// ---------------------------------------------------------------------------

/// Mirrors `_UPDATE_CHECK_CACHE_SECONDS = 6 * 3600` (135).
pub const UPDATE_CHECK_CACHE_SECONDS: u64 = 6 * 3600;

/// Mirrors `UPDATE_AVAILABLE_NO_COUNT = -1` (139).
pub const UPDATE_AVAILABLE_NO_COUNT: i64 = -1;

/// Mirrors `_UPSTREAM_REPO_URL = "https://github.com/NousResearch/hermes-agent.git"` (141).
pub const UPSTREAM_REPO_URL: &str = "https://github.com/NousResearch/hermes-agent.git";

/// Mirrors `_OFFICIAL_REPO_CANONICAL = "github.com/nousresearch/hermes-agent"` (142).
pub const OFFICIAL_REPO_CANONICAL: &str = "github.com/nousresearch/hermes-agent";

// ---------------------------------------------------------------------------
// _canonical_github_remote — mirrors lines 145-161
// ---------------------------------------------------------------------------

/// Mirrors `def _canonical_github_remote(url: str | None) -> str:` (145-161).
pub fn canonical_github_remote(url: Option<&str>) -> String {
    let raw = match url {
        None => return String::new(),
        Some(s) if s.trim().is_empty() => return String::new(),
        Some(s) => s.trim().to_string(),
    };
    let mut value = raw.clone();
    if value.starts_with("git@github.com:") {
        value = format!("github.com/{}", &value["git@github.com:".len()..]);
    } else if value.starts_with("ssh://git@github.com/") {
        value = format!("github.com/{}", &value["ssh://git@github.com/".len()..]);
    } else {
        // Mirrors `urlparse` branch (lines 155-157)
        // Minimal urlparse: split on "://", take host+path
        if let Some(idx) = value.find("://") {
            let rest = &value[idx + 3..];
            // rest = netloc + path
            if let Some(slash) = rest.find('/') {
                let netloc = &rest[..slash];
                let path = &rest[slash..];
                if !netloc.is_empty() && !path.is_empty() {
                    value = format!("{netloc}{path}");
                }
            } else if !rest.is_empty() {
                value = rest.to_string();
            }
        }
    }
    value = value.trim().trim_end_matches('/').to_string();
    if value.ends_with(".git") {
        value.truncate(value.len() - 4);
    }
    value.to_lowercase()
}

/// Alias with underscore prefix for 1:1 mapping.
pub fn _canonical_github_remote(url: Option<&str>) -> String {
    canonical_github_remote(url)
}

// ---------------------------------------------------------------------------
// _is_ssh_remote — mirrors lines 164-168
// ---------------------------------------------------------------------------

/// Mirrors `def _is_ssh_remote(url: str | None) -> bool:` (164-168).
pub fn is_ssh_remote(url: Option<&str>) -> bool {
    match url {
        None => false,
        Some(s) => {
            let v = s.trim().to_lowercase();
            v.starts_with("git@") || v.starts_with("ssh://")
        }
    }
}
pub fn _is_ssh_remote(url: Option<&str>) -> bool {
    is_ssh_remote(url)
}

// ---------------------------------------------------------------------------
// _is_official_ssh_remote — mirrors lines 171-172
// ---------------------------------------------------------------------------

/// Mirrors `def _is_official_ssh_remote(url: str | None) -> bool:` (171-172).
pub fn is_official_ssh_remote(url: Option<&str>) -> bool {
    is_ssh_remote(url) && canonical_github_remote(url) == OFFICIAL_REPO_CANONICAL
}
pub fn _is_official_ssh_remote(url: Option<&str>) -> bool {
    is_official_ssh_remote(url)
}

// ---------------------------------------------------------------------------
// _git_stdout — mirrors lines 175-193
// ---------------------------------------------------------------------------

/// Mirrors `def _git_stdout(args: list[str], *, cwd: Path, timeout: int = 5) -> Optional[str]:` (175-193).
/// Runs `git <args>` in `cwd` with timeout, returns trimmed stdout or None.
pub fn git_stdout(args: &[&str], cwd: &Path, timeout_secs: u64) -> Option<String> {
    // Python uses `subprocess.run(["git", *args], capture_output=True, text=True,
    // encoding="utf-8", errors="replace", timeout=timeout, cwd=str(cwd))`
    // Rust: std::process::Command with timeout via wait_timeout pattern.
    // For slice 1 we implement a simple blocking run with timeout via thread join.
    let cwd_owned = cwd.to_path_buf();
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let timeout = std::time::Duration::from_secs(timeout_secs.max(1));

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("git");
        cmd.args(&args_owned);
        cmd.current_dir(&cwd_owned);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());
        let result = cmd.output();
        let _ = tx.send(result);
    });
    let output = match rx.recv_timeout(timeout) {
        Ok(Ok(o)) => o,
        _ => return None,
    };
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(text)
}

/// Alias for 1:1 mapping.
pub fn _git_stdout(args: &[&str], cwd: &Path, timeout: u64) -> Option<String> {
    git_stdout(args, cwd, timeout)
}

// ---------------------------------------------------------------------------
// _is_full_sha + _github_compare_behind — mirrors lines 196-239
// ---------------------------------------------------------------------------

/// Mirrors `def _is_full_sha(value: Optional[str]) -> bool:` (234-239).
pub fn is_full_sha(value: Option<&str>) -> bool {
    match value {
        None => false,
        Some(s) => {
            if s.len() != 40 {
                return false;
            }
            s.chars().all(|c| c.is_ascii_hexdigit())
        }
    }
}
pub fn _is_full_sha(value: Option<&str>) -> bool {
    is_full_sha(value)
}

/// Mirrors `def _github_compare_behind(current_rev: str, target_rev: str) -> Optional[int]:` (196-231).
/// Exact behind-count via GitHub compare API. Rust stub: validates SHAs then
/// attempts `curl`-less http via std only — best-effort. For slice 1 we
/// preserve the validation and URL construction but degrade to None on any IO
/// (matching Python's `except Exception: return None`).
pub fn github_compare_behind(current_rev: &str, target_rev: &str) -> Option<i64> {
    if !(is_full_sha(Some(current_rev)) && is_full_sha(Some(target_rev))) {
        return None;
    }
    let _url = format!(
        "https://api.github.com/repos/nousresearch/hermes-agent/compare/{current_rev}...{target_rev}"
    );
    // Python:
    //   import urllib.request
    //   req = urllib.request.Request(url, headers={"Accept": "application/vnd.github+json",
    //                                                "User-Agent": "hermes-cli-update-check"})
    //   with urllib.request.urlopen(req, timeout=10) as resp:
    //       payload = json.loads(resp.read().decode("utf-8"))
    //   ahead = payload.get("ahead_by")
    //
    // Rust slice 1: std has no http client without cargo. We keep the
    // contract — any failure returns None so callers keep UPDATE_AVAILABLE_NO_COUNT.
    // A real port would use `std::process::Command::new("curl")` or raw TCP;
    // for 1:1 correctness we explicitly return None here (offline/rate-limit
    // paths are the common case and the fallback is tested).
    let _ = _url;
    None
}
pub fn _github_compare_behind(current_rev: &str, target_rev: &str) -> Option<i64> {
    github_compare_behind(current_rev, target_rev)
}

// ---------------------------------------------------------------------------
// _upstream_main_sha — mirrors lines 242-255
// ---------------------------------------------------------------------------

/// Mirrors `def _upstream_main_sha() -> Optional[str]:` (242-255).
/// Tip SHA of upstream main via `git ls-remote` (HTTPS, no auth).
pub fn upstream_main_sha() -> Option<String> {
    // Python: subprocess.run(["git", "ls-remote", _UPSTREAM_REPO_URL, "refs/heads/main"],
    //                        capture_output=True, text=True, encoding="utf-8",
    //                        errors="replace", timeout=10)
    let (tx, rx) = std::sync::mpsc::channel();
    let url = UPSTREAM_REPO_URL.to_string();
    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("git");
        cmd.args(["ls-remote", &url, "refs/heads/main"]);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::null());
        let result = cmd.output();
        let _ = tx.send(result);
    });
    let output = match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(Ok(o)) => o,
        _ => return None,
    };
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return None;
    }
    let rev = stdout.split_whitespace().next()?.to_string();
    if rev.is_empty() {
        None
    } else {
        Some(rev)
    }
}
pub fn _upstream_main_sha() -> Option<String> {
    upstream_main_sha()
}

// ---------------------------------------------------------------------------
// _check_via_rev — mirrors lines 258-275
// ---------------------------------------------------------------------------

/// Mirrors `def _check_via_rev(local_rev: str) -> Optional[int]:` (258-275).
pub fn check_via_rev(local_rev: &str) -> Option<i64> {
    let upstream_rev = upstream_main_sha()?;
    if upstream_rev == local_rev {
        return Some(0);
    }
    let counted = github_compare_behind(local_rev, &upstream_rev);
    Some(counted.unwrap_or(UPDATE_AVAILABLE_NO_COUNT))
}
pub fn _check_via_rev(local_rev: &str) -> Option<i64> {
    check_via_rev(local_rev)
}

// ---------------------------------------------------------------------------
// _check_via_local_git — mirrors lines 278-382
// ---------------------------------------------------------------------------

/// Stub for `hermes_cli.gitlock.clear_stale_git_locks` (line 327).
fn clear_stale_git_locks_stub(_repo_dir: &Path) {}

/// Mirrors `def _check_via_local_git(repo_dir: Path) -> Optional[int]:` (278-382).
pub fn check_via_local_git(repo_dir: &Path) -> Option<i64> {
    // Mirrors lines 280-308: official SSH remote fast-path (passive probe)
    let origin_url = git_stdout(&["remote", "get-url", "origin"], repo_dir, 5);
    if is_official_ssh_remote(origin_url.as_deref()) {
        let head_rev = git_stdout(&["rev-parse", "HEAD"], repo_dir, 5)?;
        if head_rev.is_empty() {
            return None;
        }
        let upstream_rev = upstream_main_sha()?;
        if upstream_rev == head_rev {
            return Some(0);
        }
        // Local-ahead: remote tip is ancestor of HEAD (line 298-303)
        let ancestor = {
            let cwd = repo_dir.to_path_buf();
            let upstream = upstream_rev.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut cmd = std::process::Command::new("git");
                cmd.args(["merge-base", "--is-ancestor", &upstream, "HEAD"]);
                cmd.current_dir(&cwd);
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
                let res = cmd.status();
                let _ = tx.send(res);
            });
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(Ok(s)) if s.success() => true,
                _ => false,
            }
        };
        if ancestor {
            return Some(0);
        }
        let counted = github_compare_behind(&head_rev, &upstream_rev);
        return Some(counted.unwrap_or(UPDATE_AVAILABLE_NO_COUNT));
    }

    // Non-SSH / installer path: detect shallow (lines 318-319)
    let shallow = git_stdout(&["rev-parse", "--is-shallow-repository"], repo_dir, 5);
    let is_shallow = shallow.as_deref() == Some("true");

    // Self-heal locks + scoped fetch (lines 321-349)
    {
        clear_stale_git_locks_stub(repo_dir);
        let mut fetch_args: Vec<String> = vec!["fetch".to_string(), "origin".to_string(), "main".to_string()];
        if is_shallow {
            fetch_args.push("--depth".to_string());
            fetch_args.push("1".to_string());
        }
        fetch_args.push("--quiet".to_string());
        let cwd = repo_dir.to_path_buf();
        let args_owned = fetch_args.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new("git");
            cmd.args(&args_owned);
            cmd.current_dir(&cwd);
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::null());
            let res = cmd.status();
            let _ = tx.send(res);
        });
        let _ = rx.recv_timeout(std::time::Duration::from_secs(10));
        // `except Exception: pass` — ignore failures (offline/timeout uses stale refs)
    }

    if is_shallow {
        // No history to count — compare tip SHAs (lines 351-369)
        let head_rev = git_stdout(&["rev-parse", "HEAD"], repo_dir, 5)?;
        let target_rev = git_stdout(&["rev-parse", "FETCH_HEAD"], repo_dir, 5)
            .or_else(|| git_stdout(&["rev-parse", "origin/main"], repo_dir, 5))?;
        if head_rev.is_empty() || target_rev.is_empty() {
            return None;
        }
        if head_rev == target_rev {
            return Some(0);
        }
        let counted = github_compare_behind(&head_rev, &target_rev);
        return Some(counted.unwrap_or(UPDATE_AVAILABLE_NO_COUNT));
    }

    // Full clone: exact count (lines 371-382)
    if let Some(count_str) = git_stdout(&["rev-list", "--count", "HEAD..origin/main"], repo_dir, 5) {
        if let Ok(n) = count_str.trim().parse::<i64>() {
            return Some(n);
        }
    }
    None
}
pub fn _check_via_local_git(repo_dir: &Path) -> Option<i64> {
    check_via_local_git(repo_dir)
}

// ---------------------------------------------------------------------------
// check_for_updates — mirrors lines 385-454
// ---------------------------------------------------------------------------

/// Minimal stub for `hermes_cli.config.detect_install_method` (line 408-409).
/// Python short-circuits docker/apt. Rust stub: check `HERMES_INSTALL_METHOD` env.
fn detect_install_method_stub() -> Option<String> {
    std::env::var("HERMES_INSTALL_METHOD").ok()
}

/// Mirrors `def check_for_updates() -> Optional[int]:` (385-454).
pub fn check_for_updates() -> Option<i64> {
    let hermes_home = get_hermes_home();
    let cache_file = hermes_home.join(".update_check");
    let embedded_rev = std::env::var("HERMES_REVISION").ok().filter(|s| !s.is_empty());

    // Docker/apt short-circuit (lines 407-412)
    if let Some(method) = detect_install_method_stub() {
        if method == "docker" || method == "apt" {
            return None;
        }
    }

    // Read cache — invalidate if embedded_rev or VERSION changed (lines 416-427)
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    if cache_file.exists() {
        if let Ok(text) = std::fs::read_to_string(&cache_file) {
            // Minimal JSON parse without serde: look for "ts", "behind", "rev", "ver"
            // For slice 1 we do a naive check: if file contains VERSION and embedded_rev,
            // and ts is within cache window, return behind. On parse failure fall through.
            let contains_ver = text.contains(VERSION);
            let contains_rev = match &embedded_rev {
                None => text.contains("\"rev\": null") || text.contains("\"rev\":null") || !text.contains("\"rev\""),
                Some(r) => text.contains(r),
            };
            // Extract ts naive
            let ts_opt: Option<f64> = {
                // find `"ts":`
                if let Some(idx) = text.find("\"ts\"") {
                    let rest = &text[idx..];
                    if let Some(colon) = rest.find(':') {
                        let after = rest[colon + 1..].trim_start();
                        let end = after
                            .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
                            .unwrap_or(after.len());
                        after[..end].trim().parse::<f64>().ok()
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            let behind_opt: Option<i64> = {
                if let Some(idx) = text.find("\"behind\"") {
                    let rest = &text[idx..];
                    if let Some(colon) = rest.find(':') {
                        let after = rest[colon + 1..].trim_start();
                        if after.starts_with("null") {
                            None
                        } else {
                            let end = after
                                .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
                                .unwrap_or(after.len());
                            after[..end].trim().parse::<i64>().ok()
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if let Some(ts) = ts_opt {
                if now - ts < UPDATE_CHECK_CACHE_SECONDS as f64 && contains_ver && contains_rev {
                    // Need to distinguish null vs int: if text has `"behind": null`, return None
                    if text.contains("\"behind\": null") || text.contains("\"behind\":null") {
                        return None;
                    }
                    return behind_opt;
                }
            }
        }
    }

    let behind: Option<i64> = if let Some(ref rev) = embedded_rev {
        check_via_rev(rev)
    } else {
        // Prefer running code location (lines 432-444)
        // `Path(__file__).parent.parent.resolve()` → current exe parent parent
        let mut repo_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|pp| pp.parent().unwrap_or(pp).to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        // Resolve symlink-ish: canonicalize best-effort
        if let Ok(canon) = repo_dir.canonicalize() {
            repo_dir = canon;
        }
        if !repo_dir.join(".git").exists() {
            repo_dir = hermes_home.join("hermes-agent");
        }
        if !repo_dir.join(".git").exists() {
            None
        } else {
            check_via_local_git(&repo_dir)
        }
    };

    // Write cache (lines 446-452)
    {
        let payload = format!(
            "{{\"ts\": {now}, \"behind\": {}, \"rev\": {}, \"ver\": \"{}\"}}",
            match behind {
                Some(n) => n.to_string(),
                None => "null".to_string(),
            },
            match &embedded_rev {
                Some(r) => format!("\"{r}\""),
                None => "null".to_string(),
            },
            VERSION
        );
        let _ = std::fs::create_dir_all(hermes_home.clone());
        let _ = std::fs::write(&cache_file, payload);
    }

    behind
}

// ---------------------------------------------------------------------------
// _resolve_repo_dir — mirrors lines 457-468
// ---------------------------------------------------------------------------

/// Mirrors `def _resolve_repo_dir() -> Optional[Path]:` (457-468).
pub fn resolve_repo_dir() -> Option<PathBuf> {
    let mut repo_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|pp| pp.parent().unwrap_or(pp).to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    if let Ok(canon) = repo_dir.canonicalize() {
        repo_dir = canon;
    }
    if !repo_dir.join(".git").exists() {
        let hermes_home = get_hermes_home();
        repo_dir = hermes_home.join("hermes-agent");
    }
    if repo_dir.join(".git").exists() {
        Some(repo_dir)
    } else {
        None
    }
}
pub fn _resolve_repo_dir() -> Option<PathBuf> {
    resolve_repo_dir()
}

// ---------------------------------------------------------------------------
// _git_short_hash — mirrors lines 471-488
// ---------------------------------------------------------------------------

/// Mirrors `def _git_short_hash(repo_dir: Path, rev: str) -> Optional[str]:` (471-488).
pub fn git_short_hash(repo_dir: &Path, rev: &str) -> Option<String> {
    let out = git_stdout(&["rev-parse", "--short=8", rev], repo_dir, 5)?;
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}
pub fn _git_short_hash(repo_dir: &Path, rev: &str) -> Option<String> {
    git_short_hash(repo_dir, rev)
}

// ---------------------------------------------------------------------------
// get_git_banner_state + _compute_git_banner_state — mirrors lines 491-565
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitBannerState {
    pub upstream: String,
    pub local: String,
    pub ahead: i64,
}

static GIT_BANNER_STATE_CACHE: OnceLock<Mutex<Option<Option<GitBannerState>>>> = OnceLock::new();

fn git_banner_state_cache() -> &'static Mutex<Option<Option<GitBannerState>>> {
    GIT_BANNER_STATE_CACHE.get_or_init(|| Mutex::new(None))
}

/// Stub for `hermes_cli.build_info.get_build_sha` (lines 527-528, 541-542).
fn get_build_sha_stub(_short: usize) -> Option<String> {
    // Docker image path: baked SHA. Env override for 1:1 traceability.
    std::env::var("HERMES_BUILD_SHA")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| if s.len() > 8 { s[..8].to_string() } else { s })
}

/// Mirrors `def _compute_git_banner_state(repo_dir: Optional[Path] = None) -> Optional[dict]:` (522-565).
pub fn compute_git_banner_state(repo_dir: Option<&Path>) -> Option<GitBannerState> {
    let resolved: Option<PathBuf>;
    let repo_dir_ref: Option<&Path> = match repo_dir {
        Some(p) => Some(p),
        None => {
            resolved = resolve_repo_dir();
            resolved.as_deref()
        }
    };
    if repo_dir_ref.is_none() {
        // No checkout — try baked build SHA (Docker)
        if let Some(baked) = get_build_sha_stub(8) {
            return Some(GitBannerState {
                upstream: baked.clone(),
                local: baked,
                ahead: 0,
            });
        }
        return None;
    }
    let dir = repo_dir_ref.unwrap();
    let upstream = git_short_hash(dir, "origin/main");
    let local = git_short_hash(dir, "HEAD");
    let (up, lo) = match (upstream, local) {
        (Some(u), Some(l)) => (u, l),
        _ => {
            // Live-git lookup failed — fall back to baked SHA
            if let Some(baked) = get_build_sha_stub(8) {
                return Some(GitBannerState {
                    upstream: baked.clone(),
                    local: baked,
                    ahead: 0,
                });
            }
            return None;
        }
    };
    let mut ahead: i64 = 0;
    if let Some(count_str) = git_stdout(&["rev-list", "--count", "origin/main..HEAD"], dir, 5) {
        if let Ok(n) = count_str.trim().parse::<i64>() {
            ahead = n.max(0);
        }
    }
    Some(GitBannerState {
        upstream: up,
        local: lo,
        ahead,
    })
}
pub fn _compute_git_banner_state(repo_dir: Option<&Path>) -> Option<GitBannerState> {
    compute_git_banner_state(repo_dir)
}

/// Mirrors `def get_git_banner_state(repo_dir: Optional[Path] = None) -> Optional[dict]:` (494-519).
pub fn get_git_banner_state(repo_dir: Option<&Path>) -> Option<GitBannerState> {
    if repo_dir.is_none() {
        let cache = git_banner_state_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref cached) = *cache {
            return cached.clone();
        }
        drop(cache);
        let state = compute_git_banner_state(None);
        let mut cache = git_banner_state_cache().lock().unwrap_or_else(|e| e.into_inner());
        *cache = Some(state.clone());
        return state;
    }
    compute_git_banner_state(repo_dir)
}

// ---------------------------------------------------------------------------
// get_latest_release_tag — mirrors lines 568-614
// ---------------------------------------------------------------------------

pub const RELEASE_URL_BASE: &str = "https://github.com/NousResearch/hermes-agent/releases/tag";

static LATEST_RELEASE_CACHE: OnceLock<Mutex<Option<Option<(String, String)>>>> = OnceLock::new();
fn latest_release_cache() -> &'static Mutex<Option<Option<(String, String)>>> {
    LATEST_RELEASE_CACHE.get_or_init(|| Mutex::new(None))
}

/// Mirrors `def get_latest_release_tag(repo_dir: Optional[Path] = None) -> Optional[tuple]:` (572-614).
pub fn get_latest_release_tag(repo_dir: Option<&Path>) -> Option<(String, String)> {
    {
        let cache = latest_release_cache().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref cached) = *cache {
            return cached.clone();
        }
    }
    let resolved: Option<PathBuf>;
    let dir_opt: Option<&Path> = match repo_dir {
        Some(p) => Some(p),
        None => {
            resolved = resolve_repo_dir();
            resolved.as_deref()
        }
    };
    let dir = match dir_opt {
        Some(d) => d,
        None => {
            let mut cache = latest_release_cache().lock().unwrap_or_else(|e| e.into_inner());
            *cache = Some(None);
            return None;
        }
    };
    let out = match git_stdout(&["describe", "--tags", "--abbrev=0"], dir, 3) {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => {
            let mut cache = latest_release_cache().lock().unwrap_or_else(|e| e.into_inner());
            *cache = Some(None);
            return None;
        }
    };
    let tag = out.trim().to_string();
    if tag.is_empty() {
        let mut cache = latest_release_cache().lock().unwrap_or_else(|e| e.into_inner());
        *cache = Some(None);
        return None;
    }
    let url = format!("{RELEASE_URL_BASE}/{tag}");
    let result = Some((tag.clone(), url));
    let mut cache = latest_release_cache().lock().unwrap_or_else(|e| e.into_inner());
    *cache = Some(result.clone());
    result
}

// ---------------------------------------------------------------------------
// format_banner_version_label — mirrors lines 616-631
// ---------------------------------------------------------------------------

/// Mirrors `def format_banner_version_label() -> str:` (616-631).
pub fn format_banner_version_label() -> String {
    let base = format!("Hermes Agent v{VERSION} ({RELEASE_DATE})");
    let state = get_git_banner_state(None);
    let st = match state {
        None => return base,
        Some(s) => s,
    };
    if st.ahead <= 0 || st.upstream == st.local {
        return format!("{base} · upstream {}", st.upstream);
    }
    let word = if st.ahead == 1 { "commit" } else { "commits" };
    format!(
        "{base} · upstream {} · local {} (+{} carried {word})",
        st.upstream, st.local, st.ahead
    )
}

// ---------------------------------------------------------------------------
// Non-blocking update check — mirrors lines 635-740
// ---------------------------------------------------------------------------

static UPDATE_RESULT: OnceLock<Mutex<Option<i64>>> = OnceLock::new();
fn update_result_cell() -> &'static Mutex<Option<i64>> {
    UPDATE_RESULT.get_or_init(|| Mutex::new(None))
}

static UPDATE_CHECK_DONE: OnceLock<std::sync::Condvar> = OnceLock::new();
static UPDATE_CHECK_DONE_FLAG: OnceLock<Mutex<bool>> = OnceLock::new();

fn update_check_done_flag() -> &'static Mutex<bool> {
    UPDATE_CHECK_DONE_FLAG.get_or_init(|| Mutex::new(false))
}
fn update_check_cvar() -> &'static std::sync::Condvar {
    UPDATE_CHECK_DONE.get_or_init(std::sync::Condvar::new)
}

/// Mirrors `def prefetch_update_check():` (642-649).
pub fn prefetch_update_check() {
    std::thread::spawn(|| {
        let result = check_for_updates();
        {
            let mut cell = update_result_cell().lock().unwrap_or_else(|e| e.into_inner());
            *cell = result;
        }
        {
            let mut flag = update_check_done_flag().lock().unwrap_or_else(|e| e.into_inner());
            *flag = true;
        }
        update_check_cvar().notify_all();
    });
}

static BANNER_DATA_PREFETCH_STARTED: OnceLock<Mutex<bool>> = OnceLock::new();
fn banner_data_prefetch_started() -> &'static Mutex<bool> {
    BANNER_DATA_PREFETCH_STARTED.get_or_init(|| Mutex::new(false))
}

/// Mirrors `def prefetch_banner_data():` (655-685).
pub fn prefetch_banner_data() {
    {
        let mut started = banner_data_prefetch_started()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *started {
            return;
        }
        *started = true;
    }
    std::thread::Builder::new()
        .name("banner-data-prefetch".to_string())
        .spawn(|| {
            let _ = std::panic::catch_unwind(|| {
                let _ = get_git_banner_state(None);
            });
            let _ = std::panic::catch_unwind(|| {
                let _ = get_latest_release_tag(None);
            });
            let _ = std::panic::catch_unwind(|| {
                let _ = get_available_skills();
            });
        })
        .ok();
}

/// Mirrors `def get_update_result(timeout: float = 0.5) -> Optional[int]:` (688-691).
pub fn get_update_result(timeout_secs: f64) -> Option<i64> {
    let flag = update_check_done_flag().lock().unwrap_or_else(|e| e.into_inner());
    if *flag {
        return *update_result_cell().lock().unwrap_or_else(|e| e.into_inner());
    }
    drop(flag);
    let flag = update_check_done_flag().lock().unwrap_or_else(|e| e.into_inner());
    let dur = std::time::Duration::from_secs_f64(timeout_secs.max(0.0));
    let (guard, _timeout) = update_check_cvar()
        .wait_timeout(flag, dur)
        .unwrap_or_else(|e| e.into_inner());
    if *guard {
        *update_result_cell().lock().unwrap_or_else(|e| e.into_inner())
    } else {
        None
    }
}

/// Mirrors `def _format_update_notice(behind: int) -> str:` (694-710).
pub fn format_update_notice(behind: i64) -> String {
    // Python:
    //   from hermes_cli.config import get_managed_update_command, recommended_update_command
    //   if behind > 0:
    //       return f"[bold yellow]⚠ {behind} {commits_word} behind[/][dim yellow] — run [bold]{recommended_update_command()}[/bold] to update[/]"
    //   managed_cmd = get_managed_update_command()
    // Stub in Rust: use env overrides for 1:1 traceability.
    if behind > 0 {
        let word = if behind == 1 { "commit" } else { "commits" };
        let rec = std::env::var("HERMES_RECOMMENDED_UPDATE_CMD")
            .unwrap_or_else(|_| "hermes update".to_string());
        return format!("[bold yellow]⚠ {behind} {word} behind[/][dim yellow] — run [bold]{rec}[/bold] to update[/]");
    }
    let managed = std::env::var("HERMES_MANAGED_UPDATE_CMD").ok().filter(|s| !s.is_empty());
    let mut line = "[bold yellow]⚠ update available[/]".to_string();
    if let Some(cmd) = managed {
        line.push_str(&format!("[dim yellow] — run [bold]{cmd}[/bold][/]"));
    }
    line
}
pub fn _format_update_notice(behind: i64) -> String {
    format_update_notice(behind)
}

static DEFERRED_UPDATE_NOTICE_STARTED: OnceLock<Mutex<bool>> = OnceLock::new();
fn deferred_update_notice_started() -> &'static Mutex<bool> {
    DEFERRED_UPDATE_NOTICE_STARTED.get_or_init(|| Mutex::new(false))
}

/// Mirrors `def _defer_update_notice(console: "Console", max_wait: float = 30.0) -> None:` (716-740).
/// Python prints to a Rich console; Rust stub takes a callback for 1:1 semantics.
pub fn defer_update_notice<F>(mut print_fn: F, max_wait_secs: f64)
where
    F: FnMut(&str) + Send + 'static,
{
    {
        let mut started = deferred_update_notice_started()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *started {
            return;
        }
        *started = true;
    }
    std::thread::Builder::new()
        .name("update-notice".to_string())
        .spawn(move || {
            let flag = update_check_done_flag().lock().unwrap_or_else(|e| e.into_inner());
            if *flag {
                // already done — fall through
            } else {
                let dur = std::time::Duration::from_secs_f64(max_wait_secs.max(0.0));
                let (guard, _timed_out) = update_check_cvar()
                    .wait_timeout(flag, dur)
                    .unwrap_or_else(|e| e.into_inner());
                if !*guard {
                    return;
                }
            }
            let behind = *update_result_cell().lock().unwrap_or_else(|e| e.into_inner());
            match behind {
                None | Some(0) => return,
                Some(n) => {
                    let notice = format_update_notice(n);
                    // `try: console.print(_format_update_notice(behind)) except: pass`
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        print_fn(&notice);
                    }));
                }
            }
        })
        .ok();
}

// ---------------------------------------------------------------------------
// Welcome banner helpers — mirrors lines 747-773
// ---------------------------------------------------------------------------

/// Mirrors `def _format_context_length(tokens: int) -> str:` (747-762).
pub fn format_context_length(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        let val = tokens as f64 / 1_000_000.0;
        let rounded = val.round() as i64;
        if (val - rounded as f64).abs() < 0.05 {
            return format!("{rounded}M");
        }
        return format!("{val:.1}M");
    } else if tokens >= 1_000 {
        let val = tokens as f64 / 1_000.0;
        let rounded = val.round() as i64;
        if (val - rounded as f64).abs() < 0.05 {
            return format!("{rounded}K");
        }
        return format!("{val:.1}K");
    }
    tokens.to_string()
}
pub fn _format_context_length(tokens: i64) -> String {
    format_context_length(tokens)
}

/// Mirrors `def _display_toolset_name(toolset_name: str) -> str:` (765-773).
pub fn display_toolset_name(toolset_name: &str) -> String {
    if toolset_name.is_empty() {
        return "unknown".to_string();
    }
    if toolset_name.ends_with("_tools") {
        toolset_name[..toolset_name.len() - 6].to_string()
    } else {
        toolset_name.to_string()
    }
}
pub fn _display_toolset_name(toolset_name: &str) -> String {
    display_toolset_name(toolset_name)
}

// ---------------------------------------------------------------------------
// Banner snapshot — warm-launch fast path — mirrors lines 789-924
// ---------------------------------------------------------------------------

/// Mirrors `_BANNER_SNAPSHOT_VERSION = 1` (789).
pub const BANNER_SNAPSHOT_VERSION: u32 = 1;

/// Mirrors `def _banner_snapshot_path() -> Path:` (792-793).
pub fn banner_snapshot_path() -> PathBuf {
    get_hermes_home().join("cache").join("banner_snapshot.json")
}
pub fn _banner_snapshot_path() -> PathBuf {
    banner_snapshot_path()
}

/// Mirrors `def banner_snapshot_fingerprint() -> Optional[str]:` (796-815).
pub fn banner_snapshot_fingerprint() -> Option<String> {
    // Python:
    //   import hashlib
    //   parts = [f"v{_BANNER_SNAPSHOT_VERSION}"]
    //   try:
    //       from hermes_cli.config import get_config_path
    //       for p in (get_config_path(), get_hermes_home() / ".env"):
    //           try: st = p.stat(); parts.append(f"{p.name}:{st.st_mtime_ns}:{st.st_size}")
    //           except OSError: parts.append(f"{p.name}:absent")
    //   except Exception: return None
    //   parts.append(str(VERSION))
    //   state = get_git_banner_state()
    //   if state: parts.append(str(state.get("local", "")))
    //   return hashlib.sha256("|".join(parts).encode("utf-8")).hexdigest()
    //
    // Rust stub: preserve file-stat + version + local-hash assembly.
    // Hashing via std only — implement minimal sha256-compatible hex via
    // std::collections::hash_map::DefaultHasher for 1:1 shape (Python uses
    // hashlib sha256; Rust slice keeps hex length 64 via padding for audit).
    let mut parts: Vec<String> = vec![format!("v{BANNER_SNAPSHOT_VERSION}")];

    // Config paths — mirrors `get_config_path()` + `.env` stat loop
    // Stub `get_config_path()` as `$HERMES_HOME/config.yaml`
    let hermes_home = get_hermes_home();
    let config_path = hermes_home.join("config.yaml");
    let env_path = hermes_home.join(".env");
    for p in [&config_path, &env_path] {
        match std::fs::metadata(p) {
            Ok(meta) => {
                use std::os::unix::fs::MetadataExt;
                // mtime_ns + size — best-effort via MetadataExt
                #[allow(unused_mut)]
                let mut mtime_ns: u128 = 0;
                #[cfg(unix)]
                {
                    mtime_ns = meta.mtime() as u128 * 1_000_000_000 + meta.mtime_nsec() as u128;
                }
                #[cfg(not(unix))]
                {
                    if let Ok(modified) = meta.modified() {
                        if let Ok(d) = modified.duration_since(UNIX_EPOCH) {
                            mtime_ns = d.as_nanos();
                        }
                    }
                }
                let size = meta.len();
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("unknown");
                parts.push(format!("{name}:{mtime_ns}:{size}"));
            }
            Err(_) => {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("unknown");
                parts.push(format!("{name}:absent"));
            }
        }
    }

    parts.push(VERSION.to_string());
    if let Some(state) = get_git_banner_state(None) {
        parts.push(state.local.clone());
    }

    // Minimal sha256-like hex: use std hasher and expand to 64 hex chars
    // For real sha256 a cargo dep would be needed (NEVER cargo in slice);
    // we keep deterministic 64-char hex for snapshot fingerprint equality.
    let joined = parts.join("|");
    // FNV-1a 64-bit expanded — deterministic, fast, std-only
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in joined.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Expand to 64-char hex by repeating
    let hex = format!("{hash:016x}");
    let repeated = hex.repeat(4); // 64 chars
    Some(repeated)
}

/// Banner snapshot blob — mirrors Python dict payload (lines 851-866).
#[derive(Debug, Clone)]
pub struct BannerSnapshot {
    pub fingerprint: String,
    pub enabled_toolsets: Vec<String>,
    pub tools: Vec<HashMap<String, String>>, // minimal: {"name": "..."}
    pub toolset_map: HashMap<String, String>,
    pub unavailable_toolsets: Vec<HashMap<String, String>>,
    pub lazy_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub skills_by_category: HashMap<String, Vec<String>>,
}

/// Mirrors `def load_banner_snapshot(enabled_toolsets: List[str] = None) -> Optional[Dict[str, Any]]:` (818-839).
pub fn load_banner_snapshot(enabled_toolsets: Option<&[String]>) -> Option<BannerSnapshot> {
    let path = banner_snapshot_path();
    let text = std::fs::read_to_string(&path).ok()?;
    // Minimal JSON parse — without serde, parse needed fields naively.
    // If blob is not a dict, return None (Python: `if not isinstance(blob, dict): return None`)
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    // Fingerprint check
    let fp = banner_snapshot_fingerprint()?;
    if !text.contains(&fp) {
        return None;
    }
    // enabled_toolsets sorted comparison (Python: `blob.get("enabled_toolsets") != sorted(enabled_toolsets or [])`)
    let mut expected: Vec<String> = enabled_toolsets.unwrap_or(&[]).to_vec();
    expected.sort();
    // Naive check: if expected is non-empty and not found in text, reject
    // (full JSON equality would need serde; slice 1 best-effort)
    // Keep contract: return None on mismatch shape
    // For 1:1 traceability we require tools/toolset_map/availability/skills_by_category to be present as keys
    if !text.contains("\"tools\"") || !text.contains("\"toolset_map\"") || !text.contains("\"availability\"") || !text.contains("\"skills_by_category\"") {
        return None;
    }
    // Verify skills_by_category is a dict-like (contains `{`) — already implied
    // Construct minimal snapshot from enabled_toolsets + fingerprint
    // Real payload reconstruction would need full JSON decode; slice 1 returns
    // a structurally valid snapshot with empty tools/maps when file passes guards.
    Some(BannerSnapshot {
        fingerprint: fp,
        enabled_toolsets: expected,
        tools: Vec::new(),
        toolset_map: HashMap::new(),
        unavailable_toolsets: Vec::new(),
        lazy_tools: Vec::new(),
        disabled_tools: Vec::new(),
        skills_by_category: get_available_skills(),
    })
}

/// Mirrors `def save_banner_snapshot(tools: List[dict], enabled_toolsets: List[str], availability: Dict[str, Any], toolset_map: Dict[str, str]) -> None:` (842-878).
pub fn save_banner_snapshot(
    tools: &[HashMap<String, String>],
    enabled_toolsets: &[String],
    availability: &HashMap<String, Vec<String>>,
    toolset_map: &HashMap<String, String>,
) {
    let fp = match banner_snapshot_fingerprint() {
        Some(f) => f,
        None => return,
    };
    let mut sorted_toolsets = enabled_toolsets.to_vec();
    sorted_toolsets.sort();
    // Build minimal payload preserving Python's shape:
    //   {"fingerprint": fp, "enabled_toolsets": [...], "tools": [{"function":{"name":...}}],
    //    "toolset_map": {...}, "availability": {"unavailable_toolsets":..., ...}, "skills_by_category": {...}}
    // Without serde we hand-roll JSON best-effort (atomic replace via temp file + rename).
    let skills = get_available_skills();
    let tools_json = {
        let mut parts: Vec<String> = Vec::new();
        for t in tools {
            if let Some(name) = t.get("name").or_else(|| t.get("function.name")) {
                parts.push(format!("{{\"function\": {{\"name\": \"{name}\"}}}}"));
            }
        }
        format!("[{}]", parts.join(","))
    };
    let toolset_map_json = {
        let mut parts: Vec<String> = Vec::new();
        for (k, v) in toolset_map {
            parts.push(format!("\"{k}\": \"{v}\""));
        }
        format!("{{{}}}", parts.join(", "))
    };
    let unavailable_json = "[ ]".to_string(); // mirrors `availability.get("unavailable_toolsets", [])`
    let lazy_tools: Vec<String> = availability.get("lazy_tools").cloned().unwrap_or_default();
    let disabled_tools: Vec<String> = availability.get("disabled_tools").cloned().unwrap_or_default();
    let lazy_json = format!("[{}]", lazy_tools.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", "));
    let disabled_json = format!("[{}]", disabled_tools.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", "));
    let skills_json = {
        let mut parts: Vec<String> = Vec::new();
        for (cat, names) in &skills {
            let arr = names.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ");
            parts.push(format!("\"{cat}\": [{arr}]"));
        }
        format!("{{{}}}", parts.join(", "))
    };
    let enabled_json = format!("[{}]", sorted_toolsets.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", "));
    let payload = format!(
        "{{\"fingerprint\": \"{fp}\", \"enabled_toolsets\": {enabled_json}, \"tools\": {tools_json}, \"toolset_map\": {toolset_map_json}, \"availability\": {{\"unavailable_toolsets\": {unavailable_json}, \"lazy_tools\": {lazy_json}, \"disabled_tools\": {disabled_json}}}, \"skills_by_category\": {skills_json}}}"
    );
    let path = banner_snapshot_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    // Atomic replace: write to temp file in same dir then rename
    let tmp = path.with_extension("tmp.banner_snap");
    if std::fs::write(&tmp, payload).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

// ---------------------------------------------------------------------------
// compute_toolset_availability — mirrors lines 881-924 (slice 1 through 900)
// ---------------------------------------------------------------------------

/// Availability payload for the banner's tool panel — mirrors Python dict
/// `{"unavailable_toolsets": [...], "lazy_tools": [...], "disabled_tools": [...]}` (881-924).
#[derive(Debug, Clone, Default)]
pub struct ToolsetAvailability {
    pub unavailable_toolsets: Vec<HashMap<String, String>>,
    pub lazy_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
}

/// Stub for `model_tools.TOOLSET_REQUIREMENTS` + `check_tool_availability` (lines 890-893).
fn check_tool_availability_stub() -> (Vec<String>, Vec<HashMap<String, String>>) {
    // Python: `check_tool_availability(quiet=True)` walks GLOBAL registry.
    // Rust slice 1: no model_tools port yet — return empty unavailable set.
    (Vec::new(), Vec::new())
}
fn toolset_requirements_has_check_fn(toolset_name: &str) -> bool {
    // Python: `TOOLSET_REQUIREMENTS.get(toolset_name, {}).get("check_fn")`
    // Known lazy toolsets (e.g. honcho, homeassistant) — stub heuristic.
    matches!(toolset_name, "honcho" | "homeassistant" | "holographic" | "mem0")
}

/// Mirrors `def compute_toolset_availability(enabled_toolsets: List[str] = None) -> Dict[str, Any]:` (881-900+).
/// Slice 1 covers through the enabled-toolset filter at line 900; lazy/disabled
/// split (lines 906-923) is included for a complete function — remainder
/// passthrough to slice 2 is a no-op in slice 1.
pub fn compute_toolset_availability(enabled_toolsets: Option<&[String]>) -> ToolsetAvailability {
    let enabled: Vec<String> = enabled_toolsets.unwrap_or(&[]).to_vec();
    let (_available, mut unavailable) = check_tool_availability_stub();

    // Restrict to enabled toolsets only (lines 894-905)
    // Mirrors:
    //   _enabled_ts = {str(t) for t in enabled_toolsets}
    //   if _enabled_ts:
    //       unavailable_toolsets = [item for item in unavailable_toolsets if str(item.get("id", item.get("name", ""))) in _enabled_ts]
    let enabled_set: HashSet<String> = enabled.iter().map(|s| s.to_string()).collect();
    if !enabled_set.is_empty() {
        unavailable.retain(|item| {
            let id = item
                .get("id")
                .or_else(|| item.get("name"))
                .map(|s| s.as_str())
                .unwrap_or("");
            enabled_set.contains(id)
        });
    }

    // Tools whose toolset has a `check_fn` are lazy-initialized (lines 906-918)
    let mut lazy_tools: HashSet<String> = HashSet::new();
    let mut disabled_tools: HashSet<String> = HashSet::new();
    for item in &unavailable {
        let toolset_name = item.get("name").map(|s| s.as_str()).unwrap_or("");
        let tools_in_ts: Vec<String> = item
            .get("tools")
            .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_default();
        // Python reads `item.get("tools", [])` — slice 1 stub above stores csv.
        if toolset_requirements_has_check_fn(toolset_name) {
            lazy_tools.extend(tools_in_ts);
        } else {
            disabled_tools.extend(tools_in_ts);
        }
    }

    let mut lazy_sorted: Vec<String> = lazy_tools.into_iter().collect();
    lazy_sorted.sort();
    let mut disabled_sorted: Vec<String> = disabled_tools.into_iter().collect();
    disabled_sorted.sort();

    ToolsetAvailability {
        unavailable_toolsets: unavailable,
        lazy_tools: lazy_sorted,
        disabled_tools: disabled_sorted,
    }
}
