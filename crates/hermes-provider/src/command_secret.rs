//! `command` secret source — resolve secrets via a user-configured helper.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/secret_sources/command.py` (501 lines).
//!
//! Ports the security semantics of the desktop app's TypeScript
//! `CommandSecretsProvider` (`hermes-desktop src/main/secrets/commandProvider.ts`)
//! to the Rust provider crate. The helper command (e.g. `keepassxc-cli`,
//! `secret-tool`, or a script that cats a tmpfs env file) comes from
//! `secrets.command` in `config.yaml` — NEVER from `.env`, which holds
//! only secret values.
//!
//! Security model (mirrors the TS provider line-for-line where it matters):
//!
//! * The command string is the USER'S OWN configuration (same trust level as
//!   the `.env` file they control), so it is run via `/bin/sh -c <command>`.
//! * The requested key is passed to the child ONLY via the `HERMES_SECRET_KEY`
//!   environment variable — it is NEVER interpolated into the shell string, so
//!   a hostile key name (e.g. `"; rm -rf ~`) is inert data, not code.
//! * Hard timeout (default 3s) + output cap (default 1 MiB); any failure
//!   (non-zero exit, timeout, spawn failure, oversized output) degrades to
//!   "no value" rather than raising.
//! * Failures log ONLY structured fields (exit code / signal / errno) to
//!   stderr — never the command string, the helper's stderr, or any secret
//!   value. The helper's stderr is captured via a pipe and DISCARDED so its
//!   diagnostics (which can carry secret material) never reach our stderr.
//! * The startup/apply path runs the helper exactly ONCE (with an empty
//!   `HERMES_SECRET_KEY`) — it is never called per-key in a loop, so a
//!   helper that blocks (e.g. on a vault unlock prompt) can't be spawned
//!   dozens of times.
//! * PLATFORM: the provider is POSIX-only (needs `/bin/sh`). On Windows it
//!   degrades to an empty result with a warning; Windows users stay on the
//!   default `env` provider.
//!
//! T0040 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `os.name == "nt" or platform.system() == "Windows"` ↔ `cfg!(windows)`.
//! - Python `re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$")` ↔ hand-rolled `parse_env_line` without `regex` crate.
//! - Python `subprocess.Popen(..., start_new_session=True)` + `os.killpg(..., SIGKILL)` ↔ `Command` with `pre_exec(|| setsid())` + `kill -9 -<pgid>` fallback; crate stays std-only without `nix`/`libc`.
//! - Python `get_source_environment()` ContextVar ↔ `OnceLock<Mutex<Option<HashMap>>>` shim (same shape as `secret_registry.rs`).
//! - Python `Optional[Path]` ↔ `Option<PathBuf>` / `Option<&Path>`.
//! - Python `Dict[str,str]` ↔ `HashMap<String,String>`; `FetchResult` ↔ local `FetchResult` struct.
//! - Python `signal.Signals(-code).name` ↔ manual signal-name table for the common POSIX signals.
//! - `SecretSource` trait is re-declared here mirroring `base.SecretSource` ABC so this slice compiles standalone.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Constants — mirrors lines 64-71
// ---------------------------------------------------------------------------

/// Hard cap so a hung helper can never wedge startup. Kept deliberately
/// TIGHT (3s) — a configured helper MUST be fast and NON-INTERACTIVE.
/// Mirrors `_COMMAND_TIMEOUT_SECONDS = 3.0` (line 64).
pub const COMMAND_TIMEOUT_SECONDS: f64 = 3.0;

/// Defensive cap on helper output (1 MiB) — a misbehaving command can't OOM us.
/// Mirrors `_MAX_OUTPUT_BYTES = 1024 * 1024` (line 66).
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// Shared helpers — re-implemented for slice-local self-containment
// Mirrors `agent.secret_sources.base` + `agent.secret_sources.bitwarden.FetchResult`
// ---------------------------------------------------------------------------

/// Machine-readable failure taxonomy — mirrors `base.ErrorKind` (base.py lines 81-98).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    NotConfigured,
    BinaryMissing,
    AuthFailed,
    AuthExpired,
    RefInvalid,
    Network,
    EmptyValue,
    Timeout,
    Internal,
}

impl ErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::NotConfigured => "not_configured",
            ErrorKind::BinaryMissing => "binary_missing",
            ErrorKind::AuthFailed => "auth_failed",
            ErrorKind::AuthExpired => "auth_expired",
            ErrorKind::RefInvalid => "ref_invalid",
            ErrorKind::Network => "network",
            ErrorKind::EmptyValue => "empty_value",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Internal => "internal",
        }
    }
}

/// Outcome of one source's fetch — mirrors `base.FetchResult` (bitwarden.py lines 123-150 / command.py via bitwarden import).
#[derive(Debug, Clone, Default)]
pub struct FetchResult {
    pub secrets: HashMap<String, String>,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub error_kind: Option<ErrorKind>,
    pub binary_path: Option<PathBuf>,
}

impl FetchResult {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

/// Validate env-var name — mirrors `base.is_valid_env_name` (base.py lines 268-270).
/// Regex `^[A-Za-z_][A-Za-z0-9_]*$` without `regex` crate.
pub fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    true
}

// ContextVar shim for per-fetch environment — mirrors `base._SOURCE_ENVIRONMENT` (base.py lines 54-70).
static SOURCE_ENVIRONMENT: OnceLock<Mutex<Option<HashMap<String, String>>>> = OnceLock::new();

fn source_env_cell() -> &'static Mutex<Option<HashMap<String, String>>> {
    SOURCE_ENVIRONMENT.get_or_init(|| Mutex::new(None))
}

/// Install a per-fetch environment view — mirrors `base.set_source_environment`.
/// Returns the previous value as a token for `reset_source_environment`.
pub fn set_source_environment(environ: HashMap<String, String>) -> Option<HashMap<String, String>> {
    let cell = source_env_cell();
    let mut guard = cell.lock().unwrap();
    let prev = guard.clone();
    *guard = Some(environ);
    prev
}

pub fn reset_source_environment(token: Option<HashMap<String, String>>) {
    let cell = source_env_cell();
    let mut guard = cell.lock().unwrap();
    *guard = token;
}

pub fn get_source_environment() -> HashMap<String, String> {
    let cell = source_env_cell();
    let guard = cell.lock().unwrap();
    if let Some(m) = guard.as_ref() {
        return m.clone();
    }
    env::vars().collect()
}

fn is_windows() -> bool {
    cfg!(windows)
}

// ---------------------------------------------------------------------------
// _ENV_LINE — mirrors line 71: re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$")
// ---------------------------------------------------------------------------

/// Parse a line as `KEY=VALUE` per `_ENV_LINE`. Returns `Some((key, value))`
/// where `key` matches `^[A-Za-z_][A-Za-z0-9_]*$` and `value` is everything
/// after the first `=` (may be empty, may contain `=`).
/// The regex is anchored and `.` does not cross newlines — caller splits first.
fn parse_env_line(line: &str) -> Option<(&str, &str)> {
    let eq = line.find('=')?;
    let (k, v_with_eq) = line.split_at(eq);
    // k must match ^[A-Za-z_][A-Za-z0-9_]*$
    if k.is_empty() {
        return None;
    }
    let mut chars = k.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return None,
    }
    if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    // v is everything after the first `=`; strip the leading `=`
    let v = &v_with_eq[1..];
    Some((k, v))
}

// ---------------------------------------------------------------------------
// unquote_dotenv_value — mirrors lines 78-92
// ---------------------------------------------------------------------------

/// Strip a single layer of matching surrounding quotes from a dotenv value.
///
/// Requires length >= 2 so a lone quote (`"`) is left intact rather than
/// collapsing to empty, and `""`/`''` correctly yield an empty string.
/// Shared by the single-key parser and the list path so both unquote
/// identically. Mirrors `unquote_dotenv_value` (lines 78-92).
pub fn unquote_dotenv_value(raw: &str) -> String {
    let t = raw.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        return t[1..t.len() - 1].to_string();
    }
    t.to_string()
}

// ---------------------------------------------------------------------------
// parse_secret_output — mirrors lines 95-159
// ---------------------------------------------------------------------------

/// Parse a secret-fetch helper's stdout. Supports BOTH shapes:
///
/// * a bare value (single secret): the whole trimmed stdout is the value.
/// * a dotenv blob (KEY=VALUE lines): parse them and return the entry for
///   `wanted_key`.
///
/// Mirrors the TS `parseSecretOutput` exactly, including the cross-key
/// misroute guard and the base64-padding disambiguation. Mirrors
/// `parse_secret_output` (lines 95-159).
pub fn parse_secret_output(stdout: &str, wanted_key: &str) -> Option<String> {
    let text = stdout.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();

    // 1. Exact dotenv match wins: scan for a `wanted_key=...` line.
    let mut dotenv_lines: Vec<String> = Vec::new();
    for raw in &lines {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((_k, _v)) = parse_env_line(line) {
            dotenv_lines.push(line.to_string());
        }
    }

    for line in &dotenv_lines {
        if let Some((k, v_raw)) = parse_env_line(line) {
            if k == wanted_key {
                let value = unquote_dotenv_value(v_raw);
                // Whitespace-only (e.g. a quoted `K="  "` placeholder) is "no value"
                if value.trim() != "" {
                    return Some(value);
                } else {
                    return None;
                }
            }
        }
    }

    // 2. The output is a multi-key dotenv dump that does NOT contain the
    //    wanted key → None, rather than mis-returning an unrelated line.
    //    Only >=2 env-shaped lines count as a dump.
    if dotenv_lines.len() > 1 {
        return None;
    }

    // 3. Otherwise treat the whole output as a single bare value.
    let value = text.trim().to_string();
    if value.is_empty() {
        return None;
    }

    // SECURITY (S2): a single env-shaped line for a DIFFERENT key must not
    // be returned as the wanted secret.
    // Disambiguation: base64 padding only produces env-shaped lines whose
    // "value" part is empty or all `=`; non-trivial value after non-matching
    // key means misrouted dotenv entry → None.
    if let Some((k, v_raw)) = parse_env_line(&value) {
        if k != wanted_key {
            let v_trimmed = v_raw.trim();
            // re.fullmatch(r"=*", v_trimmed) is None means non-trivial value
            let all_equals = !v_trimmed.is_empty() && v_trimmed.chars().all(|c| c == '=');
            let is_empty_or_all_equals = v_trimmed.is_empty() || all_equals;
            if !is_empty_or_all_equals {
                return None;
            }
        }
    }

    Some(value)
}

// ---------------------------------------------------------------------------
// _run_helper — mirrors lines 162-263
// ---------------------------------------------------------------------------

/// Run the helper via `/bin/sh -c` and return its stdout, or None.
///
/// The key is passed as DATA via `HERMES_SECRET_KEY` — never interpolated
/// into the command string. Both stdout and stderr are captured via pipes
/// (never inherited); stderr is discarded. Any failure logs structured
/// fields only and returns None — never raises.
/// Mirrors `_run_helper` (lines 162-263).
pub fn run_helper(
    command: &str,
    secret_key: &str,
    timeout_seconds: f64,
    max_output_bytes: usize,
) -> Option<String> {
    _run_helper(command, secret_key, timeout_seconds, max_output_bytes)
}

fn _run_helper(
    command: &str,
    secret_key: &str,
    timeout_seconds: f64,
    max_output_bytes: usize,
) -> Option<String> {
    if is_windows() {
        eprintln!(
            "[secrets:command] the 'command' provider is POSIX-only (needs /bin/sh); resolving no value on Windows"
        );
        return None;
    }

    // User-configured secret-helper command: runs with the user's full shell
    // env by design (it may need any credential to resolve the secret).
    // Mirrors lines 185-196: source_env branching.
    // `get_source_environment()` returns `os.environ` when no scoped env is
    // installed (legacy single-profile path → full env); otherwise the
    // multiplex profile's isolated map.
    let source_env = get_source_environment();
    let cell = source_env_cell();
    let is_global = cell.lock().unwrap().is_none();
    let mut env_map: HashMap<String, String> = if is_global {
        // Legacy single-profile startup intentionally preserves the existing
        // helper contract, which may rely on the user's full environment.
        // Python calls `build_subprocess_env(scrub_secrets=False, inherit_profile_home=False)`.
        // In Rust we emulate that as `env::vars()` (full env, secrets not scrubbed).
        source_env
    } else {
        // A multiplex profile must never inherit sibling secrets —
        // `dict(source_env)` isolated copy.
        source_env
    };
    env_map.insert("HERMES_SECRET_KEY".to_string(), secret_key.to_string());

    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", command]);
    cmd.env_clear();
    cmd.envs(&env_map);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // start_new_session=True so the hard timeout can kill the whole group
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                // Create new session (setsid) — mirrors start_new_session=True
                // SAFETY: setsid is async-signal-safe
                extern "C" { fn setsid() -> i32; }
                setsid();
                Ok(())
            });
        }
    }

    let mut proc = match cmd.spawn() {
        Ok(p) => p,
        Err(exc) => {
            // Mirror OSError errno logging — structured fields only, never command/stderr/secret
            let errno = exc.raw_os_error().map(|n| n.to_string()).unwrap_or_else(|| "?".to_string());
            eprintln!(
                "[secrets:command] helper failed to spawn; resolving no value: errno={}",
                errno
            );
            return None;
        }
    };

    let pid = proc.id();

    // Communicate with timeout — mirrors proc.communicate(timeout=timeout_seconds)
    let timeout_dur = Duration::from_secs_f64(timeout_seconds.max(0.1));
    let deadline = Instant::now() + timeout_dur;

    // Polling wait — mirrors subprocess.TimeoutExpired handling
    loop {
        match proc.try_wait() {
            Ok(Some(status)) => {
                // Process exited — collect output.
                // We need to read stdout/stderr. Since we used try_wait and the child
                // already exited, we can call wait_with_output via a fresh handle?
                // But `proc` already has status; we need to read the pipes.
                // Workaround: use `wait_with_output` on the already-exited child.
                // `try_wait` consumed the exit but pipes still readable via `wait_with_output`.
                // In Rust, after try_wait returns Some, calling wait_with_output will still
                // collect pipes. However some Rust versions require we call `wait_with_output`
                // directly. We handle both by attempting to read.
                let output = match proc.wait_with_output() {
                    Ok(o) => o,
                    Err(_) => {
                        // Fallback: reconstruct with status only
                        // This branch is rare; return None as failure
                        let code_str = status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".to_string());
                        // Signal handling not needed — we already have status
                        let _ = code_str;
                        return None;
                    }
                };
                // _stderr_discarded is captured and DISCARDED — never logged
                let stdout_bytes = output.stdout;
                // let _stderr = output.stderr; // discarded

                if !output.status.success() {
                    // Structured fields ONLY — never command string or helper stderr
                    if let Some(code) = output.status.code() {
                        eprintln!(
                            "[secrets:command] helper failed; resolving no value: code={} signal=none",
                            code
                        );
                    } else {
                        // Terminated by signal — try to map signal number to name
                        #[cfg(unix)]
                        {
                            use std::os::unix::process::ExitStatusExt;
                            if let Some(sig) = output.status.signal() {
                                let sig_name = signal_name(sig);
                                eprintln!(
                                    "[secrets:command] helper failed; resolving no value: code=? signal={}",
                                    sig_name
                                );
                            } else {
                                eprintln!(
                                    "[secrets:command] helper failed; resolving no value: code=? signal=?"
                                );
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            eprintln!(
                                "[secrets:command] helper failed; resolving no value: code=? signal=?"
                            );
                        }
                    }
                    return None;
                }

                if stdout_bytes.len() > max_output_bytes {
                    eprintln!(
                        "[secrets:command] helper output exceeded the {}-byte cap; resolving no value",
                        max_output_bytes
                    );
                    return None;
                }

                // Decode utf-8 with replacement — mirrors errors="replace"
                return Some(String::from_utf8_lossy(&stdout_bytes).to_string());
            }
            Ok(None) => {
                if Instant::now() > deadline {
                    // Hard timeout: kill the whole process group (POSIX-only)
                    // Mirrors os.killpg(os.getpgid(proc.pid), SIGKILL)
                    #[cfg(unix)]
                    {
                        // Try to kill process group first via `kill -9 -<pgid>`
                        // Negative PID means process group in kill(2).
                        let killed_group = Command::new("kill")
                            .args(["-9", &format!("-{}", pid)])
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status()
                            .map(|s| s.success())
                            .unwrap_or(false);
                        if !killed_group {
                            let _ = proc.kill();
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = proc.kill();
                    }
                    // Reap with 1s grace — mirrors proc.communicate(timeout=1.0) after kill
                    let reap_deadline = Instant::now() + Duration::from_secs(1);
                    while Instant::now() < reap_deadline {
                        match proc.try_wait() {
                            Ok(Some(_)) => break,
                            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                            Err(_) => break,
                        }
                    }
                    let _ = proc.wait();
                    eprintln!(
                        "[secrets:command] helper timed out after {}s; resolving no value",
                        format_timeout(timeout_seconds)
                    );
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(exc) => {
                let errno = exc.raw_os_error().map(|n| n.to_string()).unwrap_or_else(|| "?".to_string());
                eprintln!(
                    "[secrets:command] helper failed to spawn; resolving no value: errno={}",
                    errno
                );
                return None;
            }
        }
    }
}

#[cfg(unix)]
fn signal_name(sig: i32) -> String {
    // Mirrors `signal.Signals(-code).name` — map common POSIX signals
    match sig {
        1 => "SIGHUP".to_string(),
        2 => "SIGINT".to_string(),
        3 => "SIGQUIT".to_string(),
        4 => "SIGILL".to_string(),
        6 => "SIGABRT".to_string(),
        8 => "SIGFPE".to_string(),
        9 => "SIGKILL".to_string(),
        11 => "SIGSEGV".to_string(),
        13 => "SIGPIPE".to_string(),
        14 => "SIGALRM".to_string(),
        15 => "SIGTERM".to_string(),
        _ => sig.to_string(),
    }
}

fn format_timeout(v: f64) -> String {
    // Mirrors Python `f"{timeout_seconds:g}"`
    let s = format!("{}", v);
    // g format in Python strips trailing zeros; Rust Display already does similar for f64 via default?
    // Use a manual g-like formatting: if it contains many decimals, trim.
    if s.contains('.') {
        let trimmed = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if trimmed.is_empty() { "0".to_string() } else { trimmed }
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// _parse_dotenv_map — mirrors lines 266-282
// ---------------------------------------------------------------------------

/// Parse a KEY=VALUE blob into a map (the list/enumerate path).
///
/// Mirrors the TS `list()`: only env-shaped lines contribute; comments
/// and non-matching lines are skipped. A bare-value helper yields `{}`
/// — per-key resolution via `get_command_secret` still works.
/// Mirrors `_parse_dotenv_map` (lines 266-282).
pub fn parse_dotenv_map(stdout: &str) -> HashMap<String, String> {
    _parse_dotenv_map(stdout)
}

fn _parse_dotenv_map(stdout: &str) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for raw in stdout.replace("\r\n", "\n").split('\n') {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v_raw)) = parse_env_line(line) {
            out.insert(k.to_string(), unquote_dotenv_value(v_raw));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// get_command_secret / list_command_secrets — mirrors lines 285-320
// ---------------------------------------------------------------------------

/// Resolve a single secret by running the helper with the key in
/// `HERMES_SECRET_KEY`. Returns None on any failure — never raises.
/// Mirrors `get_command_secret` (lines 285-300).
pub fn get_command_secret(
    command: &str,
    key: &str,
    timeout_seconds: f64,
    max_output_bytes: usize,
) -> Option<String> {
    let cmd = command.trim();
    if cmd.is_empty() {
        return None;
    }
    let stdout = _run_helper(cmd, key, timeout_seconds, max_output_bytes)?;
    parse_secret_output(&stdout, key)
}

/// Convenience wrapper with defaults — mirrors the Python default args.
pub fn get_command_secret_default(command: &str, key: &str) -> Option<String> {
    get_command_secret(command, key, COMMAND_TIMEOUT_SECONDS, MAX_OUTPUT_BYTES)
}

/// Enumerate secrets by running the helper ONCE with an empty key.
///
/// Returns the dotenv map ONLY when the helper emits a KEY=VALUE blob;
/// a bare-value helper returns `{}`. Never raises.
/// Mirrors `list_command_secrets` (lines 303-320).
pub fn list_command_secrets(
    command: &str,
    timeout_seconds: f64,
    max_output_bytes: usize,
) -> HashMap<String, String> {
    let cmd = command.trim();
    if cmd.is_empty() {
        return HashMap::new();
    }
    let stdout = match _run_helper(cmd, "", timeout_seconds, max_output_bytes) {
        Some(s) => s,
        None => return HashMap::new(),
    };
    _parse_dotenv_map(&stdout)
}

/// Convenience wrapper with defaults.
pub fn list_command_secrets_default(command: &str) -> HashMap<String, String> {
    list_command_secrets(command, COMMAND_TIMEOUT_SECONDS, MAX_OUTPUT_BYTES)
}

// ---------------------------------------------------------------------------
// apply_command_secrets — mirrors lines 328-393
// ---------------------------------------------------------------------------

/// Run the helper once at startup and set its KEY=VALUE output on
/// `os.environ` (here: `env::set_var`).
///
/// LEGACY shim retained for API symmetry with `apply_bitwarden_secrets`;
/// the startup path goes through `CommandSource` + the registry
/// orchestrator instead (which owns precedence and the environ writes).
/// Mirrors `apply_command_secrets` (lines 328-393).
pub fn apply_command_secrets(
    command: &str,
    override_existing: bool,
    timeout_seconds: f64,
    max_output_bytes: usize,
    _home_path: Option<&Path>,
) -> FetchResult {
    let mut result = FetchResult::default();

    let cmd = command.trim();
    if cmd.is_empty() {
        result.error = Some(
            "secrets.command.enabled is true but secrets.command.command is empty.  Set the helper command in config.yaml.".to_string()
        );
        return result;
    }

    if is_windows() {
        result.warnings.push(
            "the 'command' secret source is POSIX-only (needs /bin/sh); skipping on Windows".to_string()
        );
        return result;
    }

    // The list/enumerate path: run the helper exactly ONCE with an empty
    // HERMES_SECRET_KEY and parse its stdout as a dotenv blob.
    let stdout = match _run_helper(cmd, "", timeout_seconds, max_output_bytes) {
        Some(s) => s,
        None => {
            // _run_helper already logged structured fields to stderr.
            result.warnings.push(
                "helper command failed at startup; no secrets applied (process env / .env values remain in effect)".to_string()
            );
            return result;
        }
    };

    let secrets = _parse_dotenv_map(&stdout);
    result.secrets = secrets.clone();
    if secrets.is_empty() {
        result.warnings.push(
            "helper output was not a KEY=VALUE map; nothing applied at startup (a bare-value helper still resolves single keys on demand)".to_string()
        );
        return result;
    }

    for (key, value) in secrets {
        if value.trim().is_empty() {
            // Whitespace-only placeholder entries are "no value"
            result.skipped.push(key);
            continue;
        }
        if !override_existing {
            if let Ok(existing) = env::var(&key) {
                if !existing.is_empty() {
                    result.skipped.push(key);
                    continue;
                }
            }
        }
        env::set_var(&key, &value);
        result.applied.push(key);
    }

    result
}

/// Convenience wrapper with defaults.
pub fn apply_command_secrets_default(command: &str, override_existing: bool) -> FetchResult {
    apply_command_secrets(command, override_existing, COMMAND_TIMEOUT_SECONDS, MAX_OUTPUT_BYTES, None)
}

// ---------------------------------------------------------------------------
// SecretSource adapter — mirrors lines 401-501
// ---------------------------------------------------------------------------

/// Minimal `Value` enum for config payloads — mirrors `dict` config coercion.
/// Std-only `serde_json::Value` stand-in, matching `secret_registry.rs::Value`.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Int(i64),
    String(String),
    Array(Vec<Value>),
    Map(HashMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self { Value::String(s) => Some(s.as_str()), _ => None }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self { Value::Bool(b) => Some(*b), _ => None }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self { Value::Number(n) => Some(*n), Value::Int(i) => Some(*i as f64), _ => None }
    }
    pub fn as_map(&self) -> Option<&HashMap<String, Value>> {
        match self { Value::Map(m) => Some(m), _ => None }
    }
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self { Value::Array(a) => Some(a), _ => None }
    }
    pub fn is_null(&self) -> bool { matches!(self, Value::Null) }
}

/// SecretSource trait — mirrors `base.SecretSource` ABC (base.py lines 127-249).
/// Local definition keeps the slice self-contained; merge step replaces with canonical trait.
pub trait SecretSource: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str { self.name() }
    fn shape(&self) -> &str { "mapped" }
    fn scheme(&self) -> Option<&str> { None }
    fn api_version(&self) -> i32 { 1 }
    fn is_enabled(&self, cfg: &HashMap<String, Value>) -> bool {
        cfg.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)
    }
    fn override_existing(&self, cfg: &HashMap<String, Value>) -> bool {
        cfg.get("override_existing").and_then(|v| v.as_bool()).unwrap_or(false)
    }
    fn protected_env_vars(&self, _cfg: &HashMap<String, Value>) -> Vec<String> { Vec::new() }
    fn fetch_timeout_seconds(&self, cfg: &HashMap<String, Value>) -> f64 {
        let raw = cfg.get("timeout_seconds").or_else(|| cfg.get("helper_timeout_seconds"));
        if let Some(v) = raw {
            if let Some(n) = v.as_f64() {
                if n > 0.0 { return n; }
                else { return COMMAND_TIMEOUT_SECONDS; }
            }
            if let Some(s) = v.as_str() {
                if let Ok(n) = s.parse::<f64>() {
                    if n > 0.0 { return n; }
                    else { return COMMAND_TIMEOUT_SECONDS; }
                }
            }
            return COMMAND_TIMEOUT_SECONDS;
        }
        COMMAND_TIMEOUT_SECONDS
    }
    fn config_schema(&self) -> HashMap<String, Value> { HashMap::new() }
    fn remediation(&self, kind: Option<&ErrorKind>, _cfg: &HashMap<String, Value>) -> String {
        match kind {
            Some(ErrorKind::NotConfigured) => format!("Run `hermes secrets {} setup` to finish configuration.", self.name()),
            Some(ErrorKind::BinaryMissing) => format!("Run `hermes secrets {} setup` to install the helper CLI.", self.name()),
            Some(ErrorKind::AuthFailed) => format!("Credentials rejected — run `hermes secrets {} setup` to re-authenticate.", self.name()),
            Some(ErrorKind::AuthExpired) => format!("Credentials expired — run `hermes secrets {} setup` to re-authenticate.", self.name()),
            Some(ErrorKind::Network) => "Network problem reaching the secrets backend — check connectivity and retry.".to_string(),
            Some(ErrorKind::Timeout) => format!("Backend was slow — raise secrets.{}.timeout_seconds if this recurs.", self.name()),
            _ => String::new(),
        }
    }
    fn fetch(&self, cfg: &HashMap<String, Value>, home_path: &Path) -> FetchResult;
}

/// User-configured helper command as a registered secret source.
///
/// Composes with the other sources (Bitwarden, 1Password, plugins) through
/// the `apply_all()` orchestrator — enable any combination simultaneously;
/// there is deliberately NO single-provider selector. `fetch()` only
/// fetches: precedence, `override_existing` semantics, conflict warnings,
/// and the `os.environ` writes are the orchestrator's job.
///
/// Bulk shape: the helper enumerates a KEY=VALUE blob in one run. Config:
///
/// ```yaml
/// secrets:
///   command:
///     enabled: true
///     command: "cat /run/user/1000/hermes-secrets.env"
/// ```
/// Mirrors `CommandSource` (lines 401-501).
pub struct CommandSource;

impl CommandSource {
    pub fn new() -> Self { Self }
}

impl Default for CommandSource {
    fn default() -> Self { Self::new() }
}

impl SecretSource for CommandSource {
    fn name(&self) -> &str { "command" }
    fn label(&self) -> &str { "Command helper" }
    fn shape(&self) -> &str { "bulk" }

    fn config_schema(&self) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert("enabled".to_string(), Value::Map({
            let mut inner = HashMap::new();
            inner.insert("description".to_string(), Value::String("Master switch".to_string()));
            inner.insert("default".to_string(), Value::Bool(false));
            inner
        }));
        m.insert("command".to_string(), Value::Map({
            let mut inner = HashMap::new();
            inner.insert("description".to_string(), Value::String("Helper run via /bin/sh -c; must print a KEY=VALUE blob on stdout".to_string()));
            inner.insert("default".to_string(), Value::String(String::new()));
            inner
        }));
        m.insert("helper_timeout_seconds".to_string(), Value::Map({
            let mut inner = HashMap::new();
            inner.insert("description".to_string(), Value::String("Hard timeout for one helper run".to_string()));
            inner.insert("default".to_string(), Value::Number(COMMAND_TIMEOUT_SECONDS));
            inner
        }));
        m.insert("override_existing".to_string(), Value::Map({
            let mut inner = HashMap::new();
            inner.insert("description".to_string(), Value::String("Helper values overwrite .env/shell values".to_string()));
            inner.insert("default".to_string(), Value::Bool(false));
            inner
        }));
        m
    }

    fn fetch(&self, cfg: &HashMap<String, Value>, _home_path: &Path) -> FetchResult {
        let mut result = FetchResult::default();

        let command = cfg.get("command").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if command.is_empty() {
            result.error = Some(
                "secrets.command.enabled is true but secrets.command.command is empty.  Set the helper command in config.yaml.".to_string()
            );
            result.error_kind = Some(ErrorKind::NotConfigured);
            return result;
        }

        if is_windows() {
            result.error = Some(
                "the 'command' secret source is POSIX-only (needs /bin/sh); skipping on Windows".to_string()
            );
            result.error_kind = Some(ErrorKind::NotConfigured);
            return result;
        }

        let timeout: f64 = match cfg.get("helper_timeout_seconds") {
            Some(v) => {
                if let Some(n) = v.as_f64() { n }
                else if let Some(s) = v.as_str() { s.parse::<f64>().unwrap_or(COMMAND_TIMEOUT_SECONDS) }
                else { COMMAND_TIMEOUT_SECONDS }
            }
            None => COMMAND_TIMEOUT_SECONDS,
        };
        // Guard non-positive timeout → default (mirrors base.SecretSource timeout logic)
        let _ = timeout; // keep as-is; Python fetch() does `float(cfg.get(...))` try/except and falls back to default on TypeError/ValueError only, not on <=0.
        // But we mimic the Python try/except exactly: only TypeError/ValueError fall back, negative values pass through to _run_helper which will clamp via Duration::from_secs_f64(max(0.1))
        // So we preserve the parsed value even if <=0; _run_helper will clamp.

        let timeout_effective = match cfg.get("helper_timeout_seconds") {
            Some(v) => {
                // Try float conversion like Python: float(value) or fallback
                if let Some(n) = v.as_f64() { n }
                else if let Some(s) = v.as_str() {
                    match s.parse::<f64>() {
                        Ok(n) => n,
                        Err(_) => COMMAND_TIMEOUT_SECONDS,
                    }
                } else {
                    COMMAND_TIMEOUT_SECONDS
                }
            }
            None => COMMAND_TIMEOUT_SECONDS,
        };

        let stdout = _run_helper(&command, "", timeout_effective, MAX_OUTPUT_BYTES);
        let stdout = match stdout {
            Some(s) => s,
            None => {
                // _run_helper already logged structured fields to stderr.
                result.error = Some(
                    "helper command failed (see structured fields above); no secrets applied".to_string()
                );
                result.error_kind = Some(ErrorKind::Internal);
                return result;
            }
        };

        let secrets = _parse_dotenv_map(&stdout);
        if secrets.is_empty() {
            result.warnings.push(
                "helper output was not a KEY=VALUE map; nothing to apply".to_string()
            );
            return result;
        }

        result.secrets = secrets;
        result
    }

    fn remediation(&self, kind: Option<&ErrorKind>, _cfg: &HashMap<String, Value>) -> String {
        match kind {
            Some(ErrorKind::NotConfigured) => {
                "Set secrets.command.command in config.yaml to a fast, non-interactive helper that prints KEY=VALUE lines.".to_string()
            }
            Some(ErrorKind::Internal) => {
                "Run the helper manually in a shell to see its real error — Hermes discards helper stderr so diagnostics can't leak secret material.".to_string()
            }
            _ => {
                // Delegate to default trait impl shape: call super
                match kind {
                    Some(ErrorKind::BinaryMissing) => format!("Run `hermes secrets {} setup` to install the helper CLI.", self.name()),
                    Some(ErrorKind::AuthFailed) => format!("Credentials rejected — run `hermes secrets {} setup` to re-authenticate.", self.name()),
                    Some(ErrorKind::AuthExpired) => format!("Credentials expired — run `hermes secrets {} setup` to re-authenticate.", self.name()),
                    Some(ErrorKind::Network) => "Network problem reaching the secrets backend — check connectivity and retry.".to_string(),
                    Some(ErrorKind::Timeout) => format!("Backend was slow — raise secrets.{}.timeout_seconds if this recurs.", self.name()),
                    _ => String::new(),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_roundtrip() {
        assert_eq!(unquote_dotenv_value(r#""hello""#), "hello");
        assert_eq!(unquote_dotenv_value("'hello'"), "hello");
        assert_eq!(unquote_dotenv_value(r#""  ""#), "  ");
        assert_eq!(unquote_dotenv_value("noquotes"), "noquotes");
        assert_eq!(unquote_dotenv_value("\""), "\"");
        assert_eq!(unquote_dotenv_value("\"\""), "");
        assert_eq!(unquote_dotenv_value("''"), "");
        assert_eq!(unquote_dotenv_value("  \"x\"  "), "x");
    }

    #[test]
    fn parse_env_line_basic() {
        assert_eq!(parse_env_line("FOO=bar"), Some(("FOO", "bar")));
        assert_eq!(parse_env_line("FOO="), Some(("FOO", "")));
        assert_eq!(parse_env_line("FOO=bar=baz"), Some(("FOO", "bar=baz")));
        assert_eq!(parse_env_line("_A1=1"), Some(("_A1", "1")));
        assert!(parse_env_line("1bad=val").is_none());
        assert!(parse_env_line("has-dash=val").is_none());
        assert!(parse_env_line("noequalssign").is_none());
        assert_eq!(parse_env_line("#COMMENT=ignored"), None); // caller skips #; line itself would be invalid key
    }

    #[test]
    fn parse_secret_output_exact_match() {
        let out = "FOO=bar\nOTHER=ignored\n# comment\n";
        assert_eq!(parse_secret_output(out, "FOO"), Some("bar".to_string()));
        assert_eq!(parse_secret_output("FOO=\"  hello  \"\n", "FOO"), Some("  hello  ".to_string()));
        // whitespace-only after unquote → None
        assert_eq!(parse_secret_output("FOO=\"  \"\n", "FOO"), None);
    }

    #[test]
    fn parse_secret_output_multi_key_no_match() {
        // >=2 env-shaped lines without wanted key → None (not bare value)
        let out = "A=1\nB=2\n";
        assert_eq!(parse_secret_output(out, "WANTED"), None);
    }

    #[test]
    fn parse_secret_output_single_nonmatching_env_shaped_is_bare_value_misroute_guard() {
        // Single non-matching env-shaped line with non-trivial value → None (S2 guard)
        assert_eq!(parse_secret_output("OTHER_KEY=realvalue\n", "WANTED"), None);
        // Base64 padding disambiguation: value empty or all '=' → treat as bare value
        assert_eq!(parse_secret_output("dGVzdA==\n", "WANTED"), Some("dGVzdA==".to_string()));
        assert_eq!(parse_secret_output("dGVzdA=\n", "WANTED"), Some("dGVzdA=".to_string()));
        // Single line non-env-shaped bare value → returned
        assert_eq!(parse_secret_output("just-a-secret\n", "WANTED"), Some("just-a-secret".to_string()));
    }

    #[test]
    fn parse_secret_output_bare_value_trim() {
        assert_eq!(parse_secret_output("  secret  \n", "K"), Some("secret".to_string()));
        assert_eq!(parse_secret_output("   \n\t\n", "K"), None);
        assert_eq!(parse_secret_output("", "K"), None);
    }

    #[test]
    fn parse_dotenv_map_basic() {
        let m = _parse_dotenv_map("A=1\nB=\"2\"\n# comment\n\nC='3'\nnotenv\n");
        assert_eq!(m.get("A").map(|s| s.as_str()), Some("1"));
        assert_eq!(m.get("B").map(|s| s.as_str()), Some("2"));
        assert_eq!(m.get("C").map(|s| s.as_str()), Some("3"));
        assert!(!m.contains_key("notenv"));
    }

    #[test]
    fn command_source_empty_command_error() {
        let src = CommandSource::new();
        let cfg = HashMap::new();
        let res = src.fetch(&cfg, Path::new("/tmp"));
        assert!(res.error.is_some());
        assert_eq!(res.error_kind, Some(ErrorKind::NotConfigured));
    }

    #[test]
    fn get_command_secret_empty_command_returns_none() {
        assert_eq!(get_command_secret("", "KEY", 3.0, 1024 * 1024), None);
        assert_eq!(get_command_secret("   ", "KEY", 3.0, 1024 * 1024), None);
    }

    #[test]
    fn list_command_secrets_empty_command_returns_empty() {
        let m = list_command_secrets("", 3.0, 1024 * 1024);
        assert!(m.is_empty());
    }
}
