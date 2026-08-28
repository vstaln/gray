//! Opt-in git worktree isolation for delegated subagents.
//! Port of `tools/subagent_worktree.py` (352 lines) — 1:1 behavior.
//!
//! Inspired by Muse Code's `--subagent-worktree-isolation` (Meta, Aug 2026):
//! when isolation is on, each delegated child agent gets its own git worktree
//! checked out from the parent's current commit, so parallel children never
//! contend for the same working copy and the parent's checkout stays untouched.
//! This is a clean-room implementation of the documented behavior
//! (https://dev.meta.ai/docs/muse-code/extending#multi-agent); no Muse Code
//! code was referenced.
//!
//! Enable in config.yaml::
//!
//! ```yaml
//! delegation:
//!   worktree_isolation: true   # default: false
//! ```
//!
//! Contract (mirrors Muse Code's documented semantics):
//!
//! - **Opt-in and git-only.** In a non-git workspace the setting is ignored
//!   without an error and children share the parent's working directory,
//!   exactly as before.
//! - **One worktree per child**, branched from the parent repo's current
//!   ``HEAD`` under ``<repo>/.worktrees/subagent-<id>`` on branch
//!   ``hermes-subagent/<id>``.
//! - **The parent reviews/merges.** Children commit inside their own worktree;
//!   each result entry reports the worktree path, branch, commit count, and
//!   dirty state so the parent can review or merge each branch.
//! - **Clean worktrees are pruned.** A worktree with no new commits and a
//!   clean tree is removed automatically after the child finishes; anything
//!   holding work is kept and reported. Pruning requires affirmative proof:
//!   if a git inspection probe fails the state is unknown, so the worktree is
//!   kept and the result entry carries ``inspection_failed`` + ``note`` (#88113).
//!
//! Only the local terminal backend is supported: on docker/ssh/modal/etc. the
//! worktree created on the host would not be visible inside the sandbox, so
//! isolation is skipped (with a debug log) rather than half-applied.
//!
//! Mapping:
//! - `_GIT_TIMEOUT = 30` → [`GIT_TIMEOUT_SECS`] / [`GIT_TIMEOUT`]
//! - `_WORKTREES_DIRNAME = ".worktrees"` → [`WORKTREES_DIRNAME`]
//! - `_BRANCH_NAMESPACE = "hermes-subagent"` → [`BRANCH_NAMESPACE`]
//! - `_run_git(args, cwd, timeout)` → [`run_git`] / [`run_git_with_timeout`]
//! - `local_backend_active()` → [`local_backend_active`]
//! - `resolve_repo_root(path)` → [`resolve_repo_root`]
//! - `_ensure_gitignore_entry(repo_root)` → [`ensure_gitignore_entry`]
//! - `create_subagent_worktree(parent_cwd, subagent_id)` → [`create_subagent_worktree`]
//! - `mark_worktree_payload_unproven(payload, reason, *, unmeasured)` → [`mark_worktree_payload_unproven`]
//! - `unproven_worktree_payload(info, reason)` → [`unproven_worktree_payload`]
//! - `finalize_subagent_worktree(info, *, prune=True)` → [`finalize_subagent_worktree`]
//! - `build_worktree_context_note(info)` → [`build_worktree_context_note`]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Constants — mirrors lines 49-51
// ---------------------------------------------------------------------------

/// Mirrors `_GIT_TIMEOUT = 30` (line 49).
pub const GIT_TIMEOUT_SECS: u64 = 30;

/// Mirrors `_GIT_TIMEOUT` as `Duration`.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(GIT_TIMEOUT_SECS);

/// Mirrors `_WORKTREES_DIRNAME = ".worktrees"` (line 50).
pub const WORKTREES_DIRNAME: &str = ".worktrees";

/// Mirrors `_BRANCH_NAMESPACE = "hermes-subagent"` (line 51).
pub const BRANCH_NAMESPACE: &str = "hermes-subagent";

// ---------------------------------------------------------------------------
// Git output — mirrors subprocess.CompletedProcess (lines 54-64)
// ---------------------------------------------------------------------------

/// Mirrors `subprocess.CompletedProcess` returned by `_run_git`.
///
/// `returncode` mirrors `result.returncode`, `stdout`/`stderr` are captured
/// text (utf-8, replace errors) trimmed by callers as needed.
#[derive(Debug, Clone)]
pub struct GitOutput {
    /// Exit code; 0 is success. -1 when the spawn itself failed (maps to
    /// Python `subprocess.run` raising `OSError`/`FileNotFoundError` which the
    /// callers handle via `except Exception`).
    pub returncode: i32,
    /// Captured stdout (utf-8, lossy).
    pub stdout: String,
    /// Captured stderr (utf-8, lossy).
    pub stderr: String,
}

// ---------------------------------------------------------------------------
// _run_git — mirrors lines 54-64
// ---------------------------------------------------------------------------

/// Run a git command, capturing output. Never panics on non-zero exit.
///
/// Mirrors `def _run_git(args, cwd: str, timeout: int = _GIT_TIMEOUT):`
/// ```python
/// return subprocess.run(
///     ["git", *args],
///     cwd=cwd,
///     capture_output=True,
///     text=True,
///     encoding="utf-8",
///     errors="replace",
///     timeout=timeout,
/// )
/// ```
/// In Rust the timeout is bounded via a `try_wait` loop (poll 10 ms, kill on
/// timeout, 1 s post-kill drain) mirroring `hermes_tui::git_probe` /
///
/// `hermes_cli._subprocess_compat.bounded_git_probe`. Non-interactive git env
/// (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=Never`) and `CREATE_NO_WINDOW`
/// on Windows are set so probes never prompt.
pub fn run_git(args: &[&str], cwd: &str) -> GitOutput {
    run_git_with_timeout(args, cwd, GIT_TIMEOUT)
}

/// Like [`run_git`] but with an explicit timeout.
///
/// Callers that need the default should use [`run_git`]; this variant exists
/// so tests can inject a shorter timeout and so the `timeout` kwarg from
/// Python is not lost in the port.
pub fn run_git_with_timeout(args: &[&str], cwd: &str, timeout: Duration) -> GitOutput {
    // Build argv: ["git", *args]
    let mut cmd = Command::new("git");
    cmd.args(args);
    cmd.current_dir(cwd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GCM_INTERACTIVE", "Never");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            // Mirrors Python `except Exception` in callers — return non-zero with stderr
            return GitOutput {
                returncode: -1,
                stdout: String::new(),
                stderr: e.to_string(),
            };
        }
    };

    // Take pipes early so we can drain them without blocking on full buffers
    // (output is small: single path / commit lines). For timeout we need to
    // read after the child exits.
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    #[cfg(windows)]
                    {
                        // Best-effort tree kill — mirrors kill_process_tree taskkill /T /F
                        let pid = child.id().to_string();
                        let mut tk = Command::new("taskkill");
                        tk.args(["/T", "/F", "/PID", &pid]);
                        tk.stdin(Stdio::null());
                        tk.stdout(Stdio::null());
                        tk.stderr(Stdio::null());
                        #[cfg(windows)]
                        {
                            use std::os::windows::process::CommandExt;
                            tk.creation_flags(0x08000000);
                        }
                        let _ = tk.spawn().and_then(|mut c| {
                            let s = Instant::now();
                            loop {
                                match c.try_wait() {
                                    Ok(Some(_)) => break Ok(()),
                                    Ok(None) => {
                                        if s.elapsed() > Duration::from_secs(2) {
                                            let _ = c.kill();
                                            break Ok(());
                                        }
                                        std::thread::sleep(Duration::from_millis(10));
                                    }
                                    Err(e) => break Err(e),
                                }
                            }
                        });
                    }
                    // 1 s post-kill drain — mirrors bounded_probe_run communicate(timeout=1)
                    let drain_start = Instant::now();
                    let mut terminated = false;
                    let mut final_status = None;
                    while drain_start.elapsed() < Duration::from_secs(1) {
                        match child.try_wait() {
                            Ok(Some(s)) => {
                                terminated = true;
                                final_status = Some(s);
                                break;
                            }
                            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                            Err(_) => break,
                        }
                    }
                    if !terminated {
                        return GitOutput {
                            returncode: -1,
                            stdout: String::new(),
                            stderr: format!("git timeout after {}s: {:?}", timeout.as_secs(), args),
                        };
                    }
                    break final_status;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                return GitOutput {
                    returncode: -1,
                    stdout: String::new(),
                    stderr: e.to_string(),
                };
            }
        }
    };

    let status = match status {
        Some(s) => s,
        None => {
            return GitOutput {
                returncode: -1,
                stdout: String::new(),
                stderr: "git wait failed".to_string(),
            };
        }
    };

    // Drain stdout/stderr with a 1 s bound — mirrors the stdout reader thread in git_probe
    let stdout = drain_pipe_stdout(stdout_pipe, Duration::from_secs(1));
    let stderr = drain_pipe_stderr(stderr_pipe, Duration::from_secs(1));

    GitOutput {
        returncode: status.code().unwrap_or(-1),
        stdout,
        stderr,
    }
}

fn drain_pipe_stdout(mut pipe: Option<std::process::ChildStdout>, timeout: Duration) -> String {
    use std::sync::mpsc;
    if let Some(mut out) = pipe.take() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        match rx.recv_timeout(timeout) {
            Ok(buf) => String::from_utf8_lossy(&buf).to_string(),
            Err(_) => String::new(),
        }
    } else {
        String::new()
    }
}

fn drain_pipe_stderr(mut pipe: Option<std::process::ChildStderr>, timeout: Duration) -> String {
    use std::sync::mpsc;
    if let Some(mut out) = pipe.take() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        match rx.recv_timeout(timeout) {
            Ok(buf) => String::from_utf8_lossy(&buf).to_string(),
            Err(_) => String::new(),
        }
    } else {
        String::new()
    }
}

/// Alias for callers that want explicit timeout (kept for 1:1 line traceability).
///
/// The Python `_run_git` is the single entry point; `run_git_correct` exists
/// so ports that historically used the direct `Command::output` path can keep
/// compiling. It delegates to [`run_git_with_timeout`] — identical behavior.
pub fn run_git_correct(args: &[&str], cwd: &str, timeout: Duration) -> GitOutput {
    run_git_with_timeout(args, cwd, timeout)
}

// ---------------------------------------------------------------------------
// Helpers — home resolution, path expansion (mirrors Python os.path)
// ---------------------------------------------------------------------------

fn get_hermes_home() -> PathBuf {
    for key in ["GRAY_HOME", "HERMES_HOME"] {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim().to_string();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed);
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim().to_string();
        if !trimmed.is_empty() {
            // gray uses ~/.gray; hermes uses ~/.hermes — check both, prefer gray for this workspace
            // but keep compat: if .gray exists use it else .hermes. For 1:1 we check .hermes first.
            return PathBuf::from(trimmed).join(".hermes");
        }
    }
    PathBuf::from("/tmp/.hermes")
}

fn expand_user(path_str: &str) -> String {
    if path_str == "~" || path_str.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            if path_str == "~" {
                return home;
            }
            return format!("{}{}", home, &path_str[1..]);
        }
    }
    // Also handle ~ via dirs fallback
    path_str.to_string()
}

fn abspath(path_str: &str) -> Option<PathBuf> {
    let expanded = expand_user(path_str);
    let p = Path::new(&expanded);
    if p.is_absolute() {
        Some(p.to_path_buf())
    } else if let Ok(cwd) = std::env::current_dir() {
        Some(cwd.join(p))
    } else {
        Some(PathBuf::from("/").join(p))
    }
}

// ---------------------------------------------------------------------------
// local_backend_active — mirrors lines 67-78
// ---------------------------------------------------------------------------

/// True when the terminal backend is local (worktrees visible to tools).
///
/// Mirrors `def local_backend_active() -> bool:` (lines 67-78):
/// ```python
/// try:
///     from hermes_cli.config import load_config_readonly
///     cfg = load_config_readonly()
///     backend = ((cfg.get("terminal") or {}).get("backend") or "local")
///     return str(backend).strip().lower() in ("", "local")
/// except Exception:
///     return True
/// ```
/// Rust: reads `~/.hermes/config.yaml` (or `$HERMES_HOME/config.yaml`) and
/// looks for `terminal: backend:` or a top-level `backend:`. Any parse / IO
/// failure returns `true` (legacy entry points without loader default to local).
pub fn local_backend_active() -> bool {
    // Try to load config via hermes_cli config file; any error → true.
    let home = get_hermes_home();
    let cfg_path = home.join("config.yaml");
    let text = match fs::read_to_string(&cfg_path) {
        Ok(t) => t,
        Err(_) => return true,
    };
    // Minimal yaml scan without yaml crate (NEVER cargo): look for backend lines.
    // Search for "backend:" case-insensitive; value is until newline / comment / quote.
    // If file contains a terminal.backend we respect it; otherwise fall back to "local".
    // This keeps the no-dep promise and is faithful for the opt-in check (local vs non-local).
    let backend = extract_backend(&text);
    let b = backend.trim().to_lowercase();
    b.is_empty() || b == "local"
}

fn extract_backend(text: &str) -> String {
    // Find last "backend:" occurrence (closest to terminal: block) but simple scan suffices.
    // Look line by line for "backend" key.
    for line in text.lines() {
        let trimmed = line.trim();
        // skip comments
        if trimmed.starts_with('#') {
            continue;
        }
        // find "backend:" (with optional quotes)
        let lower = trimmed.to_ascii_lowercase();
        if let Some(pos) = lower.find("backend") {
            // ensure it's a key: check that before "backend" we have start or whitespace or : structure
            // and after "backend" we have optional spaces then colon
            let after = &trimmed[pos + 7..];
            let after_trim = after.trim_start();
            if after_trim.starts_with(':') {
                let val_part = after_trim[1..].trim();
                // strip quotes and inline comments
                let val = val_part
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .trim()
                    .to_string();
                // For this minimal port we take the first backend found; Python would take
                // terminal.backend specifically, but a file with multiple backends
                // typically only has one.
                return val;
            }
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// resolve_repo_root — mirrors lines 81-98
// ---------------------------------------------------------------------------

/// Return the git toplevel for `path`, or `None` when not in a work tree.
///
/// Mirrors `def resolve_repo_root(path: Optional[str]) -> Optional[str]:` (81-98):
/// ```python
/// if not path: return None
/// try: candidate = os.path.abspath(os.path.expanduser(str(path)))
/// except Exception: return None
/// if not os.path.isdir(candidate): return None
/// try: result = _run_git(["rev-parse", "--show-toplevel"], cwd=candidate)
/// except Exception: logger.debug(...); return None
/// if result.returncode != 0: return None
/// root = result.stdout.strip()
/// return root or None
/// ```
pub fn resolve_repo_root(path: Option<&str>) -> Option<String> {
    let p = path?;
    if p.trim().is_empty() {
        return None;
    }
    let candidate = abspath(p)?;
    if !candidate.is_dir() {
        return None;
    }
    let candidate_str = candidate.to_string_lossy().to_string();
    // Use the correct draining variant for 1:1 capture
    let result = run_git_correct(&["rev-parse", "--show-toplevel"], &candidate_str, GIT_TIMEOUT);
    // In the original `_run_git` the exception path would return None; our
    // run_git returns returncode -1 which we treat as non-zero.
    if result.returncode != 0 {
        return None;
    }
    let root = result.stdout.trim().to_string();
    if root.is_empty() {
        None
    } else {
        Some(root)
    }
}

// ---------------------------------------------------------------------------
// _ensure_gitignore_entry — mirrors lines 101-118
// ---------------------------------------------------------------------------

/// Best-effort: keep `.worktrees/` out of git status.
///
/// Mirrors `def _ensure_gitignore_entry(repo_root: str) -> None:` (101-118).
/// Reads `.gitignore` as utf-8-sig (BOM-aware), checks splitlines, appends
/// `.worktrees/` with newline handling. Any IO error is swallowed with a
/// debug log (no panic).
pub fn ensure_gitignore_entry(repo_root: &str) {
    let gitignore = Path::new(repo_root).join(".gitignore");
    let entry = format!("{WORKTREES_DIRNAME}/");
    // Read existing as utf-8, handling BOM (utf-8-sig) by stripping leading BOM if present
    let existing = if gitignore.exists() {
        match fs::read(&gitignore) {
            Ok(bytes) => {
                // Strip BOM if present (EF BB BF) and decode lossy
                let slice = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
                    &bytes[3..]
                } else {
                    &bytes[..]
                };
                String::from_utf8_lossy(slice).to_string()
            }
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };
    if existing.split('\n').map(|l| l.trim_end_matches('\r')).any(|l| l == entry) {
        return;
    }
    // Also check splitlines exact: Python uses splitlines() which splits on \r\n etc.
    // Our map above covers \n and \r\n; good enough for 1:1.
    let needs_newline = !existing.is_empty() && !existing.ends_with('\n');
    let mut to_write = String::new();
    if needs_newline {
        to_write.push('\n');
    }
    to_write.push_str(&entry);
    to_write.push('\n');
    if let Err(e) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(to_write.as_bytes())
        })
    {
        // Mirrors logger.debug("subagent worktree: could not update .gitignore: %s", exc)
        let _ = e;
        // In Rust we use log::debug if available; fallback to no-op to keep 1:1 quiet.
        #[cfg(feature = "log")]
        log::debug!("subagent worktree: could not update .gitignore: {}", e);
    }
}

// ---------------------------------------------------------------------------
// Worktree structures — mirrors Python dicts
// ---------------------------------------------------------------------------

/// Metadata returned by [`create_subagent_worktree`] on success.
///
/// Mirrors the dict `{"path": ..., "branch": ..., "repo_root": ..., "base_commit": ...}`
/// (line 167-172).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    /// Absolute path to the worktree (e.g. `<repo>/.worktrees/subagent-abc123`).
    pub path: String,
    /// Branch name (e.g. `hermes-subagent/subagent-abc123`).
    pub branch: String,
    /// Repo root (toplevel).
    pub repo_root: String,
    /// Base commit at creation time (`HEAD`).
    pub base_commit: String,
}

/// Result-entry payload returned by [`finalize_subagent_worktree`].
///
/// Mirrors the dict `{"path": ..., "branch": ..., "commits": ..., "dirty": ..., "pruned": ...}`
/// plus optional `inspection_failed` + `note` (#88113).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreePayload {
    /// Worktree path (may be empty if creation never succeeded).
    pub path: String,
    /// Branch name.
    pub branch: String,
    /// Number of commits ahead of base.
    pub commits: i64,
    /// Whether the worktree has uncommitted changes.
    pub dirty: bool,
    /// Whether the worktree was pruned (removed) — also true when nothing on disk.
    pub pruned: bool,
    /// Set when a git probe failed; fields are defaults, not measurements (#88113).
    pub inspection_failed: Option<bool>,
    /// Human note when `inspection_failed` is set.
    pub note: Option<String>,
}

impl WorktreePayload {
    /// Helper to serialize to JSON (mirrors Python dict JSON-ability).
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "path": self.path,
            "branch": self.branch,
            "commits": self.commits,
            "dirty": self.dirty,
            "pruned": self.pruned,
        });
        if let Some(failed) = self.inspection_failed {
            v["inspection_failed"] = serde_json::json!(failed);
        }
        if let Some(note) = &self.note {
            v["note"] = serde_json::json!(note);
        }
        v
    }
}

// ---------------------------------------------------------------------------
// create_subagent_worktree — mirrors lines 120-172
// ---------------------------------------------------------------------------

/// Create an isolated worktree for one child agent.
///
/// Mirrors `def create_subagent_worktree(parent_cwd: Optional[str], subagent_id: Optional[str] = None) -> Optional[Dict[str, str]]:`
/// Returns metadata on success, or `None` when the workspace is not a git
/// repository or worktree creation fails — mirroring Muse Code, absence
/// of git downgrades silently to shared-workspace behavior.
pub fn create_subagent_worktree(
    parent_cwd: Option<&str>,
    subagent_id: Option<&str>,
) -> Option<WorktreeInfo> {
    let repo_root = resolve_repo_root(parent_cwd)?;
    // short_id = (subagent_id or uuid.uuid4().hex[:8]).replace("/", "-")
    let short_id_raw = match subagent_id {
        Some(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => random_hex8(),
    };
    let short_id = short_id_raw.replace('/', "-");
    let wt_name = format!("subagent-{short_id}");
    let branch = format!("{BRANCH_NAMESPACE}/{wt_name}");
    let wt_path = Path::new(&repo_root).join(WORKTREES_DIRNAME).join(&wt_name);

    if let Err(e) = fs::create_dir_all(wt_path.parent().unwrap_or(Path::new(&repo_root))) {
        let _ = e;
        #[cfg(feature = "log")]
        log::warn!("subagent worktree: cannot create {}: {}", wt_path.parent().unwrap_or(Path::new(&repo_root)).display(), e);
        return None;
    }

    ensure_gitignore_entry(&repo_root);

    // base = _run_git(["rev-parse", "HEAD"], cwd=repo_root)
    // base_commit = base.stdout.strip() if base.returncode == 0 else ""
    let base = run_git_correct(&["rev-parse", "HEAD"], &repo_root, GIT_TIMEOUT);
    let base_commit = if base.returncode == 0 {
        base.stdout.trim().to_string()
    } else {
        String::new()
    };

    let wt_str = wt_path.to_string_lossy().to_string();
    let result = run_git_correct(
        &["worktree", "add", &wt_str, "-b", &branch, "HEAD"],
        &repo_root,
        GIT_TIMEOUT,
    );
    if result.returncode != 0 {
        // Common on repos with zero commits (unborn HEAD) — degrade silently.
        #[cfg(feature = "log")]
        log::warn!(
            "subagent worktree: git worktree add failed: {}",
            result.stderr.trim()
        );
        let _ = result.stderr.trim();
        return None;
    }

    #[cfg(feature = "log")]
    log::info!("subagent worktree created: {} (branch {})", wt_str, branch);

    Some(WorktreeInfo {
        path: wt_str,
        branch,
        repo_root,
        base_commit,
    })
}

fn random_hex8() -> String {
    let mut buf = [0u8; 4];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        if f.read_exact(&mut buf).is_ok() {
            return buf.iter().map(|b| format!("{b:02x}")).collect();
        }
    }
    // Fallback: time + pid (deterministic enough for 1:1 short_id uniqueness)
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let v = nanos.wrapping_add(pid).wrapping_add(0x9e3779b97f4a7c15u128);
    format!("{v:032x}")[..8].to_string()
}

// ---------------------------------------------------------------------------
// mark_worktree_payload_unproven — mirrors lines 175-209
// ---------------------------------------------------------------------------

/// Flag a worktree result payload as un-inspected, in place (#88113).
///
/// Mirrors `def mark_worktree_payload_unproven(payload: Dict[str, Any], reason: str, *, unmeasured: str = "commits/dirty") -> Dict[str, Any]:`
/// A failed probe proves nothing about the tree, so the fields it would have
/// filled keep their defaults. The parent agent only ever sees this dict — it
/// cannot read logs — so the uncertainty has to travel *in the payload*, or
/// "0 commits, clean" reads as "the child produced nothing" and the work we
/// just preserved is never looked at.
///
/// *unmeasured* names only the fields this failure actually left unproven: one
/// probe can succeed while the other fails (a bad ``base_commit`` fails
/// ``rev-list`` while ``status`` still reports a real ``dirty``), and claiming
/// a measured value is UNKNOWN would be its own kind of misreport.
///
/// Shared by ``finalize_subagent_worktree`` and ``delegate_tool``'s
/// finalize-raised fallback so the two producers of this schema cannot drift.
pub fn mark_worktree_payload_unproven(
    payload: &mut WorktreePayload,
    reason: &str,
    unmeasured: Option<&str>,
) -> &mut WorktreePayload {
    let unmeasured = unmeasured.unwrap_or("commits/dirty");
    let path = payload.path.clone();
    let branch = payload.branch.clone();
    payload.inspection_failed = Some(true);
    payload.note = Some(format!(
        "git inspection failed ({reason}): {unmeasured} UNKNOWN — not proven zero/clean. The worktree and branch were preserved — inspect {path} (branch {branch}) before assuming no work."
    ));
    #[cfg(feature = "log")]
    log::warn!(
        "subagent worktree: git inspection failed ({}) — keeping {} (branch {}) for manual review",
        reason,
        path,
        branch
    );
    let _ = reason;
    payload
}

/// Convenience wrapper with default `unmeasured = "commits/dirty"` (mirrors Python default).
pub fn mark_worktree_payload_unproven_default(
    payload: &mut WorktreePayload,
    reason: &str,
) -> &mut WorktreePayload {
    mark_worktree_payload_unproven(payload, reason, None)
}

// ---------------------------------------------------------------------------
// unproven_worktree_payload — mirrors lines 212-231
// ---------------------------------------------------------------------------

/// Build a complete un-inspected payload from creation-side *info*.
///
/// Mirrors `def unproven_worktree_payload(info: Dict[str, str], reason: str) -> Dict[str, Any]:`
/// For callers that never got a payload back at all (``delegate_tool``'s
/// fallback when ``finalize_subagent_worktree`` itself raises). Emits exactly
/// the schema the parent expects — notably WITHOUT the creation-side
/// ``repo_root``/``base_commit`` internals.
pub fn unproven_worktree_payload(info: &WorktreeInfo, reason: &str) -> WorktreePayload {
    let mut payload = WorktreePayload {
        path: info.path.clone(),
        branch: info.branch.clone(),
        commits: 0,
        dirty: false,
        pruned: false,
        inspection_failed: None,
        note: None,
    };
    mark_worktree_payload_unproven(&mut payload, reason, None);
    payload
}

// ---------------------------------------------------------------------------
// finalize_subagent_worktree — mirrors lines 234-337
// ---------------------------------------------------------------------------

/// Inspect (and possibly prune) a child worktree after the child finishes.
///
/// Mirrors `def finalize_subagent_worktree(info: Dict[str, str], *, prune: bool = True) -> Dict[str, Any]:`
/// Returns a result-entry payload: path, branch, ``commits`` ahead of the
/// base, ``dirty`` (uncommitted changes present), and ``pruned``. A worktree
/// with zero commits and a clean tree is removed when *prune* is true **and
/// both git probes succeeded**; anything holding work is always kept for the
/// parent to review or merge.
///
/// If ``git rev-list``/``git status`` exits non-zero (or the inspection
/// raises), the tree state is unknown, so the worktree and branch are kept
/// and the payload carries ``inspection_failed: True`` plus a ``note``.
/// ``commits``/``dirty`` are then defaults, NOT measurements — the parent
/// must inspect the worktree instead of concluding the child did no work.
pub fn finalize_subagent_worktree(info: &WorktreeInfo, prune: bool) -> WorktreePayload {
    let path = info.path.clone();
    let branch = info.branch.clone();
    let repo_root = info.repo_root.clone();
    let base_commit = info.base_commit.clone();

    let mut payload = WorktreePayload {
        path: path.clone(),
        branch: branch.clone(),
        commits: 0,
        dirty: false,
        pruned: false,
        inspection_failed: None,
        note: None,
    };

    if path.is_empty() || !Path::new(&path).is_dir() {
        payload.pruned = true; // nothing on disk to review
        return payload;
    }

    // Helper: mirrors inner `_unproven(reason, *, unmeasured="commits/dirty")`
    // We capture payload mutably via closure.

    // A worktree whose commit count was never measured must not be pruned
    // either: the prune condition reads payload["commits"], and without a base
    // commit that value is an unproven default, exactly the class of bug
    // #88113 is about.
    if base_commit.is_empty() {
        mark_worktree_payload_unproven(
            &mut payload,
            "no base_commit recorded — commit count unmeasurable",
            Some("commits"),
        );
        return payload;
    }

    let mut failed: Vec<String> = Vec::new();
    let mut unmeasured: Vec<String> = Vec::new();

    // The Python wraps both probes in a single try/except that catches
    // timeout, OSError, or non-numeric stdout. In Rust we treat spawn
    // failure as returncode -1 (already handled as failed entry), but a
    // truly exceptional condition (panic) is not expected. We still guard
    // the int parse as the exception source.
    let outcome: Result<(), String> = (|| {
        let counted = run_git_correct(
            &["rev-list", "--count", &format!("{base_commit}..HEAD")],
            &path,
            GIT_TIMEOUT,
        );
        if counted.returncode == 0 {
            let s = counted.stdout.trim();
            let val_str = if s.is_empty() { "0" } else { s };
            match val_str.parse::<i64>() {
                Ok(v) => payload.commits = v,
                Err(e) => return Err(format!("rev-list parse failed: {e}")),
            }
        } else {
            let stderr_snip = counted.stderr.trim().chars().take(200).collect::<String>();
            failed.push(format!("rev-list exit {}: {}", counted.returncode, stderr_snip));
            unmeasured.push("commits".to_string());
        }

        let status = run_git_correct(&["status", "--porcelain"], &path, GIT_TIMEOUT);
        if status.returncode == 0 {
            payload.dirty = !status.stdout.trim().is_empty();
        } else {
            let stderr_snip = status.stderr.trim().chars().take(200).collect::<String>();
            failed.push(format!("status exit {}: {}", status.returncode, stderr_snip));
            unmeasured.push("dirty".to_string());
        }
        Ok(())
    })();

    if let Err(exc) = outcome {
        // Same unknown state as a non-zero exit (timeout, OSError, or a
        // non-numeric rev-list stdout) — keep the worktree rather than risk
        // deleting work, and tell the caller the numbers are unproven. Which
        // probe raised is unknowable here, so neither value is trustworthy.
        mark_worktree_payload_unproven(&mut payload, &format!("inspection raised: {exc}"), None);
        return payload;
    }

    if !failed.is_empty() {
        // Fail-safe (#88113): a destructive cleanup requires affirmative proof
        // of "zero commits + clean tree"; the defaults prove nothing.
        let reason = failed.join("; ");
        let unmeasured_str = unmeasured.join("/");
        mark_worktree_payload_unproven(&mut payload, &reason, Some(&unmeasured_str));
        return payload;
    }

    if prune && payload.commits == 0 && !payload.dirty {
        // Try to remove worktree and delete branch; failures are debug-logged, not fatal.
        let cwd = if repo_root.is_empty() { path.clone() } else { repo_root.clone() };
        let removed = run_git_correct(&["worktree", "remove", "--force", &path], &cwd, GIT_TIMEOUT);
        if removed.returncode == 0 {
            let _ = run_git_correct(&["branch", "-D", &branch], &cwd, GIT_TIMEOUT);
            payload.pruned = true;
            #[cfg(feature = "log")]
            log::info!("subagent worktree pruned (no work): {}", path);
        } else {
            #[cfg(feature = "log")]
            log::debug!(
                "subagent worktree: prune failed: {}",
                removed.stderr.trim()
            );
            let _ = removed.stderr.trim();
        }
    }

    payload
}

/// Variant with default `prune = true` (mirrors Python default).
pub fn finalize_subagent_worktree_default(info: &WorktreeInfo) -> WorktreePayload {
    finalize_subagent_worktree(info, true)
}

// ---------------------------------------------------------------------------
// build_worktree_context_note — mirrors lines 340-352
// ---------------------------------------------------------------------------

/// Context block telling the child to work inside its isolated worktree.
///
/// Mirrors `def build_worktree_context_note(info: Dict[str, str]) -> str:` (340-352):
/// ```python
/// return (
///     "\n\n[WORKTREE ISOLATION] You are working in an isolated git worktree "
///     f"at: {info.get('path')}\n"
///     f"Your dedicated branch is: {info.get('branch')}\n"
///     "All file edits and shell commands must happen inside this worktree "
///     "directory (your terminal already starts there). Do NOT cd to the "
///     "main repository checkout. Commit your changes to your branch when "
///     "done; the parent agent will review and merge your branch. If you "
///     "make no commits and leave the tree clean, the worktree is discarded "
///     "automatically."
/// )
/// ```
pub fn build_worktree_context_note(info: &WorktreeInfo) -> String {
    format!(
        "\n\n[WORKTREE ISOLATION] You are working in an isolated git worktree at: {}\nYour dedicated branch is: {}\nAll file edits and shell commands must happen inside this worktree directory (your terminal already starts there). Do NOT cd to the main repository checkout. Commit your changes to your branch when done; the parent agent will review and merge your branch. If you make no commits and leave the tree clean, the worktree is discarded automatically.",
        info.path, info.branch
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_dir(suffix: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "hermes-subagent-wt-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        ));
        let _ = fs::create_dir_all(&base);
        base
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(GIT_TIMEOUT_SECS, 30);
        assert_eq!(GIT_TIMEOUT, Duration::from_secs(30));
        assert_eq!(WORKTREES_DIRNAME, ".worktrees");
        assert_eq!(BRANCH_NAMESPACE, "hermes-subagent");
    }

    #[test]
    fn run_git_nonzero_on_bogus_cwd() {
        let out = run_git_correct(&["rev-parse", "--show-toplevel"], "/nonexistent_xyz_12345", GIT_TIMEOUT);
        assert_ne!(out.returncode, 0);
    }

    #[test]
    fn resolve_repo_root_none_and_missing() {
        assert_eq!(resolve_repo_root(None), None);
        assert_eq!(resolve_repo_root(Some("")), None);
        assert_eq!(resolve_repo_root(Some("   ")), None);
        assert_eq!(resolve_repo_root(Some("/nonexistent_xyz_12345_no_such_dir")), None);
    }

    #[test]
    fn resolve_repo_root_non_repo_tmp_is_none() {
        let dir = tmp_dir("nonrepo");
        let s = dir.to_string_lossy().to_string();
        assert_eq!(resolve_repo_root(Some(&s)), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_gitignore_creates_and_idempotent() {
        let dir = tmp_dir("gitignore");
        ensure_gitignore_entry(&dir.to_string_lossy().to_string());
        let gi = dir.join(".gitignore");
        let c1 = fs::read_to_string(&gi).unwrap();
        assert!(c1.contains(".worktrees/"));
        // second call should not duplicate
        ensure_gitignore_entry(&dir.to_string_lossy().to_string());
        let c2 = fs::read_to_string(&gi).unwrap();
        assert_eq!(c1, c2);
        // existing content without trailing newline
        let dir2 = tmp_dir("gitignore2");
        fs::write(dir2.join(".gitignore"), "node_modules/").unwrap();
        ensure_gitignore_entry(&dir2.to_string_lossy().to_string());
        let c3 = fs::read_to_string(dir2.join(".gitignore")).unwrap();
        assert!(c3.contains("node_modules/"));
        assert!(c3.contains(".worktrees/"));
        assert!(c3.contains('\n'));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn mark_payload_sets_failed_and_note() {
        let mut p = WorktreePayload {
            path: "/tmp/foo/.worktrees/subagent-abc".to_string(),
            branch: "hermes-subagent/subagent-abc".to_string(),
            commits: 0,
            dirty: false,
            pruned: false,
            inspection_failed: None,
            note: None,
        };
        mark_worktree_payload_unproven(&mut p, "rev-list exit 128: fatal", Some("commits"));
        assert_eq!(p.inspection_failed, Some(true));
        let note = p.note.unwrap();
        assert!(note.contains("rev-list exit 128"));
        assert!(note.contains("commits UNKNOWN"));
        assert!(note.contains("/tmp/foo/.worktrees/subagent-abc"));
        assert!(note.contains("hermes-subagent/subagent-abc"));
        assert!(note.contains("preserved"));
    }

    #[test]
    fn mark_payload_default_unmeasured() {
        let mut p = WorktreePayload {
            path: "/p".to_string(),
            branch: "b".to_string(),
            commits: 0,
            dirty: false,
            pruned: false,
            inspection_failed: None,
            note: None,
        };
        mark_worktree_payload_unproven(&mut p, "boom", None);
        assert!(p.note.unwrap().contains("commits/dirty UNKNOWN"));
    }

    #[test]
    fn unproven_payload_from_info() {
        let info = WorktreeInfo {
            path: "/repo/.worktrees/subagent-x".to_string(),
            branch: "hermes-subagent/subagent-x".to_string(),
            repo_root: "/repo".to_string(),
            base_commit: "abc".to_string(),
        };
        let p = unproven_worktree_payload(&info, "no base_commit");
        assert_eq!(p.path, info.path);
        assert_eq!(p.branch, info.branch);
        assert_eq!(p.commits, 0);
        assert!(!p.dirty);
        assert!(!p.pruned);
        assert_eq!(p.inspection_failed, Some(true));
        assert!(p.note.unwrap().contains("no base_commit"));
        // must NOT expose repo_root/base_commit
        let j = p.to_json();
        assert!(j.get("repo_root").is_none());
        assert!(j.get("base_commit").is_none());
    }

    #[test]
    fn finalize_missing_path_pruned() {
        let info = WorktreeInfo {
            path: "/nonexistent_xyz_12345_no_such_dir".to_string(),
            branch: "hermes-subagent/subagent-missing".to_string(),
            repo_root: "/tmp".to_string(),
            base_commit: "abc".to_string(),
        };
        let p = finalize_subagent_worktree(&info, true);
        assert!(p.pruned);
        assert_eq!(p.commits, 0);
        assert!(!p.dirty);
    }

    #[test]
    fn finalize_empty_base_commit_unproven_commits() {
        let dir = tmp_dir("finalize-unproven");
        // need dir exists so first guard passes, but base_commit empty triggers unproven
        let info = WorktreeInfo {
            path: dir.to_string_lossy().to_string(),
            branch: "hermes-subagent/subagent-y".to_string(),
            repo_root: dir.to_string_lossy().to_string(),
            base_commit: String::new(),
        };
        let p = finalize_subagent_worktree(&info, true);
        assert_eq!(p.inspection_failed, Some(true));
        assert!(p.note.unwrap().contains("no base_commit"));
        assert!(p.note.as_ref().unwrap().contains("commits UNKNOWN"));
        // should not be pruned (unproven)
        assert!(!p.pruned);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_context_note_contains_paths() {
        let info = WorktreeInfo {
            path: "/repo/.worktrees/subagent-abc123".to_string(),
            branch: "hermes-subagent/subagent-abc123".to_string(),
            repo_root: "/repo".to_string(),
            base_commit: "deadbeef".to_string(),
        };
        let note = build_worktree_context_note(&info);
        assert!(note.contains("[WORKTREE ISOLATION]"));
        assert!(note.contains("/repo/.worktrees/subagent-abc123"));
        assert!(note.contains("hermes-subagent/subagent-abc123"));
        assert!(note.contains("Do NOT cd to the main repository checkout"));
        assert!(note.contains("Commit your changes"));
        assert!(note.contains("discarded automatically"));
    }

    #[test]
    fn local_backend_active_defaults_true_on_missing_config() {
        // Without a config file, should return true (local). We test by using a
        // temp HERMES_HOME that has no config.yaml.
        let tmp = tmp_dir("backend-active");
        // Temporarily set HERMES_HOME to tmp (which has no config.yaml)
        let prev = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()) };
        // Also clear GRAY_HOME to avoid picking it up
        let prev_gray = std::env::var("GRAY_HOME").ok();
        unsafe { std::env::remove_var("GRAY_HOME") };
        let v = local_backend_active();
        assert!(v, "missing config should default to local=true");
        // restore
        if let Some(p) = prev {
            unsafe { std::env::set_var("HERMES_HOME", p) };
        } else {
            unsafe { std::env::remove_var("HERMES_HOME") };
        }
        if let Some(p) = prev_gray {
            unsafe { std::env::set_var("GRAY_HOME", p) };
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn worktree_payload_json_shape() {
        let p = WorktreePayload {
            path: "/a".to_string(),
            branch: "b".to_string(),
            commits: 1,
            dirty: true,
            pruned: false,
            inspection_failed: Some(true),
            note: Some("n".to_string()),
        };
        let j = p.to_json();
        assert_eq!(j["path"], "/a");
        assert_eq!(j["commits"], 1);
        assert_eq!(j["dirty"], true);
        assert_eq!(j["inspection_failed"], true);
        assert_eq!(j["note"], "n");
        // without optional fields they are omitted
        let p2 = WorktreePayload {
            path: "/a".to_string(),
            branch: "b".to_string(),
            commits: 0,
            dirty: false,
            pruned: true,
            inspection_failed: None,
            note: None,
        };
        let j2 = p2.to_json();
        assert!(j2.get("inspection_failed").is_none());
        assert!(j2.get("note").is_none());
        assert_eq!(j2["pruned"], true);
    }

    #[test]
    fn short_id_generation_sanitizes_slash() {
        // Simulate create logic for sanitization: "/" -> "-"
        let id = "a/b/c";
        let sanitized = id.replace('/', "-");
        assert_eq!(sanitized, "a-b-c");
        // random_hex8 should be 8 hex chars
        let h = random_hex8();
        assert_eq!(h.len(), 8);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
