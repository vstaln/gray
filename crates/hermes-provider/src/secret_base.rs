//! Secret-source contract: the ABC every secret backend implements.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/secret_sources/base.py` (336 lines).
//!
//! A *secret source* resolves credentials from an external secret manager
//! (Bitwarden Secrets Manager, 1Password, an OS keystore, a user script, ...)
//! into environment-variable-shaped values at process startup, AFTER
//! `~/.hermes/.env` has loaded and BEFORE the rest of Hermes reads
//! `os.environ`.
//!
//! Scope of the contract (deliberate, please do not widen):
//! * **Read-only.**  Sources resolve refs → values.  No write-back.
//! * **Startup-time, synchronous.**  `fetch()` is called once per process
//!   (per HERMES_HOME) by the orchestrator in `secret_registry`, which
//!   enforces a wall-clock timeout around it.
//! * **Never raises, never prompts.**  `fetch()` returns a `FetchResult`.
//! * **Sources fetch; the orchestrator applies.**  A source returns the
//!   name→value mapping it *would* contribute.  Precedence is owned by
//!   the orchestrator.
//!
//! T0042 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `SECRET_SOURCE_API_VERSION = 1` ↔ `pub const SECRET_SOURCE_API_VERSION: i32 = 1`.
//! - Python `ContextVar[Optional[MutableMapping[str,str]]]` ↔ `OnceLock<Mutex<Option<HashMap<String,String>>>>`.
//! - Python `ErrorKind(str, Enum)` ↔ `enum ErrorKind` with `as_str()`.
//! - Python `FetchResult` dataclass ↔ `struct FetchResult` with `Default`.
//! - Python `SecretSource(ABC)` ↔ `trait SecretSource: Send + Sync`.
//! - Python `re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")` ↔ manual char scan (no `regex` dep).
//! - Python `re.compile(r"\x1b(?:\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)?)")` ↔ manual scan.
//! - Python `subprocess.run(..., env=minimal, timeout=..., stdin=DEVNULL)` ↔ `std::process::Command` + polling timeout.
//! - `crate stays std-only` — no `regex`, `log`, or `serde` deps.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Constants — mirrors base.py lines 52, 75, 78
// ---------------------------------------------------------------------------

/// Bump ONLY for breaking changes to the required contract surface
/// (abstract-method signatures, FetchResult required fields).
/// Mirrors `SECRET_SOURCE_API_VERSION = 1` (base.py line 52).
pub const SECRET_SOURCE_API_VERSION: i32 = 1;

/// Timeout the orchestrator enforces around fetch() when the source's
/// config section doesn't override it.  Mirrors `DEFAULT_FETCH_TIMEOUT_SECONDS = 120.0` (line 75).
pub const DEFAULT_FETCH_TIMEOUT_SECONDS: f64 = 120.0;

/// Default timeout for run_secret_cli() subprocess invocations.
/// Mirrors `DEFAULT_CLI_TIMEOUT_SECONDS = 30.0` (line 78).
pub const DEFAULT_CLI_TIMEOUT_SECONDS: f64 = 30.0;

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` / `dict` config payloads for 1:1 coercion
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// ContextVar shim — mirrors base.py lines 54-70
// ---------------------------------------------------------------------------

static SOURCE_ENVIRONMENT: OnceLock<Mutex<Option<HashMap<String, String>>>> = OnceLock::new();

fn source_env_cell() -> &'static Mutex<Option<HashMap<String, String>>> {
    SOURCE_ENVIRONMENT.get_or_init(|| Mutex::new(None))
}

/// Install a per-fetch environment view without changing `os.environ`.
/// Mirrors `set_source_environment` (base.py lines 58-60).
/// Returns the previous value as a token for `reset_source_environment`.
pub fn set_source_environment(environ: HashMap<String, String>) -> Option<HashMap<String, String>> {
    let cell = source_env_cell();
    let mut guard = cell.lock().unwrap();
    let prev = guard.clone();
    *guard = Some(environ);
    prev
}

/// Mirrors `reset_source_environment` (base.py lines 63-64).
pub fn reset_source_environment(token: Option<HashMap<String, String>>) {
    let cell = source_env_cell();
    let mut guard = cell.lock().unwrap();
    *guard = token;
}

/// Return the active per-fetch environment, or the process environment.
/// Mirrors `get_source_environment` (base.py lines 67-70).
pub fn get_source_environment() -> HashMap<String, String> {
    let cell = source_env_cell();
    let guard = cell.lock().unwrap();
    if let Some(m) = guard.as_ref() {
        return m.clone();
    }
    env::vars().collect()
}

// ---------------------------------------------------------------------------
// ErrorKind — mirrors base.py lines 81-98
// ---------------------------------------------------------------------------

/// Machine-readable failure taxonomy for `FetchResult.error`.
///
/// A fixed vocabulary keeps startup warnings and `hermes secrets status`
/// uniform across backends, and lets the orchestrator implement
/// kind-dependent policy exactly once.
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

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "not_configured" => Some(ErrorKind::NotConfigured),
            "binary_missing" => Some(ErrorKind::BinaryMissing),
            "auth_failed" => Some(ErrorKind::AuthFailed),
            "auth_expired" => Some(ErrorKind::AuthExpired),
            "ref_invalid" => Some(ErrorKind::RefInvalid),
            "network" => Some(ErrorKind::Network),
            "empty_value" => Some(ErrorKind::EmptyValue),
            "timeout" => Some(ErrorKind::Timeout),
            "internal" => Some(ErrorKind::Internal),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// FetchResult — mirrors base.py lines 101-124
// ---------------------------------------------------------------------------

/// Outcome of one source's fetch.
///
/// `secrets` holds what the source *would* contribute; whether each
/// var is actually applied is the orchestrator's decision.  `applied`
/// and `skipped` exist for backward compatibility with the original
/// Bitwarden fetch-and-apply entry point and are left empty by
/// conforming `fetch()` implementations.
#[derive(Debug, Clone, Default)]
pub struct FetchResult {
    pub secrets: HashMap<String, String>,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub error_kind: Option<ErrorKind>,
    /// Path of the helper binary used, when the source is CLI-driven.
    /// Surfaced by status commands; None for SDK/API-driven sources.
    pub binary_path: Option<PathBuf>,
}

impl FetchResult {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

// ---------------------------------------------------------------------------
// SecretSource trait — mirrors base.py lines 127-249
// ---------------------------------------------------------------------------

/// One external secret backend.
///
/// Subclasses set the class attributes and implement `fetch`.
/// Everything else has a sensible default.
pub trait SecretSource: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str { self.name() }
    fn shape(&self) -> &str { "mapped" }
    fn scheme(&self) -> Option<&str> { None }
    fn api_version(&self) -> i32 { SECRET_SOURCE_API_VERSION }

    /// Resolve this source's secrets. MUST NOT raise or prompt.
    ///
    /// `cfg` is the source's raw config section (`secrets.<name>`)
    /// from config.yaml — treat every field defensively.
    /// `home_path` is the resolved HERMES_HOME.
    fn fetch(&self, cfg: &HashMap<String, Value>, home_path: &Path) -> FetchResult;

    // -- optional hooks (defaults are correct for most sources) ------------

    /// Whether the user turned this source on.
    /// Mirrors `is_enabled` (base.py lines 173-175).
    fn is_enabled(&self, cfg: &HashMap<String, Value>) -> bool {
        cfg.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// May this source overwrite vars that .env / the shell already set?
    /// Mirrors `override_existing` (base.py lines 177-184).
    fn override_existing(&self, cfg: &HashMap<String, Value>) -> bool {
        cfg.get("override_existing").and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// Env vars the orchestrator must never let ANY source overwrite.
    /// Mirrors `protected_env_vars` (base.py lines 186-193).
    fn protected_env_vars(&self, _cfg: &HashMap<String, Value>) -> Vec<String> {
        Vec::new()
    }

    /// Wall-clock budget the orchestrator enforces around fetch().
    /// Mirrors `fetch_timeout_seconds` (base.py lines 195-201).
    fn fetch_timeout_seconds(&self, cfg: &HashMap<String, Value>) -> f64 {
        let raw = cfg.get("timeout_seconds");
        if let Some(v) = raw {
            if let Some(n) = v.as_f64() {
                if n > 0.0 { return n; }
                return DEFAULT_FETCH_TIMEOUT_SECONDS;
            }
            if let Some(s) = v.as_str() {
                if let Ok(n) = s.parse::<f64>() {
                    if n > 0.0 { return n; }
                    return DEFAULT_FETCH_TIMEOUT_SECONDS;
                }
            }
            return DEFAULT_FETCH_TIMEOUT_SECONDS;
        }
        DEFAULT_FETCH_TIMEOUT_SECONDS
    }

    /// Optional description of this source's config keys.
    /// Mirrors `config_schema` (base.py lines 203-210).
    fn config_schema(&self) -> HashMap<String, Value> {
        HashMap::new()
    }

    /// One-line, actionable next step for a failed fetch.
    /// Mirrors `remediation` (base.py lines 212-249).
    ///
    /// Must never raise and must not perform I/O — it's a pure
    /// kind→string mapping on the startup path.
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
}

// ---------------------------------------------------------------------------
// Shared helpers — mirrors base.py lines 256-336
// ---------------------------------------------------------------------------

/// Mirrors `_ENV_NAME_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")` (base.py line 257).
/// Uses manual scan so the crate stays std-only (no `regex` dep).
pub fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {},
        _ => return false,
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    true
}

// ANSI CSI/OSC escape sequences — helper-CLI stderr often carries color
// codes that must not reach Hermes' own startup output.
// NOTE: intentionally NOT migrated to tools.ansi_strip.strip_ansi — the
// optional terminator here (`(?:\x07|\x1b\\)?`) also strips *unterminated*
// OSC sequences (common when a CLI is killed mid-write), which strip_ansi
// leaves untouched. strip_ansi is not a superset of this regex.
// Mirrors `_ANSI_RE` (base.py line 265).
pub fn scrub_ansi(text: &str) -> String {
    // Manual scan for `\x1b(?:\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)?)`
    // Two branches after ESC:
    //  1. CSI: ESC [ [0-9;?]* [ -/]* [@-~]
    //  2. OSC: ESC ] [^\x07\x1b]* (?:\x07 | ESC \) )?   — terminator optional to catch unterminated
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != 0x1b {
            // Copy one utf8 char
            let ch = text[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        // ESC at i
        if i + 1 >= bytes.len() {
            // Lone ESC at end — strip it
            i += 1;
            continue;
        }
        let next = bytes[i + 1];
        if next == b'[' {
            // CSI: consume ESC [
            let mut j = i + 2;
            // [0-9;?]*
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';' || bytes[j] == b'?') {
                j += 1;
            }
            // [ -/]*
            while j < bytes.len() && bytes[j] >= 0x20 && bytes[j] <= 0x2f {
                j += 1;
            }
            // [@-~]
            if j < bytes.len() && bytes[j] >= 0x40 && bytes[j] <= 0x7e {
                // Whole CSI matched — strip ESC..terminator inclusive
                i = j + 1;
                continue;
            } else {
                // Incomplete CSI — strip ESC [ and any intermediate we consumed
                // (matches Python regex which requires final byte; but we still want to strip partial)
                // For fidelity: if no final byte, Python regex would NOT match; but we choose to strip ESC only
                // to avoid over-stripping. Keep strict: only strip if final byte present.
                out.push('\x1b');
                i += 1;
                continue;
            }
        } else if next == b']' {
            // OSC: ESC ] [^\x07\x1b]* (?:\x07 | ESC \)?
            let mut j = i + 2;
            // [^\x07\x1b]*
            while j < bytes.len() && bytes[j] != 0x07 && bytes[j] != 0x1b {
                // advance by one byte for now — OSC payload is typically ascii
                // but we handle utf8 by not splitting chars: just byte scan
                j += 1;
            }
            // Optional terminator
            if j < bytes.len() && bytes[j] == 0x07 {
                // BEL terminator
                i = j + 1;
                continue;
            } else if j + 1 < bytes.len() && bytes[j] == 0x1b && bytes[j + 1] == b'\\' {
                i = j + 2;
                continue;
            } else if j < bytes.len() && bytes[j] == 0x1b {
                // ESC not followed by \ — unterminated OSC, Python regex with optional terminator
                // strips up to but not including lone ESC? Actually `[^\x07\x1b]*` stops at ESC,
                // then `(?:\x07|\x1b\\)?` is optional, so ESC]... without terminator still matches ESC]payload.
                // So we strip ESC]payload (up to j, exclusive of terminator).
                i = j;
                continue;
            } else {
                // At end or no terminator — still strip ESC]payload (optional terminator covers unterminated)
                // Python: `\][^\x07\x1b]*(?:\x07|\x1b\\)?` matches even without terminator.
                i = j;
                // If we consumed to end, break
                if i >= bytes.len() {
                    break;
                }
                continue;
            }
        } else {
            // ESC not followed by [ or ] — not our ANSI sequence, keep ESC
            out.push('\x1b');
            i += 1;
            continue;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// run_secret_cli — mirrors base.py lines 278-336
// ---------------------------------------------------------------------------

/// Result of `run_secret_cli` — mirrors `subprocess.CompletedProcess` (text mode).
#[derive(Debug, Clone)]
pub struct CompletedProcess {
    pub args: Vec<String>,
    pub returncode: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run a secret-manager helper CLI with a minimal, allowlisted env.
///
/// Security posture shared by every subprocess-driven backend:
/// * argv list only — never `shell=True`.
/// * The child gets `PATH`/`HOME`/locale basics plus only the env
///   vars named in `allow_env` (auth/session vars) and `extra_env`
///   — never a copy of the full post-dotenv `os.environ`.
/// * `NO_COLOR=1` is set and stderr/stdout are ANSI-scrubbed so
///   helper diagnostics can't smuggle escape sequences into Hermes output.
/// * stdin is `/dev/null` so a helper that decides to prompt fails
///   fast instead of hanging startup.
///
/// Raises `RuntimeError` (as `Err(String)`) on spawn failure or timeout
/// (message safe to surface); returns the completed process otherwise —
/// callers own returncode interpretation.
///
/// Mirrors `run_secret_cli` (base.py lines 278-336).
pub fn run_secret_cli(
    argv: &[String],
    allow_env: &[String],
    extra_env: Option<&HashMap<String, String>>,
    timeout: f64,
) -> Result<CompletedProcess, String> {
    run_secret_cli_with_timeout(argv, allow_env, extra_env, timeout)
}

/// Convenience overload accepting `&[&str]`.
pub fn run_secret_cli_strs(
    argv: &[&str],
    allow_env: &[&str],
    extra_env: Option<&HashMap<String, String>>,
    timeout: f64,
) -> Result<CompletedProcess, String> {
    let argv_owned: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let allow_owned: Vec<String> = allow_env.iter().map(|s| s.to_string()).collect();
    run_secret_cli(&argv_owned, &allow_owned, extra_env, timeout)
}

fn run_secret_cli_with_timeout(
    argv: &[String],
    allow_env: &[String],
    extra_env: Option<&HashMap<String, String>>,
    timeout: f64,
) -> Result<CompletedProcess, String> {
    if argv.is_empty() {
        return Err("run_secret_cli: argv must not be empty".to_string());
    }
    let prog_name = Path::new(&argv[0])
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| argv[0].clone());

    // Build minimal env — mirrors base.py lines 305-314
    let base_keep: &[&str] = &[
        "PATH",
        "HOME",
        "USERPROFILE",
        "SYSTEMROOT",
        "TMPDIR",
        "TEMP",
        "LANG",
        "LC_ALL",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
    ];
    let mut env_map: HashMap<String, String> = HashMap::new();
    for key in base_keep.iter().chain(allow_env.iter().map(|s| s.as_str())) {
        if let Ok(val) = env::var(key) {
            env_map.insert(key.to_string(), val);
        }
    }
    if let Some(extra) = extra_env {
        for (k, v) in extra {
            env_map.insert(k.clone(), v.clone());
        }
    }
    env_map.entry("NO_COLOR".to_string()).or_insert_with(|| "1".to_string());

    let timeout_dur = Duration::from_secs_f64(timeout.max(0.1));

    // Use a worker thread + channel so we can enforce a wall-clock timeout
    // around `Command::output()` without needing `wait_timeout` (not in std).
    // This mirrors `subprocess.run(..., timeout=...)` + daemon thread in Python:
    // on timeout we return an error immediately; the worker thread lingers
    // as a daemon until the child exits (process exit reaps it).
    let argv_clone = argv.to_vec();
    let env_map_clone = env_map.clone();
    let (tx, rx) = std::sync::mpsc::channel::<Result<std::process::Output, String>>();

    std::thread::spawn(move || {
        let mut cmd = Command::new(&argv_clone[0]);
        if argv_clone.len() > 1 {
            cmd.args(&argv_clone[1..]);
        }
        cmd.env_clear();
        cmd.envs(&env_map_clone);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let res = match cmd.output() {
            Ok(o) => Ok(o),
            Err(e) => {
                let prog = Path::new(&argv_clone[0])
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| argv_clone[0].clone());
                Err(format!("failed to invoke {}: {}", prog, e))
            }
        };
        let _ = tx.send(res);
    });

    match rx.recv_timeout(timeout_dur) {
        Ok(Ok(output)) => {
            let stdout_raw = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr_raw = String::from_utf8_lossy(&output.stderr).to_string();
            let code = output.status.code().unwrap_or(-1);
            let proc = CompletedProcess {
                args: argv.to_vec(),
                returncode: code,
                stdout: stdout_raw,
                stderr: scrub_ansi(&stderr_raw),
            };
            Ok(proc)
        }
        Ok(Err(e)) => Err(e),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err(format!("{} timed out after {:.0}s", prog_name, timeout))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(format!("failed to invoke {}: channel closed", prog_name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_env_names() {
        assert!(is_valid_env_name("FOO"));
        assert!(is_valid_env_name("FOO_BAR"));
        assert!(is_valid_env_name("_foo"));
        assert!(is_valid_env_name("A1"));
        assert!(!is_valid_env_name("1bad"));
        assert!(!is_valid_env_name("has-dash"));
        assert!(!is_valid_env_name("has.dot"));
        assert!(!is_valid_env_name(""));
        assert!(!is_valid_env_name("has space"));
    }

    #[test]
    fn scrub_ansi_csi() {
        assert_eq!(scrub_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(scrub_ansi("plain"), "plain");
        assert_eq!(scrub_ansi("\x1b[1;32mhi\x1b[0m there"), "hi there");
    }

    #[test]
    fn scrub_ansi_osc_stripped() {
        // OSC terminated by BEL
        assert_eq!(scrub_ansi("a\x1b]0;title\x07b"), "ab");
        // OSC terminated by ESC \
        assert_eq!(scrub_ansi("a\x1b]0;title\x1b\\b"), "ab");
        // Unterminated OSC (common when CLI killed mid-write) — also stripped
        assert_eq!(scrub_ansi("a\x1b]0;title"), "a");
        assert_eq!(scrub_ansi("a\x1b]0;title b"), "a b");
    }

    #[test]
    fn fetch_result_ok() {
        let r = FetchResult::default();
        assert!(r.ok());
        let mut r2 = FetchResult::default();
        r2.error = Some("boom".to_string());
        assert!(!r2.ok());
    }

    #[test]
    fn constants_match_python() {
        assert_eq!(SECRET_SOURCE_API_VERSION, 1);
        assert_eq!(DEFAULT_FETCH_TIMEOUT_SECONDS, 120.0);
        assert_eq!(DEFAULT_CLI_TIMEOUT_SECONDS, 30.0);
    }

    #[test]
    fn error_kind_roundtrip() {
        for k in [
            ErrorKind::NotConfigured,
            ErrorKind::BinaryMissing,
            ErrorKind::AuthFailed,
            ErrorKind::AuthExpired,
            ErrorKind::RefInvalid,
            ErrorKind::Network,
            ErrorKind::EmptyValue,
            ErrorKind::Timeout,
            ErrorKind::Internal,
        ] {
            let s = k.as_str();
            assert_eq!(ErrorKind::from_str(s), Some(k.clone()));
        }
    }

    #[test]
    fn source_environment_roundtrip() {
        let mut m = HashMap::new();
        m.insert("FOO".to_string(), "bar".to_string());
        let tok = set_source_environment(m.clone());
        assert_eq!(get_source_environment().get("FOO").map(|s| s.as_str()), Some("bar"));
        reset_source_environment(tok);
        // After reset, should be process env (no FOO unless set in real env)
        // Just check it doesn't panic
        let _ = get_source_environment();
    }

    #[test]
    fn secret_source_defaults() {
        struct Dummy;
        impl SecretSource for Dummy {
            fn name(&self) -> &str { "dummy" }
            fn fetch(&self, _cfg: &HashMap<String, Value>, _home: &Path) -> FetchResult {
                FetchResult::default()
            }
        }
        let d = Dummy;
        assert_eq!(d.name(), "dummy");
        assert_eq!(d.label(), "dummy");
        assert_eq!(d.shape(), "mapped");
        assert_eq!(d.scheme(), None);
        assert_eq!(d.api_version(), SECRET_SOURCE_API_VERSION);
        let cfg = HashMap::new();
        assert!(!d.is_enabled(&cfg));
        assert!(!d.override_existing(&cfg));
        assert!(d.protected_env_vars(&cfg).is_empty());
        assert_eq!(d.fetch_timeout_seconds(&cfg), DEFAULT_FETCH_TIMEOUT_SECONDS);
        assert!(d.config_schema().is_empty());
        assert_eq!(d.remediation(None, &cfg), "");
        assert!(!d.remediation(Some(&ErrorKind::Network), &cfg).is_empty());
        assert!(d.remediation(Some(&ErrorKind::Internal), &cfg).is_empty());
    }

    #[test]
    fn fetch_timeout_parsing() {
        struct Dummy;
        impl SecretSource for Dummy {
            fn name(&self) -> &str { "dummy" }
            fn fetch(&self, _cfg: &HashMap<String, Value>, _home: &Path) -> FetchResult { FetchResult::default() }
        }
        let d = Dummy;
        let mut cfg = HashMap::new();
        cfg.insert("timeout_seconds".to_string(), Value::Number(5.0));
        assert_eq!(d.fetch_timeout_seconds(&cfg), 5.0);
        cfg.insert("timeout_seconds".to_string(), Value::String("10".to_string()));
        assert_eq!(d.fetch_timeout_seconds(&cfg), 10.0);
        cfg.insert("timeout_seconds".to_string(), Value::Number(-1.0));
        assert_eq!(d.fetch_timeout_seconds(&cfg), DEFAULT_FETCH_TIMEOUT_SECONDS);
        cfg.insert("timeout_seconds".to_string(), Value::String("bad".to_string()));
        assert_eq!(d.fetch_timeout_seconds(&cfg), DEFAULT_FETCH_TIMEOUT_SECONDS);
        cfg.insert("timeout_seconds".to_string(), Value::Int(0));
        assert_eq!(d.fetch_timeout_seconds(&cfg), DEFAULT_FETCH_TIMEOUT_SECONDS);
    }

    #[test]
    fn run_secret_cli_echo() {
        // Use /bin/sh which is universally available on Linux
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), "echo hello".to_string()];
        let res = run_secret_cli(&argv, &[], None, 5.0).expect("echo should succeed");
        assert_eq!(res.returncode, 0);
        assert!(res.stdout.contains("hello"));
    }

    #[test]
    fn run_secret_cli_timeout() {
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), "sleep 2".to_string()];
        let res = run_secret_cli(&argv, &[], None, 0.3);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("timed out"));
    }

    #[test]
    fn run_secret_cli_missing_binary() {
        let argv = vec!["/nonexistent/binary_xyz_hermes_test".to_string()];
        let res = run_secret_cli(&argv, &[], None, 2.0);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("failed to invoke"));
    }

    #[test]
    fn run_secret_cli_scrubs_ansi_stderr() {
        // printf with ANSI in stderr
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), "printf '\\033[31merr\\033[0m' >&2".to_string()];
        let res = run_secret_cli(&argv, &[], None, 5.0).expect("should succeed");
        assert_eq!(res.stderr, "err");
    }

    #[test]
    fn run_secret_cli_extra_env() {
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), "echo $MY_TEST_VAR".to_string()];
        let mut extra = HashMap::new();
        extra.insert("MY_TEST_VAR".to_string(), "hello_extra".to_string());
        // MY_TEST_VAR is not in allow_env, but extra_env should inject it
        let res = run_secret_cli(&argv, &[], Some(&extra), 5.0).expect("should succeed");
        assert!(res.stdout.contains("hello_extra"));
    }
}
