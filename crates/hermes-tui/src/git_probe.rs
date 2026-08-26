//! Git working-tree probing for the gateway: run git, resolve repo roots, fold
//! linked worktrees under their common root.
//!
//! 1:1 port of `tui_gateway/git_probe.py` (202 lines).
//!
//! Probing runs where the gateway runs, so it resolves repos for both local and
//! remote backends (unlike the desktop's electron probe, which only sees the local
//! fs). Resolved roots are cached with a thread-safe, single-flight cache: the
//! gateway's long handlers run on worker threads, so concurrent identical probes
//! (e.g. two overlapping project-tree builds) share one `git` invocation instead
//! of racing an unguarded dict.
//!
//! Positive results are cached for the process lifetime; negative results (a cwd
//! that isn't a git repo, or a deleted/nonexistent dir) are cached only for a
//! short TTL (`NEG_TTL`). Caching negatives matters a lot for the desktop
//! Projects tree: ``project_tree.build_tree`` resolves a cwd once *per session*
//! (not per distinct cwd), so a power user with hundreds of sessions in
//! non-git/deleted dirs would otherwise re-spawn ``git`` hundreds of times on
//! *every* sidebar open — the cause of the multi-second "Projects" load.
//!
//! ```python
//! # Python — tui_gateway/git_probe.py
//! _GIT_TIMEOUT = 1.5
//! _WARM_WORKERS = 8
//! _NEG_TTL = 30.0
//! def run_git(cwd: str, *args: str) -> str: ...
//! def branch(cwd: str) -> str: ...
//! class _RootCache:
//!     def invalidate(self) -> None: ...
//!     def resolve(self, key: str, probe) -> str: ...
//! _cache = _RootCache()
//! def invalidate() -> None: ...
//! def repo_root(cwd: str) -> str: ...
//! def common_repo_root(cwd: str) -> str: ...
//! def resolve(cwd: str) -> dict | None: ...
//! def warm_roots(cwds: Iterable[str], max_workers: int = _WARM_WORKERS) -> None: ...
//! ```
//!
//! # Rust mapping
//!
//! * `_GIT_TIMEOUT = 1.5` → [`GIT_TIMEOUT_SECS`] / [`GIT_TIMEOUT`] (`Duration::from_millis(1500)`).
//! * `_WARM_WORKERS = 8` → [`WARM_WORKERS`].
//! * `_NEG_TTL = 30.0` → [`NEG_TTL_SECS`] / [`NEG_TTL`] (`Duration::from_secs(30)`).
//! * `bounded_git_probe(["git","-C",cwd,*args], timeout=_GIT_TIMEOUT)` →
//!   [`run_git`] builds `Command::new("git").arg("-C").arg(cwd).args(args)` with
//!   `GIT_TERMINAL_PROMPT=0` / `GCM_INTERACTIVE=Never`, `stdin(Null)` +
//!   `stdout/piped` + `stderr/piped`, `creation_flags(CREATE_NO_WINDOW)` on Windows,
//!   and a bounded `try_wait` loop (poll 10 ms, kill on timeout, 1 s post-kill
//!   drain, abandon if pipes still held) mirroring `hermes_cli._subprocess_compat
//!   .bounded_git_probe` / `bounded_probe_run` / `kill_process_tree`.
//! * `os.path.isdir(cwd)` guard → `Path::new(cwd).is_dir()`.
//! * `branch(cwd)` → [`branch`]: `run_git(cwd, ["branch","--show-current"])` or
//!   `run_git(cwd, ["rev-parse","--short","HEAD"])`.
//! * `_RootCache` (`_lock`, `_roots`, `_neg`, `_inflight`) → [`RootCache`] with
//!   `Mutex<Inner>` (`roots: HashMap<String,String>`, `neg: HashMap<String,Instant>`,
//!   `inflight: HashMap<String, Arc<(Mutex<bool>,Condvar)>>`). `threading.Event`
//!   single-flight → `Arc<(Mutex<bool>,Condvar)>` + `wait_timeout(GIT_TIMEOUT+0.5)`;
//!   `time.monotonic() + _NEG_TTL` → `Instant::now() + NEG_TTL`; positive roots
//!   live forever, negatives expire after `NEG_TTL`.
//! * `_cache = _RootCache()` → global [`RootCache`] behind `OnceLock` (`global_cache()`).
//! * `invalidate()` → [`invalidate`] + [`RootCache::invalidate`].
//! * `repo_root(cwd)` → [`repo_root`] (`global_cache().resolve(cwd, || run_git(...))`).
//! * `common_repo_root(cwd)` → [`common_repo_root`] (checks `repo_root(cwd)` first
//!   to spare non-repo cwds a second spawn, then resolves `common:{cwd}` with a
//!   probe that runs `rev-parse --path-format=absolute --git-common-dir`,
//!   `realpath` via `fs::canonicalize`, basename `.git` check, `dirname` with
//!   `replace(os.sep,"/")` → replace `\` with `/`).
//! * `resolve(cwd)` → [`resolve`] returns `Option<ResolveResult>` (`repo_root` +
//!   `worktree_root`) instead of `dict|None`; `worktree_root == repo_root` is
//!   the main checkout (same contract).
//! * `warm_roots(cwds, max_workers)` → [`warm_roots`] / [`warm_roots_with_workers`]
//!   dedup+trim+sort, single-entry fast path, bounded `ThreadPoolExecutor` →
//!   `std::thread` workers on `Arc<Mutex<VecDeque<String>>>` capped at
//!   `min(max_workers, pending.len())`.
//! * `threading.Lock` → `Mutex`; `threading.Event` → `Condvar`; `time.monotonic()`
//!   → `Instant::now()`; `os.path.realpath` → `fs::canonicalize` with fallback;
//!   `os.path.basename/dirname` → `Path::file_name/parent`; `os.sep` replace →
//!   `replace('\\',"/")`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Constants — mirrors git_probe.py:35-42
// ---------------------------------------------------------------------------

/// Mirrors `_GIT_TIMEOUT = 1.5`.
pub const GIT_TIMEOUT_SECS: f64 = 1.5;

/// Mirrors `_GIT_TIMEOUT` as `Duration`.
pub const GIT_TIMEOUT: Duration = Duration::from_millis(1500);

/// Mirrors `_WARM_WORKERS = 8`.
pub const WARM_WORKERS: usize = 8;

/// Mirrors `_NEG_TTL = 30.0`.
pub const NEG_TTL_SECS: f64 = 30.0;

/// Mirrors `_NEG_TTL` as `Duration`.
pub const NEG_TTL: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Low-level git spawn — mirrors bounded_git_probe / run_git
// ---------------------------------------------------------------------------

fn bounded_git_probe_argv(argv: &[String]) -> String {
    if argv.is_empty() {
        return String::new();
    }
    // Build command from argv (argv[0] is "git")
    let mut cmd = Command::new(&argv[0]);
    if argv.len() > 1 {
        cmd.args(&argv[1..]);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Non-interactive git env — mirrors hermes_cli._subprocess_compat.noninteractive_git_env
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GCM_INTERACTIVE", "Never");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW — mirrors windows_hide_flags()
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return String::new(),
    };

    // Take stdout/stderr so we can read them after the child exits without
    // blocking on pipe fullness (output is tiny: a single path/branch line).
    let stdout_handle = child.stdout.take();
    let _stderr_handle = child.stderr.take();

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if start.elapsed() > GIT_TIMEOUT {
                    // Timeout — mirror kill_process_tree + bounded 1s drain
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
                            // bounded 2s for taskkill itself (mirrors kill_process_tree timeout=2)
                            let s = Instant::now();
                            loop {
                                match c.try_wait() {
                                    Ok(Some(_)) => break Ok(()),
                                    Ok(None) => {
                                        if s.elapsed() > Duration::from_secs(2) {
                                            let _ = c.kill();
                                            break Ok(());
                                        }
                                        thread::sleep(Duration::from_millis(10));
                                    }
                                    Err(e) => break Err(e),
                                }
                            }
                        });
                    }
                    // Bounded 1s post-kill drain — mirrors bounded_probe_run communicate(timeout=1)
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
                            Ok(None) => thread::sleep(Duration::from_millis(10)),
                            Err(_) => break,
                        }
                    }
                    if !terminated {
                        // Pipes still held by descendant — abandon (daemon reader threads)
                        return String::new();
                    }
                    // Even if drain succeeded, timeout is failure → empty
                    let _ = final_status;
                    return String::new();
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break None,
        }
    };

    // Process finished within timeout — collect stdout
    let status = match status {
        Some(s) => s,
        None => return String::new(),
    };
    if !status.success() {
        return String::new();
    }

    // Read stdout (already taken). Use a bounded thread read to avoid blocking
    // forever if a descendant still holds the pipe (mirrors abandon after 1s).
    let out_str = if let Some(mut out) = stdout_handle {
        use std::io::Read;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(buf) => String::from_utf8_lossy(&buf).trim().to_string(),
            Err(_) => return String::new(), // drain didn't finish → treat as empty
        }
    } else {
        String::new()
    };
    out_str
}

/// ``git -C <cwd> <args>`` → stripped stdout, or ``""`` on any failure.
///
/// Mirrors `tui_gateway/git_probe.py::run_git`:
///
/// ```python
/// def run_git(cwd: str, *args: str) -> str:
///     if not cwd or not os.path.isdir(cwd):
///         return ""
///     return bounded_git_probe(["git", "-C", cwd, *args], timeout=_GIT_TIMEOUT)
/// ```
pub fn run_git(cwd: &str, args: &[&str]) -> String {
    if cwd.is_empty() || !Path::new(cwd).is_dir() {
        return String::new();
    }
    let mut argv = Vec::with_capacity(3 + args.len());
    argv.push("git".to_string());
    argv.push("-C".to_string());
    argv.push(cwd.to_string());
    for &a in args {
        argv.push(a.to_string());
    }
    bounded_git_probe_argv(&argv)
}

/// Current branch or short HEAD — mirrors `branch(cwd)`.
///
/// ```python
/// def branch(cwd: str) -> str:
///     return run_git(cwd, "branch", "--show-current") or run_git(cwd, "rev-parse", "--short", "HEAD")
/// ```
pub fn branch(cwd: &str) -> String {
    let b = run_git(cwd, &["branch", "--show-current"]);
    if !b.is_empty() {
        return b;
    }
    run_git(cwd, &["rev-parse", "--short", "HEAD"])
}

// ---------------------------------------------------------------------------
// _RootCache — thread-safe single-flight cache
// ---------------------------------------------------------------------------

struct Inner {
    roots: HashMap<String, String>,
    neg: HashMap<String, Instant>,
    inflight: HashMap<String, Arc<(Mutex<bool>, Condvar)>>,
}

/// Thread-safe, single-flight cache of git-root probes.
///
/// Mirrors `tui_gateway/git_probe.py::_RootCache`.
pub struct RootCache {
    inner: Mutex<Inner>,
}

impl RootCache {
    /// Create an empty cache. Mirrors `_RootCache.__init__`.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                roots: HashMap::new(),
                neg: HashMap::new(),
                inflight: HashMap::new(),
            }),
        }
    }

    /// Drop cached roots after a known mutation. Mirrors `invalidate`.
    pub fn invalidate(&self) {
        let mut g = self.inner.lock().unwrap();
        g.roots.clear();
        g.neg.clear();
        g.inflight.clear();
    }

    /// Resolve `key` via `probe`, with single-flight dedup.
    ///
    /// Mirrors `_RootCache.resolve`:
    ///
    /// ```python
    /// def resolve(self, key: str, probe) -> str:
    ///     while True:
    ///         with self._lock:
    ///             hit = self._roots.get(key)
    ///             if hit: return hit
    ///             expiry = self._neg.get(key)
    ///             if expiry is not None:
    ///                 if expiry > time.monotonic(): return ""
    ///                 del self._neg[key]
    ///             gate = self._inflight.get(key)
    ///             if gate is None: gate=Event(); self._inflight[key]=gate; leader=True
    ///             else: leader=False
    ///         if not leader: gate.wait(timeout=_GIT_TIMEOUT+0.5); continue
    ///         value = probe()
    ///         with self._lock:
    ///             if value: self._roots[key]=value
    ///             else: self._neg[key]=time.monotonic()+_NEG_TTL
    ///             self._inflight.pop(key,None)
    ///         gate.set()
    ///         return value
    /// ```
    pub fn resolve<F>(&self, key: &str, probe: F) -> String
    where
        F: FnOnce() -> String,
    {
        let mut probe_opt = Some(probe);
        loop {
            let gate: Arc<(Mutex<bool>, Condvar)>;
            let is_leader: bool;
            {
                let mut inner = self.inner.lock().unwrap();
                if let Some(hit) = inner.roots.get(key) {
                    if !hit.is_empty() {
                        return hit.clone();
                    }
                }
                if let Some(expiry) = inner.neg.get(key).copied() {
                    if expiry > Instant::now() {
                        return String::new();
                    } else {
                        inner.neg.remove(key);
                    }
                }
                if let Some(existing) = inner.inflight.get(key) {
                    gate = Arc::clone(existing);
                    is_leader = false;
                } else {
                    gate = Arc::new((Mutex::new(false), Condvar::new()));
                    inner.inflight.insert(key.to_string(), Arc::clone(&gate));
                    is_leader = true;
                }
            }

            if !is_leader {
                let (lock, cvar) = &*gate;
                let guard = lock.lock().unwrap();
                if !*guard {
                    let timeout = Duration::from_secs_f64(GIT_TIMEOUT_SECS + 0.5);
                    let (g, _) = cvar.wait_timeout(guard, timeout).unwrap();
                    let _ = g;
                }
                continue;
            }

            // Leader — run probe without holding the lock (followers wait on gate)
            let probe = probe_opt.take().expect("probe called once per resolve");
            let value = probe();

            {
                let mut inner = self.inner.lock().unwrap();
                if !value.is_empty() {
                    inner.roots.insert(key.to_string(), value.clone());
                } else {
                    inner
                        .neg
                        .insert(key.to_string(), Instant::now() + NEG_TTL);
                }
                inner.inflight.remove(key);
                let (lock, cvar) = &*gate;
                *lock.lock().unwrap() = true;
                cvar.notify_all();
            }
            return value;
        }
    }

    /// Number of positive entries (test helper).
    #[cfg(test)]
    pub fn len_roots(&self) -> usize {
        self.inner.lock().unwrap().roots.len()
    }

    /// Number of negative entries (test helper).
    #[cfg(test)]
    pub fn len_neg(&self) -> usize {
        self.inner.lock().unwrap().neg.len()
    }

    /// Whether `key` is inflight (test helper).
    #[cfg(test)]
    pub fn is_inflight(&self, key: &str) -> bool {
        self.inner.lock().unwrap().inflight.contains_key(key)
    }
}

impl Default for RootCache {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_CACHE: OnceLock<RootCache> = OnceLock::new();

fn global_cache() -> &'static RootCache {
    GLOBAL_CACHE.get_or_init(RootCache::new)
}

/// Drop cached roots after a known mutation. Mirrors `git_probe.invalidate()`.
///
/// ```python
/// def invalidate() -> None:
///     _cache.invalidate()
/// ```
pub fn invalidate() {
    global_cache().invalidate();
}

// ---------------------------------------------------------------------------
// repo_root / common_repo_root / resolve
// ---------------------------------------------------------------------------

/// Top-level git repo root for `cwd` (`""` when not a repo).
///
/// Mirrors `repo_root(cwd)`:
///
/// ```python
/// def repo_root(cwd: str) -> str:
///     if not cwd: return ""
///     return _cache.resolve(cwd, lambda: run_git(cwd, "rev-parse", "--show-toplevel"))
/// ```
pub fn repo_root(cwd: &str) -> String {
    if cwd.is_empty() {
        return String::new();
    }
    let cwd_owned = cwd.to_string();
    global_cache().resolve(cwd, move || {
        run_git(&cwd_owned, &["rev-parse", "--show-toplevel"])
    })
}

/// The MAIN (common) repo root for `cwd`, folding linked worktrees.
///
/// Mirrors `common_repo_root(cwd)` — see Python docstring for the
/// `--show-toplevel` vs `--git-common-dir` folding and the forward-slash
/// normalization that prevents the main checkout from being misread as a
/// linked worktree.
///
/// ```python
/// def common_repo_root(cwd: str) -> str:
///     if not cwd: return ""
///     if not repo_root(cwd): return ""
///     def _probe() -> str:
///         gitdir = run_git(cwd, "rev-parse", "--path-format=absolute", "--git-common-dir")
///         if gitdir:
///             gitdir = os.path.realpath(gitdir)
///             if os.path.basename(gitdir) == ".git":
///                 return os.path.dirname(gitdir).replace(os.sep, "/")
///         return repo_root(cwd)
///     return _cache.resolve(f"common:{cwd}", _probe)
/// ```
pub fn common_repo_root(cwd: &str) -> String {
    if cwd.is_empty() {
        return String::new();
    }
    if repo_root(cwd).is_empty() {
        return String::new();
    }
    let cwd_owned = cwd.to_string();
    let key = format!("common:{}", cwd);
    global_cache().resolve(&key, move || {
        let gitdir = run_git(
            &cwd_owned,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        );
        if !gitdir.is_empty() {
            let real = realpath(&gitdir);
            let p = Path::new(&real);
            if p.file_name().map(|n| n == ".git").unwrap_or(false) {
                if let Some(parent) = p.parent() {
                    // Mirrors os.path.dirname(gitdir).replace(os.sep, "/")
                    // os.sep is platform-native; normalize to forward slashes so
                    // the common root compares equal to repo_root's raw toplevel.
                    let s = parent.to_string_lossy().to_string();
                    return s.replace('\\', "/");
                }
            }
        }
        repo_root(&cwd_owned)
    })
}

fn realpath(p: &str) -> String {
    match std::fs::canonicalize(Path::new(p)) {
        Ok(pb) => {
            // On Windows canonicalize returns \\?\C:\... — strip the verbatim prefix
            // so basename checks still see ".git" and dirname comparisons stay stable.
            let s = pb.to_string_lossy().to_string();
            if s.starts_with(r"\\?\") {
                s[4..].to_string()
            } else {
                s
            }
        }
        Err(_) => p.to_string(),
    }
}

/// Result of [`resolve`] — mirrors `{"repo_root": ..., "worktree_root": ...}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveResult {
    /// Common repo root (folded for worktrees).
    pub repo_root: String,
    /// This checkout's own toplevel.
    pub worktree_root: String,
}

/// Inject-able resolver for `project_tree.build_tree`.
///
/// Mirrors `resolve(cwd)`:
///
/// ```python
/// def resolve(cwd: str) -> dict | None:
///     worktree_root = repo_root(cwd)
///     if not worktree_root: return None
///     return {"repo_root": common_repo_root(cwd) or worktree_root, "worktree_root": worktree_root}
/// ```
pub fn resolve(cwd: &str) -> Option<ResolveResult> {
    let worktree_root = repo_root(cwd);
    if worktree_root.is_empty() {
        return None;
    }
    let common = common_repo_root(cwd);
    let repo_root_val = if common.is_empty() {
        worktree_root.clone()
    } else {
        common
    };
    Some(ResolveResult {
        repo_root: repo_root_val,
        worktree_root,
    })
}

// ---------------------------------------------------------------------------
// warm_roots
// ---------------------------------------------------------------------------

/// Pre-resolve many cwds' roots in parallel (bounded) so a cold first paint
/// doesn't serialize one git subprocess per session cwd.
///
/// Mirrors `warm_roots(cwds, max_workers=_WARM_WORKERS)`:
///
/// ```python
/// def warm_roots(cwds: Iterable[str], max_workers: int = _WARM_WORKERS) -> None:
///     pending = sorted({(cwd or "").strip() for cwd in cwds} - {""})
///     if not pending: return
///     if len(pending) == 1: resolve(pending[0]); return
///     with ThreadPoolExecutor(max_workers=min(max_workers, len(pending))) as pool:
///         list(pool.map(resolve, pending))
/// ```
pub fn warm_roots<I, S>(cwds: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    warm_roots_with_workers(cwds, WARM_WORKERS)
}

/// Like [`warm_roots`] but with an explicit worker cap.
pub fn warm_roots_with_workers<I, S>(cwds: I, max_workers: usize)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut set: HashSet<String> = HashSet::new();
    for cwd in cwds {
        let s = cwd.as_ref().trim().to_string();
        if !s.is_empty() {
            set.insert(s);
        }
    }
    let mut pending: Vec<String> = set.into_iter().collect();
    pending.sort();
    if pending.is_empty() {
        return;
    }
    if pending.len() == 1 {
        let _ = resolve(&pending[0]);
        return;
    }
    let workers = std::cmp::min(max_workers, pending.len()).max(1);
    let queue = Arc::new(Mutex::new(VecDeque::from(pending)));
    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let q = Arc::clone(&queue);
        let h = thread::spawn(move || loop {
            let cwd = {
                let mut g = q.lock().unwrap();
                g.pop_front()
            };
            match cwd {
                Some(c) => {
                    let _ = resolve(&c);
                }
                None => break,
            }
        });
        handles.push(h);
    }
    for h in handles {
        let _ = h.join();
    }
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_dir(suffix: &str) -> String {
        let base = std::env::temp_dir().join(format!(
            "hermes-git-probe-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        ));
        let _ = fs::create_dir_all(&base);
        base.to_string_lossy().to_string()
    }

    #[test]
    fn constants_match_python() {
        assert!((GIT_TIMEOUT_SECS - 1.5).abs() < 1e-9);
        assert_eq!(GIT_TIMEOUT, Duration::from_millis(1500));
        assert_eq!(WARM_WORKERS, 8);
        assert!((NEG_TTL_SECS - 30.0).abs() < 1e-9);
        assert_eq!(NEG_TTL, Duration::from_secs(30));
    }

    #[test]
    fn run_git_empty_and_missing_is_empty() {
        assert_eq!(run_git("", &["rev-parse", "--show-toplevel"]), "");
        assert_eq!(
            run_git("/nonexistent_xyz_12345_no_such_dir", &["rev-parse", "--show-toplevel"]),
            ""
        );
    }

    #[test]
    fn run_git_non_repo_is_empty() {
        let dir = tmp_dir("nonrepo");
        // tmp dir exists but is not a git repo → git exits non-zero → ""
        assert_eq!(run_git(&dir, &["rev-parse", "--show-toplevel"]), "");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repo_root_empty_is_empty() {
        assert_eq!(repo_root(""), "");
    }

    #[test]
    fn common_repo_root_empty_and_non_repo_is_empty() {
        assert_eq!(common_repo_root(""), "");
        let dir = tmp_dir("common-nonrepo");
        assert_eq!(common_repo_root(&dir), "");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_non_repo_is_none() {
        let dir = tmp_dir("resolve-nonrepo");
        assert!(resolve(&dir).is_none());
        assert!(resolve("").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn branch_non_repo_is_empty() {
        let dir = tmp_dir("branch-nonrepo");
        assert_eq!(branch(&dir), "");
        assert_eq!(branch(""), "");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn root_cache_positive_cached_forever() {
        let cache = RootCache::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::clone(&calls);
        let v1 = cache.resolve("k1", move || {
            c1.fetch_add(1, Ordering::SeqCst);
            "hit".to_string()
        });
        assert_eq!(v1, "hit");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Second resolve should not call probe
        let v2 = cache.resolve("k1", || panic!("should be cached"));
        assert_eq!(v2, "hit");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.len_roots(), 1);
    }

    #[test]
    fn root_cache_negative_cached_short_ttl() {
        let cache = RootCache::new();
        let v1 = cache.resolve("neg1", || "".to_string());
        assert_eq!(v1, "");
        assert_eq!(cache.len_neg(), 1);
        // Immediate second resolve should be cached negative (no probe)
        let v2 = cache.resolve("neg1", || panic!("negative should be cached"));
        assert_eq!(v2, "");
    }

    #[test]
    fn root_cache_invalidate_clears_all() {
        let cache = RootCache::new();
        let _ = cache.resolve("a", || "val".to_string());
        let _ = cache.resolve("b", || "".to_string());
        assert_eq!(cache.len_roots(), 1);
        assert_eq!(cache.len_neg(), 1);
        cache.invalidate();
        assert_eq!(cache.len_roots(), 0);
        assert_eq!(cache.len_neg(), 0);
    }

    #[test]
    fn root_cache_single_flight_dedups_concurrent_probes() {
        let cache = Arc::new(RootCache::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let c = Arc::clone(&cache);
            let ctr = Arc::clone(&calls);
            handles.push(thread::spawn(move || {
                c.resolve("single", move || {
                    ctr.fetch_add(1, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(50));
                    "once".to_string()
                })
            }));
        }
        let mut results = Vec::new();
        for h in handles {
            results.push(h.join().unwrap());
        }
        for r in &results {
            assert_eq!(r, "once");
        }
        // Single-flight should collapse to 1 probe (leader) — allow small race where 2 leaders interleave
        // but with Event semantics it should be 1.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn warm_roots_empty_and_single_fast_path() {
        // Empty → no panic
        warm_roots(Vec::<String>::new());
        warm_roots(Vec::<&str>::new());
        // Single → direct resolve path (non-repo → None)
        let dir = tmp_dir("warm-single");
        warm_roots(vec![dir.clone()]);
        // dedup + trim + sort: duplicates and whitespace and empty filtered
        warm_roots(vec![
            dir.clone(),
            dir.clone(),
            format!("  {}  ", dir),
            "".to_string(),
            "   ".to_string(),
        ]);
        warm_roots_with_workers(vec![dir.clone(), "".to_string()], 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn warm_roots_parallel_bounded() {
        let dirs: Vec<String> = (0..6).map(|i| tmp_dir(&format!("warm-par-{}", i))).collect();
        // All non-repo, so each resolve will be negative-cached; parallel should not panic
        warm_roots_with_workers(dirs.clone(), 2);
        // Second warm should hit negative cache quickly
        warm_roots_with_workers(dirs.clone(), 8);
        for d in dirs {
            let _ = fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn realpath_fallback_and_git_suffix() {
        // Fallback when path doesn't exist
        let p = "/no/such/path/.git";
        assert_eq!(realpath(p), p);
        // When path exists and is a dir, canonicalize may succeed — check .git handling via common logic
        // Just verify is_git detection: file_name == ".git"
        assert!(Path::new("/a/b/.git").file_name().unwrap() == ".git");
        assert!(Path::new("/a/b/c").file_name().unwrap() != ".git");
    }

    #[test]
    fn resolve_result_shape() {
        // Non-repo → None, so shape not exercised without a real repo; verify construction
        let r = ResolveResult {
            repo_root: "/a".to_string(),
            worktree_root: "/a".to_string(),
        };
        assert_eq!(r.repo_root, r.worktree_root);
    }
}
