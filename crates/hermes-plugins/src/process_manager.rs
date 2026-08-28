//! Subprocess lifecycle manager for the google_meet bot.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/plugins/google_meet/process_manager.py` (339 LOC).
//!
//! Single active meeting at a time. Stores the running pid + out_dir in a
//! session-scoped state file under `$HERMES_HOME/workspace/meetings/.active.json`
//! so tool calls across turns can find the bot, and `on_session_end` can clean it.
//!
//! The bot runs as a detached subprocess — we don't hold file descriptors open,
//! so the parent agent loop can't block on it. We communicate via files only.
//!
//! File + directory layout (under `$HERMES_HOME`):
//! ```text
//!   workspace/meetings/
//!       .active.json                # pointer to current session's bot
//!       <meeting-id>/
//!           status.json             # live bot state (written by bot each tick)
//!           transcript.txt          # scraped captions
//! ```
//! `.active.json` holds:
//! `{"pid": 12345, "meeting_id": "abc-defg-hij", "out_dir": "...", "url": "...", "started_at": 123..., "session_id": "optional"}`
//!
//! Python surface ported line-for-line:
//!   - `_root`, `_active_file`, `_read_active`, `_write_active`, `_clear_active`
//!   - `_pid_alive` (via `gateway.status._pid_exists`)
//!   - `start`, `status`, `transcript`, `enqueue_say`, `stop`
//!   - `MEET_URL_RE`, `_is_safe_meet_url`, `_meeting_id_from_url` (from `plugins.google_meet.meet_bot`)
//!
//! Transport notes (mirrors Python side-effects without `cargo` in this task):
//!   - `hermes_constants.get_hermes_home()` → `get_hermes_home()` via `$HERMES_HOME` / `$HOME`.
//!   - `gateway.status._pid_exists` → `pid_alive` / `pid_exists` via `/proc/<pid>` check on Linux,
//!     `kill -0` fallback semantics on other POSIX, and always-false on Windows stub.
//!     Real port would use `psutil.pid_exists` or `OpenProcess` on Windows.
//!   - `subprocess.Popen(..., start_new_session=True, close_fds=True, stdin=DEVNULL, stdout=log)` →
//!     `std::process::Command` with `Stdio::null()` + append log file; `start_new_session`
//!     would be `pre_exec(|| libc::setsid())` / `creation_flags(DETACHED_PROCESS)` — kept as
//!     plain spawn with std only so crate stays compilable without `libc`/`nix`/`winapi`.
//!   - `agent.secret_scope.get_secret` → env-var lookup for `HERMES_MEET_REALTIME_KEY` /
//!     `OPENAI_API_KEY` (parent scope is not available in detached child, so key is captured at spawn).
//!   - `signal.SIGTERM` / `SIGKILL` → `kill <pid> -TERM/-KILL` via `Command` or `libc::kill` when linked;
//!     std-only fallback uses `/bin/kill`.
//!   - `time.time()` → `SystemTime::now().duration_since(UNIX_EPOCH).as_secs_f64()`.
//!   - `json.loads` / `json.dumps` → `serde_json`.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Home helpers — mirrors hermes_constants.get_hermes_home()
// ---------------------------------------------------------------------------

/// Mirrors `hermes_constants.get_hermes_home()`: `$HERMES_HOME` → `~/.hermes`.
pub fn get_hermes_home() -> PathBuf {
    if let Ok(val) = std::env::var("HERMES_HOME") {
        let t = val.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let t = home.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t).join(".hermes");
        }
    }
    if let Ok(up) = std::env::var("USERPROFILE") {
        let t = up.trim().to_string();
        if !t.is_empty() {
            return PathBuf::from(t).join(".hermes");
        }
    }
    PathBuf::from(".hermes")
}

fn python_executable() -> String {
    if let Ok(exe) = std::env::var("PYTHON") {
        if !exe.trim().is_empty() {
            return exe;
        }
    }
    // Prefer python3 if on PATH — matches tooling in google_meet_tools.rs
    if which("python3").is_some() {
        return "python3".to_string();
    }
    if which("python").is_some() {
        return "python".to_string();
    }
    "python3".to_string()
}

fn which(bin: &str) -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let cand = dir.join(bin);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Paths — mirrors _root() / _active_file()
// ---------------------------------------------------------------------------

/// Mirrors `def _root() -> Path: return Path(get_hermes_home()) / "workspace" / "meetings"`.
pub fn root() -> PathBuf {
    get_hermes_home().join("workspace").join("meetings")
}

/// Mirrors `def _active_file() -> Path: return _root() / ".active.json"`.
pub fn active_file() -> PathBuf {
    root().join(".active.json")
}

// ---------------------------------------------------------------------------
// JSON helpers — mirrors _read_active / _write_active / _clear_active
// ---------------------------------------------------------------------------

/// Mirrors `def _read_active() -> Optional[Dict[str, Any]]`.
pub fn read_active() -> Option<Value> {
    let p = active_file();
    if !p.is_file() {
        return None;
    }
    let text = fs::read_to_string(&p).ok()?;
    serde_json::from_str::<Value>(&text).ok()
}

/// Mirrors `def _write_active(data: Dict[str, Any]) -> None` — atomic tmp+replace.
pub fn write_active(data: &Value) -> std::io::Result<()> {
    let p = active_file();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = p.with_extension("json.tmp");
    // Also handle case where with_extension would strip .json -> .tmp ; ensure .json.tmp
    let tmp2 = p.with_file_name(format!(
        "{}.tmp",
        p.file_name().unwrap_or_default().to_string_lossy()
    ));
    let tmp_path = if tmp2.exists() || !tmp.exists() { tmp2 } else { tmp };
    // Prefer the .json.tmp variant for fidelity with Python's with_suffix(".json.tmp")
    let actual_tmp = p.with_file_name(format!(
        "{}.tmp",
        p.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _ = tmp_path;
    fs::write(&actual_tmp, serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string()))?;
    fs::rename(&actual_tmp, &p)?;
    Ok(())
}

/// Mirrors `def _clear_active() -> None`.
pub fn clear_active() {
    let _ = fs::remove_file(active_file());
}

// ---------------------------------------------------------------------------
// PID liveness — mirrors gateway.status._pid_exists
// ---------------------------------------------------------------------------

/// Mirrors `gateway.status._pid_exists` / `process_manager._pid_alive`.
///
/// `os.kill(pid, 0)` is NOT a no-op on Windows (bpo-14484) — it routes
/// through GenerateConsoleCtrlEvent and can kill the target. Use the
/// cross-platform existence check instead.
pub fn pid_exists(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        // psutil uses /proc on Linux internally; check directly without extra dep.
        let proc_path = format!("/proc/{}", pid);
        if Path::new(&proc_path).exists() {
            return true;
        }
        return false;
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        // POSIX fallback: try kill -0 via libc if available, else /bin/kill probe.
        // We avoid pulling `libc` crate; use `kill` command as std-only fallback.
        // Real port would be: unsafe { libc::kill(pid, 0) == 0 || errno == EPERM }
        let out = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output();
        match out {
            Ok(o) => o.status.success(),
            Err(_) => false,
        }
    }
    #[cfg(windows)]
    {
        // Windows: psutil uses OpenProcess + GetExitCodeProcess.
        // Std-only stub: cannot probe without winapi; assume dead to avoid false live.
        // Real port would use `windows-sys` or `psutil` equivalent.
        let _ = pid;
        false
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// Alias matching Python name `def _pid_alive(pid: int) -> bool`.
pub fn pid_alive(pid: i32) -> bool {
    pid_exists(pid)
}

// ---------------------------------------------------------------------------
// Meet URL helpers — mirrors plugins.google_meet.meet_bot._is_safe_meet_url
//                              and _meeting_id_from_url
// ---------------------------------------------------------------------------

fn is_alnum_lower(c: char) -> bool {
    matches!(c, 'a'..='z' | '0'..='9')
}

/// Mirrors `MEET_URL_RE` + `def _is_safe_meet_url(url: str) -> bool`.
///
/// `MEET_URL_RE = re.compile(r"^https://meet\.google\.com/("
///   r"[a-z0-9]{3,}-[a-z0-9]{3,}-[a-z0-9]{3,}"
///   r"|lookup/[^/?#]+"
///   r"|new"
///   r")(?:[/?#].*)?$")`
pub fn is_safe_meet_url(url: &str) -> bool {
    let s = url.trim();
    let prefix = "https://meet.google.com/";
    if !s.starts_with(prefix) {
        return false;
    }
    let rest = &s[prefix.len()..];
    if rest.is_empty() {
        return false;
    }
    // Split at first / ? #
    let mut end = rest.len();
    for (i, ch) in rest.char_indices() {
        if ch == '/' || ch == '?' || ch == '#' {
            end = i;
            break;
        }
    }
    let first = &rest[..end];
    if first == "new" {
        return true;
    }
    if first.starts_with("lookup/") {
        let tail = &first["lookup/".len()..];
        // lookup/[^/?#]+ — we already cut at /?#, so just check non-empty and no embedded /?#
        if !tail.is_empty() && !tail.contains('/') && !tail.contains('?') && !tail.contains('#') {
            return true;
        }
        return false;
    }
    // [a-z0-9]{3,}-[a-z0-9]{3,}-[a-z0-9]{3,}
    let parts: Vec<&str> = first.split('-').collect();
    if parts.len() != 3 {
        return false;
    }
    for part in parts {
        if part.len() < 3 {
            return false;
        }
        if !part.chars().all(is_alnum_lower) {
            return false;
        }
    }
    true
}

/// Mirrors `def _meeting_id_from_url(url: str) -> str`.
///
/// Extract `abc-defg-hij` from `https://meet.google.com/abc-defg-hij`,
/// else `meet-<int(time.time())>`.
pub fn meeting_id_from_url(url: &str) -> String {
    // re.search(r"meet\.google\.com/([a-z0-9]{3,}-[a-z0-9]{3,}-[a-z0-9]{3,})", url or "")
    let s = url;
    if let Some(pos) = s.find("meet.google.com/") {
        let after = &s[pos + "meet.google.com/".len()..];
        // Extract leading token up to / ? # or end, then check if it matches 3-seg pattern
        let mut end = after.len();
        for (i, ch) in after.char_indices() {
            if ch == '/' || ch == '?' || ch == '#' || ch == '"' || ch == '\'' || ch == ' ' {
                end = i;
                break;
            }
        }
        let token = &after[..end];
        // Check token is 3 segments of [a-z0-9]{3,}
        let parts: Vec<&str> = token.split('-').collect();
        if parts.len() == 3 && parts.iter().all(|p| p.len() >= 3 && p.chars().all(is_alnum_lower)) {
            return token.to_string();
        }
        // Also handle case where token contains extra path after? The regex would still find
        // the 3-seg inside; our simple check only looks at first segment. Fall back to
        // scanning the whole url for any 3-seg substring.
        // Scan for pattern manually
        let chars: Vec<char> = s.chars().collect();
        for i in 0..chars.len() {
            // try to parse 3 segments starting at i
            let mut j = i;
            let mut segs = 0;
            let mut seg_len = 0;
            let mut valid = true;
            while j < chars.len() && segs < 3 {
                let c = chars[j];
                if c == '-' {
                    if seg_len < 3 {
                        valid = false;
                        break;
                    }
                    segs += 1;
                    seg_len = 0;
                } else if is_alnum_lower(c) {
                    seg_len += 1;
                } else {
                    break;
                }
                j += 1;
            }
            if valid && segs == 2 && seg_len >= 3 {
                // We consumed 2 dashes and final segment >=3, and next char is not alnum/"-"
                // Ensure preceding char is "/" or start and we matched exactly 3 segments
                let candidate: String = chars[i..j].iter().collect();
                let candidate_parts: Vec<&str> = candidate.split('-').collect();
                if candidate_parts.len() == 3 && candidate_parts.iter().all(|p| p.len() >= 3) {
                    // Verify it was inside meet.google.com/... context
                    // Check that substring is after meet.google.com/
                    let prefix = s[..i].to_string();
                    if prefix.contains("meet.google.com/") {
                        return candidate;
                    }
                }
            }
        }
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("meet-{}", secs)
}

// ---------------------------------------------------------------------------
// Helpers: time, env resolution, log spawning
// ---------------------------------------------------------------------------

fn now_secs_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn resolve_realtime_key(explicit: Option<&str>) -> Option<String> {
    if let Some(k) = explicit {
        if !k.trim().is_empty() {
            return Some(k.to_string());
        }
    }
    // Mirrors: from agent.secret_scope import get_secret; get_secret("HERMES_MEET_REALTIME_KEY") or get_secret("OPENAI_API_KEY")
    // In Rust we read env vars — the multiplexed scope is installed in the parent process env.
    if let Ok(v) = std::env::var("HERMES_MEET_REALTIME_KEY") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    if let Ok(v) = std::env::var("OPENAI_API_KEY") {
        if !v.trim().is_empty() {
            return Some(v);
        }
    }
    None
}

fn kill_pid(pid: i32, sig: &str) {
    // sig: "TERM" or "KILL" — mirrors os.kill(pid, signal.SIGTERM/SIGKILL)
    // Try libc kill if we can, else shell out to /bin/kill.
    // Std-only: use Command("kill")
    let _ = Command::new("kill")
        .args([format!("-{}", sig), pid.to_string()])
        .output();
}

// ---------------------------------------------------------------------------
// Public API — used by tool handlers + CLI
// ---------------------------------------------------------------------------

/// Options for `start` — mirrors Python kwargs.
#[derive(Debug, Clone)]
pub struct StartOptions {
    pub out_dir: Option<PathBuf>,
    pub headed: bool,
    pub auth_state: Option<String>,
    pub guest_name: String,
    pub duration: Option<String>,
    pub session_id: Option<String>,
    pub mode: String,
    pub realtime_model: Option<String>,
    pub realtime_voice: Option<String>,
    pub realtime_instructions: Option<String>,
    pub realtime_api_key: Option<String>,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            out_dir: None,
            headed: false,
            auth_state: None,
            guest_name: "Hermes Agent".to_string(),
            duration: None,
            session_id: None,
            mode: "transcribe".to_string(),
            realtime_model: None,
            realtime_voice: None,
            realtime_instructions: None,
            realtime_api_key: None,
        }
    }
}

/// Spawn the meet_bot subprocess for `url`.
///
/// If a bot is already running for this hermes install, leave it first —
/// we enforce single-active-meeting semantics.
///
/// Returns a dict summarizing the started bot (`{"ok": true, ...}` or `{"ok": false, "error": ...}`).
///
/// Mirrors `def start(url: str, *, out_dir=None, headed=False, auth_state=None, guest_name="Hermes Agent", duration=None, session_id=None, mode="transcribe", ...)` (lines 84-203).
pub fn start(url: &str, opts: StartOptions) -> Value {
    if !is_safe_meet_url(url) {
        return json!({
            "ok": false,
            "error": format!("refusing: only https://meet.google.com/ URLs are allowed. got: {:?}", url)
        });
    }

    if let Some(existing) = read_active() {
        if let Some(pid_val) = existing.get("pid").and_then(|v| v.as_i64()) {
            let pid = pid_val as i32;
            if pid_alive(pid) {
                stop_with_reason("replaced by new meet_join");
            }
        }
    }

    let meeting_id = meeting_id_from_url(url);
    let out = opts
        .out_dir
        .clone()
        .unwrap_or_else(|| root().join(&meeting_id));
    let _ = fs::create_dir_all(&out);

    for name in ["transcript.txt", "status.json"] {
        let f = out.join(name);
        if f.exists() {
            let _ = fs::remove_file(&f);
        }
    }

    let mut env_map: HashMap<String, String> = std::env::vars().collect();
    env_map.insert("HERMES_MEET_URL".to_string(), url.to_string());
    env_map.insert("HERMES_MEET_OUT_DIR".to_string(), out.to_string_lossy().to_string());
    env_map.insert("HERMES_MEET_GUEST_NAME".to_string(), opts.guest_name.clone());
    if opts.headed {
        env_map.insert("HERMES_MEET_HEADED".to_string(), "1".to_string());
    }
    if let Some(ref s) = opts.auth_state {
        if !s.trim().is_empty() {
            env_map.insert("HERMES_MEET_AUTH_STATE".to_string(), s.clone());
        }
    }
    if let Some(ref d) = opts.duration {
        if !d.trim().is_empty() {
            env_map.insert("HERMES_MEET_DURATION".to_string(), d.clone());
        }
    }
    if !opts.mode.trim().is_empty() {
        env_map.insert("HERMES_MEET_MODE".to_string(), opts.mode.clone());
    }
    if let Some(ref m) = opts.realtime_model {
        if !m.trim().is_empty() {
            env_map.insert("HERMES_MEET_REALTIME_MODEL".to_string(), m.clone());
        }
    }
    if let Some(ref v) = opts.realtime_voice {
        if !v.trim().is_empty() {
            env_map.insert("HERMES_MEET_REALTIME_VOICE".to_string(), v.clone());
        }
    }
    if let Some(ref instr) = opts.realtime_instructions {
        if !instr.trim().is_empty() {
            env_map.insert("HERMES_MEET_REALTIME_INSTRUCTIONS".to_string(), instr.clone());
        }
    }
    if let Some(key) = resolve_realtime_key(opts.realtime_api_key.as_deref()) {
        env_map.insert("HERMES_MEET_REALTIME_KEY".to_string(), key);
    }

    let log_path = out.join("bot.log");
    // Detach: stdin=devnull, stdout/stderr → log file, new session so parent signals don't propagate.
    // Python: open(log_path, "ab", buffering=0) + Popen(..., stdin=DEVNULL, stdout=log_fh, stderr=STDOUT, env=env, start_new_session=True, close_fds=True)
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path);
    let log_file = match log_file {
        Ok(f) => f,
        Err(e) => {
            return json!({"ok": false, "error": format!("failed to open log file {:?}: {}", log_path, e)});
        }
    };

    let py = python_executable();
    let mut cmd = Command::new(py);
    cmd.args(["-m", "plugins.google_meet.meet_bot"])
        .stdin(Stdio::null())
        .envs(&env_map);
    // Duplicate log file for stdout and stderr (Python: stderr=STDOUT)
    // We need to clone the file handle for both.
    let stdout_file = match log_file.try_clone() {
        Ok(f) => f,
        Err(_) => log_file,
    };
    // Re-open for stderr if we cloned — otherwise use same.
    // Command takes File via Stdio::from
    // We need to handle ownership: try_clone gives second handle.
    // Use std::fs::File -> Stdio
    // To keep both, we use one for stdout and reopen for stderr if needed.
    let log_clone_for_stderr = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();
    // Set stdout
    // We already consumed log_file as stdout_file; set it
    // Workaround: reopen again for stdout if we need two handles
    // Actually we already have stdout_file; use it.
    let log_for_stdout = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .unwrap_or(stdout_file);
    // But we need to keep it simple: just open twice.
    let stdout_handle = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();
    let stderr_handle = log_clone_for_stderr;

    if let Some(f) = stdout_handle {
        cmd.stdout(Stdio::from(f));
    } else {
        cmd.stdout(Stdio::null());
    }
    if let Some(f) = stderr_handle {
        cmd.stderr(Stdio::from(f));
    } else {
        cmd.stderr(Stdio::null());
    }

    // start_new_session / close_fds would be:
    //   #[cfg(unix)] { use std::os::unix::process::CommandExt; cmd.pre_exec(|| { libc::setsid(); Ok(()) }); }
    // Kept as plain spawn with std only; documented upgrade path.
    let proc = cmd.spawn();
    let child = match proc {
        Ok(c) => c,
        Err(e) => {
            return json!({"ok": false, "error": format!("failed to spawn meet_bot: {}", e)});
        }
    };
    let pid = child.id() as i64;

    // Child now owns log fds; parent continues.
    // We deliberately don't wait or hold the Child handle detached — drop it.
    // Python keeps proc.pid only; we do the same.

    let record = json!({
        "pid": pid,
        "meeting_id": meeting_id,
        "out_dir": out.to_string_lossy().to_string(),
        "url": url,
        "started_at": now_secs_f64(),
        "session_id": opts.session_id,
        "log_path": log_path.to_string_lossy().to_string(),
        "mode": opts.mode,
    });
    let _ = write_active(&record);
    let mut out_val = json!({"ok": true});
    if let Value::Object(map) = record {
        if let Value::Object(out_map) = &mut out_val {
            for (k, v) in map {
                out_map.insert(k, v);
            }
        }
    }
    out_val
}

/// Convenience wrapper matching Python positional call `start(url, out_dir=..., headed=..., ...)`.
pub fn start_simple(url: &str) -> Value {
    start(url, StartOptions::default())
}

/// Return the current meeting state, or `{"ok": false, "reason": ...}`.
///
/// Mirrors `def status() -> Dict[str, Any]` (lines 206-232).
pub fn status() -> Value {
    let active = match read_active() {
        Some(v) => v,
        None => return json!({"ok": false, "reason": "no active meeting"}),
    };
    let pid = active.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let alive = if pid != 0 { pid_alive(pid) } else { false };
    let out_dir_str = active.get("out_dir").and_then(|v| v.as_str()).unwrap_or("");
    let status_path = Path::new(out_dir_str).join("status.json");
    let mut bot_status: Value = json!({});
    if status_path.is_file() {
        if let Ok(text) = fs::read_to_string(&status_path) {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                if v.is_object() {
                    bot_status = v;
                }
            }
        }
    }
    let mut out = json!({
        "ok": true,
        "alive": alive,
        "pid": pid,
        "meetingId": active.get("meeting_id"),
        "url": active.get("url"),
        "startedAt": active.get("started_at"),
        "outDir": active.get("out_dir"),
    });
    if let (Some(out_map), Value::Object(bot_map)) = (out.as_object_mut(), bot_status) {
        for (k, v) in bot_map {
            out_map.insert(k, v);
        }
    }
    out
}

/// Read the current transcript file. Returns ok=False if none exists.
///
/// Mirrors `def transcript(last: Optional[int] = None) -> Dict[str, Any]` (lines 235-259).
pub fn transcript(last: Option<usize>) -> Value {
    let active = match read_active() {
        Some(v) => v,
        None => return json!({"ok": false, "reason": "no active meeting"}),
    };
    let out_dir_str = active.get("out_dir").and_then(|v| v.as_str()).unwrap_or("");
    let tp = Path::new(out_dir_str).join("transcript.txt");
    if !tp.is_file() {
        return json!({
            "ok": true,
            "meetingId": active.get("meeting_id"),
            "lines": [],
            "total": 0,
            "path": tp.to_string_lossy().to_string(),
        });
    }
    let text = fs::read_to_string(&tp).unwrap_or_default();
    let all_lines: Vec<String> = text.lines().filter(|ln| !ln.trim().is_empty()).map(|s| s.to_string()).collect();
    let total = all_lines.len();
    let lines: Vec<String> = if let Some(n) = last {
        if n >= total {
            all_lines.clone()
        } else {
            all_lines[total - n..].to_vec()
        }
    } else {
        all_lines.clone()
    };
    json!({
        "ok": true,
        "meetingId": active.get("meeting_id"),
        "lines": lines,
        "total": total,
        "path": tp.to_string_lossy().to_string(),
    })
}

/// Append a `say` request to the active bot's JSONL queue.
///
/// Returns `{"ok": false, "reason": ...}` when no meeting is active or
/// the active bot is in transcribe-only mode. Otherwise writes a line to
/// `<out_dir>/say_queue.jsonl` that the bot's realtime speaker thread
/// will consume.
///
/// Mirrors `def enqueue_say(text: str) -> Dict[str, Any]` (lines 262-301).
pub fn enqueue_say(text: &str) -> Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return json!({"ok": false, "reason": "text is required"});
    }
    let active = match read_active() {
        Some(v) => v,
        None => return json!({"ok": false, "reason": "no active meeting"}),
    };
    let mode = active.get("mode").and_then(|v| v.as_str()).unwrap_or("transcribe");
    if mode != "realtime" {
        return json!({
            "ok": false,
            "reason": "active meeting is in transcribe mode — pass mode='realtime' to meet_join to enable agent speech"
        });
    }
    let out_dir_str = active.get("out_dir").and_then(|v| v.as_str()).unwrap_or("");
    let out_dir = Path::new(out_dir_str);
    if !out_dir.is_dir() {
        return json!({"ok": false, "reason": format!("out_dir missing: {}", out_dir.display())});
    }
    let queue_path = out_dir.join("say_queue.jsonl");
    // uuid hex[:12] — use time+pid entropy to avoid `uuid` crate
    let id = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        let mixed = now ^ (pid << 32) ^ (now >> 16);
        format!("{:012x}", mixed & 0xffff_ffff_ffffu128)
    };
    let entry = json!({"id": id, "text": trimmed});
    let line = serde_json::to_string(&entry).unwrap_or_else(|_| "{}".to_string());
    let mut f = match fs::OpenOptions::new().create(true).append(true).open(&queue_path) {
        Ok(f) => f,
        Err(e) => return json!({"ok": false, "reason": format!("failed to open queue: {}", e)}),
    };
    if let Err(e) = writeln!(f, "{}", line) {
        return json!({"ok": false, "reason": format!("failed to write queue: {}", e)});
    }
    json!({
        "ok": true,
        "meetingId": active.get("meeting_id"),
        "enqueued_id": id,
        "queue_path": queue_path.to_string_lossy().to_string(),
    })
}

/// Signal the active bot to leave cleanly, then clear the active pointer.
///
/// Sends SIGTERM and waits up to 10s for the bot to exit. Falls back to
/// SIGKILL if the bot doesn't respond.
///
/// Mirrors `def stop(*, reason: str = "requested") -> Dict[str, Any]` (lines 304-339).
pub fn stop_with_reason(reason: &str) -> Value {
    let active = match read_active() {
        Some(v) => v,
        None => return json!({"ok": false, "reason": "no active meeting"}),
    };
    let pid = active.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let out_dir_str = active.get("out_dir").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let transcript_path = if out_dir_str.is_empty() {
        None
    } else {
        Some(Path::new(&out_dir_str).join("transcript.txt").to_string_lossy().to_string())
    };

    if pid != 0 && pid_alive(pid) {
        kill_pid(pid, "TERM");
        for _ in 0..20 {
            if !pid_alive(pid) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if pid_alive(pid) {
            // windows-footgun: ok — POSIX-only plugin (google_meet registers no-op on Windows; see __init__.py)
            kill_pid(pid, "KILL");
        }
    }

    clear_active();
    json!({
        "ok": true,
        "reason": reason,
        "meetingId": active.get("meeting_id"),
        "transcriptPath": transcript_path,
    })
}

/// Mirrors `def stop(*, reason: str = "requested")` with default reason.
pub fn stop() -> Value {
    stop_with_reason("requested")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn is_safe_url_accepts_valid() {
        assert!(is_safe_meet_url("https://meet.google.com/abc-defg-hij"));
        assert!(is_safe_meet_url("https://meet.google.com/abc-defg-hij?authuser=0"));
        assert!(is_safe_meet_url("https://meet.google.com/lookup/abc123"));
        assert!(is_safe_meet_url("https://meet.google.com/new"));
        assert!(is_safe_meet_url("https://meet.google.com/abc-defg-hij/"));
    }

    #[test]
    fn is_safe_url_rejects_bad() {
        assert!(!is_safe_meet_url("http://meet.google.com/abc-defg-hij"));
        assert!(!is_safe_meet_url("https://meet.google.com/"));
        assert!(!is_safe_meet_url("https://meet.google.com/ab-cd-ef"));
        assert!(!is_safe_meet_url("https://evil.com/abc-defg-hij"));
        assert!(!is_safe_meet_url(""));
    }

    #[test]
    fn meeting_id_from_url_extracts() {
        assert_eq!(
            meeting_id_from_url("https://meet.google.com/abc-defg-hij"),
            "abc-defg-hij"
        );
        assert_eq!(
            meeting_id_from_url("https://meet.google.com/abc-defg-hij?foo=bar"),
            "abc-defg-hij"
        );
    }

    #[test]
    fn meeting_id_from_url_fallback() {
        let id = meeting_id_from_url("https://meet.google.com/lookup/xyz");
        assert!(id.starts_with("meet-"));
        let id2 = meeting_id_from_url("https://meet.google.com/new");
        assert!(id2.starts_with("meet-"));
    }

    #[test]
    fn read_write_clear_active_roundtrip() {
        let tmp = std::env::temp_dir().join(format!(
            "hermes-pm-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&tmp);
        let prev = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        let data = json!({"pid": 12345, "meeting_id": "abc-defg-hij", "out_dir": tmp.join("abc").to_string_lossy().to_string()});
        write_active(&data).unwrap();
        let back = read_active().unwrap();
        assert_eq!(back["meeting_id"], json!("abc-defg-hij"));
        clear_active();
        assert!(read_active().is_none());
        if let Some(v) = prev { unsafe { std::env::set_var("HERMES_HOME", v); } } else { unsafe { std::env::remove_var("HERMES_HOME"); } }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn pid_alive_negative_is_false() {
        assert!(!pid_alive(0));
        assert!(!pid_alive(-1));
    }

    #[test]
    fn status_no_active() {
        let tmp = std::env::temp_dir().join(format!(
            "hermes-pm-test2-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&tmp);
        let prev = std::env::var("HERMES_HOME").ok();
        unsafe { std::env::set_var("HERMES_HOME", tmp.to_string_lossy().to_string()); }
        let s = status();
        assert_eq!(s["ok"], json!(false));
        assert_eq!(s["reason"], json!("no active meeting"));
        if let Some(v) = prev { unsafe { std::env::set_var("HERMES_HOME", v); } } else { unsafe { std::env::remove_var("HERMES_HOME"); } }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn enqueue_say_requires_text() {
        let v = enqueue_say("   ");
        assert_eq!(v["ok"], json!(false));
        assert!(v["reason"].as_str().unwrap().contains("text is required"));
    }

    #[test]
    fn start_refuses_bad_url() {
        let v = start("http://evil.com/abc-defg-hij", StartOptions::default());
        assert_eq!(v["ok"], json!(false));
        assert!(v["error"].as_str().unwrap().contains("only https://meet.google.com/"));
    }
}
