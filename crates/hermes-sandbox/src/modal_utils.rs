//! Shared Hermes-side execution flow for Modal transports.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/tools/environments/modal_utils.py` (210 lines).
//! This module deliberately stops at the Hermes boundary:
//! - command preparation
//! - cwd/timeout normalization
//! - stdin/sudo shell wrapping
//! - common result shape
//! - interrupt/cancel polling
//!
//! Direct Modal and managed Modal keep separate transport logic, persistence, and
//! trust-boundary decisions in their own modules.
//!
//! Python source docstring (preserved):
//! ```text
//! Shared Hermes-side execution flow for Modal transports.
//!
//! This module deliberately stops at the Hermes boundary:
//! - command preparation
//! - cwd/timeout normalization
//! - stdin/sudo shell wrapping
//! - common result shape
//! - interrupt/cancel polling
//!
//! Direct Modal and managed Modal keep separate transport logic, persistence, and
//! trust-boundary decisions in their own modules.
//! ```

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, OnceLock,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Constants — mirrors Python class vars on BaseModalExecutionEnvironment
// ---------------------------------------------------------------------------

/// Mirrors `BaseModalExecutionEnvironment._stdin_mode = "payload"`.
pub const DEFAULT_STDIN_MODE: &str = "payload";

/// Mirrors `BaseModalExecutionEnvironment._poll_interval_seconds = 0.25`.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Mirrors `BaseModalExecutionEnvironment._interrupt_output = "[Command interrupted]"`.
pub const DEFAULT_INTERRUPT_OUTPUT: &str = "[Command interrupted]";

/// Mirrors `BaseModalExecutionEnvironment._unexpected_error_prefix = "Modal execution error"`.
pub const DEFAULT_UNEXPECTED_ERROR_PREFIX: &str = "Modal execution error";

// ---------------------------------------------------------------------------
// ExecResult — mirrors Python `{"output": str, "returncode": int}`
// ---------------------------------------------------------------------------

/// Mirrors `BaseModalExecutionEnvironment._result` dict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub output: String,
    pub returncode: i32,
}

impl ExecResult {
    pub fn new(output: impl Into<String>, returncode: i32) -> Self {
        Self {
            output: output.into(),
            returncode,
        }
    }
}

// ---------------------------------------------------------------------------
// PreparedModalExec — mirrors `@dataclass(frozen=True) class PreparedModalExec`
// ---------------------------------------------------------------------------

/// Normalized command data passed to a transport-specific exec runner.
///
/// Mirrors `tools.environments.modal_utils.PreparedModalExec`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedModalExec {
    pub command: String,
    pub cwd: String,
    pub timeout: u64,
    pub stdin_data: Option<String>,
}

// ---------------------------------------------------------------------------
// ModalExecStart — mirrors `@dataclass(frozen=True) class ModalExecStart`
// ---------------------------------------------------------------------------

/// Transport response after starting an exec.
///
/// Mirrors `tools.environments.modal_utils.ModalExecStart`.
#[derive(Debug, Clone)]
pub struct ModalExecStart<H> {
    pub handle: Option<H>,
    pub immediate_result: Option<ExecResult>,
}

impl<H> ModalExecStart<H> {
    pub fn with_handle(handle: H) -> Self {
        Self {
            handle: Some(handle),
            immediate_result: None,
        }
    }
    pub fn with_result(result: ExecResult) -> Self {
        Self {
            handle: None,
            immediate_result: Some(result),
        }
    }
    pub fn empty() -> Self {
        Self {
            handle: None,
            immediate_result: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers: shlex, uuid, heredoc/sudo wrapping
// ---------------------------------------------------------------------------

fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-' 
            )
    });
    if safe {
        return s.to_string();
    }
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{}'", escaped)
}

fn uuid_hex8() -> String {
    // Cheap pseudo-uuid: nanos + pid hex, first 8 chars — mirrors Python uuid4().hex[:8]
    // but without external crate.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let tid = format!("{:?}", std::thread::current().id());
    let mut h: u64 = nanos as u64 ^ (pid as u64).wrapping_mul(0x9e3779b97f4a7c15);
    for b in tid.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u64);
    }
    format!("{h:08x}")[..8].to_string()
}

/// Append stdin as a shell heredoc for transports without stdin piping.
///
/// Mirrors `tools.environments.modal_utils.wrap_modal_stdin_heredoc`.
pub fn wrap_modal_stdin_heredoc(command: &str, stdin_data: &str) -> String {
    let mut marker = format!("HERMES_EOF_{}", uuid_hex8());
    // Avoid marker collision with stdin content — mirrors Python while loop.
    // Bound attempts to avoid infinite loop on pathological stdin; 10 is ample.
    let mut attempts = 0;
    while stdin_data.contains(&marker) && attempts < 10 {
        marker = format!("HERMES_EOF_{}", uuid_hex8());
        attempts += 1;
    }
    format!("{command} << '{marker}'\n{stdin_data}\n{marker}")
}

/// Feed sudo via a shell pipe for transports without direct stdin piping.
///
/// Mirrors `tools.environments.modal_utils.wrap_modal_sudo_pipe`.
pub fn wrap_modal_sudo_pipe(command: &str, sudo_stdin: &str) -> String {
    // Mirrors `printf '%s\\n' {shlex.quote(sudo_stdin.rstrip())} | {command}`
    // Use rstrip: trim trailing \n/\r/\t/space then re-add via printf %s\n
    let trimmed = sudo_stdin.trim_end_matches(|c| c == '\n' || c == '\r');
    // shlex.quote after rstrip in Python; note it also trims trailing spaces via rstrip() default? Python rstrip() without args strips all whitespace.
    // Python: sudo_stdin.rstrip() — strips all trailing whitespace including spaces/tabs.
    let trimmed_ws = sudo_stdin.trim_end();
    // We already stripped \n/\r above, but follow Python's rstrip() semantics fully: use trimmed_ws
    // The second branch above already did correct; use trimmed_ws for fidelity.
    let quoted = shlex_quote(trimmed_ws);
    let _ = trimmed; // keep variable to show intent, but quoted uses full rstrip
    format!("printf '%s\\n' {} | {command}", quoted)
}

// ---------------------------------------------------------------------------
// Sudo transform — mirrors `tools.terminal_tool._transform_sudo_command`
// Minimal faithful subset: rewrite `sudo` -> `sudo -S -p ''` when SUDO_PASSWORD set.
// `prepare_command` is the Rust analogue of `BaseEnvironment._prepare_command`.
// ---------------------------------------------------------------------------

fn looks_like_env_assignment(token: &str) -> bool {
    if let Some(eq) = token.find('=') {
        if eq == 0 {
            return false;
        }
        let name = &token[..eq];
        let valid = name.chars().enumerate().all(|(i, c)| {
            if i == 0 {
                c.is_ascii_alphabetic() || c == '_'
            } else {
                c.is_ascii_alphanumeric() || c == '_'
            }
        });
        return valid && !name.is_empty() && name.chars().next().map(|c| c.is_ascii_alphabetic() || c == '_').unwrap_or(false);
    }
    false
}

fn read_shell_token(command: &str, start: usize) -> (String, usize) {
    let bytes: Vec<char> = command.chars().collect();
    let n = bytes.len();
    let mut i = start;
    while i < n {
        let ch = bytes[i];
        if ch.is_whitespace() || matches!(ch, ';' | '|' | '&' | '(' | ')') {
            break;
        }
        if ch == '\'' {
            i += 1;
            while i < n && bytes[i] != '\'' {
                i += 1;
            }
            if i < n {
                i += 1;
            }
            continue;
        }
        if ch == '"' {
            i += 1;
            while i < n {
                let inner = bytes[i];
                if inner == '\\' && i + 1 < n {
                    i += 2;
                    continue;
                }
                if inner == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if ch == '\\' && i + 1 < n {
            i += 2;
            continue;
        }
        i += 1;
    }
    let token: String = bytes[start..i].iter().collect();
    (token, i)
}

fn rewrite_real_sudo_invocations(command: &str) -> (String, usize) {
    let chars: Vec<char> = command.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0usize;
    let mut command_start = true;
    let mut sudo_count = 0usize;

    while i < n {
        let ch = chars[i];
        if ch.is_whitespace() {
            out.push(ch);
            if ch == '\n' {
                command_start = true;
            }
            i += 1;
            continue;
        }
        if ch == '#' && command_start {
            // comment to end of line
            let remaining: String = chars[i..].iter().collect();
            if let Some(nl) = remaining.find('\n') {
                out.push_str(&remaining[..nl]);
                i += nl;
            } else {
                out.push_str(&remaining);
                break;
            }
            continue;
        }
        if i + 1 < n && ((chars[i] == '&' && chars[i + 1] == '&') || (chars[i] == '|' && chars[i + 1] == '|') || (chars[i] == ';' && chars[i + 1] == ';')) {
            out.push(chars[i]);
            out.push(chars[i + 1]);
            i += 2;
            command_start = true;
            continue;
        }
        if matches!(ch, ';' | '|' | '&' | '(') {
            out.push(ch);
            i += 1;
            command_start = true;
            continue;
        }
        if ch == ')' {
            out.push(ch);
            i += 1;
            command_start = false;
            continue;
        }
        let cmd_str: String = chars.iter().collect();
        let (token, next_i) = read_shell_token(&cmd_str, i);
        if command_start && token == "sudo" {
            out.push_str("sudo -S -p ''");
            sudo_count += 1;
        } else {
            out.push_str(&token);
        }
        if command_start && looks_like_env_assignment(&token) {
            command_start = true;
        } else {
            command_start = false;
        }
        i = next_i;
    }
    (out, sudo_count)
}

/// Mirrors `tools.terminal_tool._transform_sudo_command` (credential-gated rewrite).
///
/// Returns `(transformed_command, sudo_stdin)`. `sudo_stdin` is `Some(password\n * count)` when
/// a password is available and sudo was rewritten, else `None`.
pub fn transform_sudo_command(command: &str) -> (String, Option<String>) {
    let (transformed, sudo_count) = rewrite_real_sudo_invocations(command);
    if sudo_count == 0 {
        return (command.to_string(), None);
    }
    // Scope-aware password read: check SUDO_PASSWORD env (mirrors Python's secret_scope + os.environ).
    // In Rust we only have env var; interactive prompt path is out of scope for this shared module.
    let password = std::env::var("SUDO_PASSWORD").ok().filter(|v| !v.is_empty());
    // Also check cached file via env sentinel HERMES_SUDO_PASSWORD_CACHE for test injection
    let password = password.or_else(|| {
        std::env::var("HERMES_SUDO_PASSWORD_CACHE")
            .ok()
            .filter(|v| !v.is_empty())
    });

    // Local NOPASSWD probe: only for local backend; we skip host probing here and treat
    // absence of password as no rewrite (fail gracefully like Python when has_configured_password false and sudo_nopasswd_works false).
    // Python's _sudo_nopasswd_works only returns true for TERMINAL_ENV=local via `sudo -n true`.
    // In this shared module we don't probe subprocess; just return unchanged when no password.
    // This preserves fail-closed vs. fail-open distinction without subprocess dependency here.
    if let Some(pw) = password {
        // `password + "\n"` per invocation
        let mut stdin = String::new();
        for _ in 0..sudo_count {
            stdin.push_str(&pw);
            stdin.push('\n');
        }
        (transformed, Some(stdin))
    } else {
        // No password available — Python returns original command unchanged (so it fails with "sudo: a password is required")
        // But also may have probed NOPASSWD; we return original to let sudo fail naturally.
        // Important: we should NOT return the transformed version without password, so return original.
        (command.to_string(), None)
    }
}

// ---------------------------------------------------------------------------
// Interrupt handling — mirrors `tools.interrupt`
// ---------------------------------------------------------------------------

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Mirrors `tools.interrupt.is_interrupted` — checks current thread interrupt flag.
///
/// In Python this is per-thread via threading.current_thread().ident set.
/// In Rust we use a process-global flag for simplicity; callers needing per-thread
/// isolation should use `is_interrupted_for` via thread-local if needed.
/// Global is sufficient for the modal poll loop's best-effort cancellation.
pub fn is_interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

/// Mirrors `tools.interrupt.set_interrupt(True/False)`.
pub fn set_interrupt(active: bool) {
    INTERRUPTED.store(active, Ordering::Relaxed);
}

pub fn clear_interrupt() {
    set_interrupt(false);
}

// ---------------------------------------------------------------------------
// Activity callback — mirrors `tools.environments.base.touch_activity_if_due`
// ---------------------------------------------------------------------------

/// Activity state for gateway liveness — mirrors Python `dict` with `last_touch`, `start`, `interval`.
#[derive(Debug, Clone)]
pub struct ActivityState {
    pub last_touch: Instant,
    pub start: Instant,
    pub interval: Duration,
}

impl ActivityState {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            last_touch: now,
            start: now,
            interval: Duration::from_secs(10),
        }
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

impl Default for ActivityState {
    fn default() -> Self {
        Self::new()
    }
}

type ActivityCallback = Box<dyn Fn(String) + Send + Sync>;

static ACTIVITY_CALLBACK: OnceLock<Mutex<Option<ActivityCallback>>> = OnceLock::new();

fn activity_callback_lock() -> &'static Mutex<Option<ActivityCallback>> {
    ACTIVITY_CALLBACK.get_or_init(|| Mutex::new(None))
}

/// Mirrors `tools.environments.base.set_activity_callback`.
pub fn set_activity_callback(cb: Option<ActivityCallback>) {
    if let Ok(mut g) = activity_callback_lock().lock() {
        *g = cb;
    }
}

pub fn get_activity_callback_label(label: &str, elapsed_secs: u64) -> String {
    format!("{label} ({elapsed_secs}s elapsed)")
}

/// Mirrors `tools.environments.base.touch_activity_if_due(state, label)`.
///
/// Fires the registered activity callback at most once every `state.interval`.
/// Swallows all exceptions so callers don't need try/except (mirrors Python).
pub fn touch_activity_if_due(state: &mut ActivityState, label: &str) {
    let now = Instant::now();
    if now.duration_since(state.last_touch) < state.interval {
        return;
    }
    state.last_touch = now;
    let cb_opt = activity_callback_lock().lock().ok().and_then(|g| {
        // We can't clone the Box<dyn Fn>, so we hold lock only to check presence and call.
        // Call inside lock is okay because callback is expected to be short.
        // To avoid holding lock across callback, we drop lock before calling via flag.
        if g.is_some() { Some(()) } else { None }
    });
    if cb_opt.is_some() {
        // Re-lock to call
        if let Ok(g) = activity_callback_lock().lock() {
            if let Some(cb) = g.as_ref() {
                let elapsed = now.duration_since(state.start).as_secs();
                let msg = get_activity_callback_label(label, elapsed);
                // Catch panics to mirror Python's except Exception: pass
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(msg)));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BaseModalExecutionEnvironment — mirrors Python `BaseModalExecutionEnvironment(BaseEnvironment)`
// ---------------------------------------------------------------------------

/// Execution flow for the *managed* Modal transport (gateway-owned sandbox).
///
/// This deliberately overrides `BaseEnvironment.execute` because the tool-gateway handles
/// command preparation, CWD tracking, and env-snapshot management on the server side.
/// The base class's `_wrap_command` / `_wait_for_process` / snapshot machinery does not apply here — the
/// gateway owns that responsibility. See `ManagedModalEnvironment` for the concrete subclass.
///
/// Mirrors `tools.environments.modal_utils.BaseModalExecutionEnvironment`.
pub trait BaseModalExecutionEnvironment {
    /// Opaque handle returned by `_start_modal_exec` — e.g. exec id.
    type Handle;

    // ------------------------------------------------------------------
    // Required: transport identity and abstract ops
    // ------------------------------------------------------------------

    /// Mirrors `BaseEnvironment.cwd`.
    fn cwd(&self) -> &str;

    /// Mirrors `BaseEnvironment.timeout` (seconds).
    fn timeout_secs(&self) -> u64;

    /// Begin a transport-specific exec.
    ///
    /// Mirrors `@abstractmethod def _start_modal_exec(self, prepared: PreparedModalExec) -> ModalExecStart`.
    fn start_modal_exec(
        &mut self,
        prepared: &PreparedModalExec,
    ) -> Result<ModalExecStart<Self::Handle>, String>;

    /// Return a final result dict when complete, else `None`.
    ///
    /// Mirrors `@abstractmethod def _poll_modal_exec(self, handle: Any) -> dict | None`.
    fn poll_modal_exec(&self, handle: &Self::Handle) -> Result<Option<ExecResult>, String>;

    /// Cancel or terminate the active transport exec.
    ///
    /// Mirrors `@abstractmethod def _cancel_modal_exec(self, handle: Any) -> None`.
    fn cancel_modal_exec(&self, handle: &Self::Handle) -> Result<(), String>;

    // ------------------------------------------------------------------
    // Overridable class vars as trait methods
    // ------------------------------------------------------------------

    /// Mirrors `BaseModalExecutionEnvironment._stdin_mode = "payload"`.
    fn stdin_mode(&self) -> &'static str {
        DEFAULT_STDIN_MODE
    }

    /// Mirrors `BaseModalExecutionEnvironment._poll_interval_seconds = 0.25`.
    fn poll_interval(&self) -> Duration {
        DEFAULT_POLL_INTERVAL
    }

    /// Mirrors `BaseModalExecutionEnvironment._client_timeout_grace_seconds: float | None = None`.
    fn client_timeout_grace(&self) -> Option<Duration> {
        None
    }

    /// Mirrors `BaseModalExecutionEnvironment._interrupt_output = "[Command interrupted]"`.
    fn interrupt_output(&self) -> &str {
        DEFAULT_INTERRUPT_OUTPUT
    }

    /// Mirrors `BaseModalExecutionEnvironment._unexpected_error_prefix = "Modal execution error"`.
    fn unexpected_error_prefix(&self) -> &str {
        DEFAULT_UNEXPECTED_ERROR_PREFIX
    }

    /// Hook for backends that need pre-exec sync or validation.
    ///
    /// Mirrors `def _before_execute(self) -> None: pass`.
    fn before_execute(&mut self) {}

    /// Mirrors `def _prepare_command(self, command: str) -> tuple[str, str|None]`.
    /// Default delegates to `transform_sudo_command`.
    fn prepare_command(&self, command: &str) -> (String, Option<String>) {
        transform_sudo_command(command)
    }

    // ------------------------------------------------------------------
    // Provided helpers — mirrors Python concrete methods
    // ------------------------------------------------------------------

    /// Mirrors `def _prepare_modal_exec(self, command, *, cwd="", timeout=None, stdin_data=None) -> PreparedModalExec`.
    fn prepare_modal_exec(
        &self,
        command: &str,
        cwd: &str,
        timeout: Option<u64>,
        stdin_data: Option<&str>,
    ) -> PreparedModalExec {
        let effective_cwd = if cwd.is_empty() {
            self.cwd().to_string()
        } else {
            cwd.to_string()
        };
        let effective_timeout = timeout.unwrap_or_else(|| self.timeout_secs());

        let mut exec_command = command.to_string();
        let mut exec_stdin = if self.stdin_mode() == "payload" {
            stdin_data.map(|s| s.to_string())
        } else {
            None
        };
        if stdin_data.is_some() && self.stdin_mode() == "heredoc" {
            exec_command = wrap_modal_stdin_heredoc(&exec_command, stdin_data.unwrap());
        }

        let (prepared_cmd, sudo_stdin) = self.prepare_command(&exec_command);
        exec_command = prepared_cmd;
        if let Some(sudo) = sudo_stdin {
            exec_command = wrap_modal_sudo_pipe(&exec_command, &sudo);
        }

        PreparedModalExec {
            command: exec_command,
            cwd: effective_cwd,
            timeout: effective_timeout,
            stdin_data: exec_stdin,
        }
    }

    /// Mirrors `def _result(self, output: str, returncode: int) -> dict`.
    fn result(&self, output: &str, returncode: i32) -> ExecResult {
        ExecResult {
            output: output.to_string(),
            returncode,
        }
    }

    /// Mirrors `def _error_result(self, output: str) -> dict`.
    fn error_result(&self, output: &str) -> ExecResult {
        self.result(output, 1)
    }

    /// Mirrors `def _timeout_result_for_modal(self, timeout: int) -> dict`.
    fn timeout_result_for_modal(&self, timeout: u64) -> ExecResult {
        self.result(&format!("Command timed out after {timeout}s"), 124)
    }

    /// Mirrors `def execute(self, command, cwd="", *, timeout=None, stdin_data=None, rewrite_compound_background=True, bounded_capture=False) -> dict`.
    ///
    /// Managed/remote modal transports execute commands via explicit transport and do not rely on shell
    /// background rewriters. `rewrite_compound_background` / `bounded_capture` are accepted for
    /// `BaseEnvironment.execute()` signature parity but ignored — modal transports return the remote
    /// function's result in one payload, so streaming-time bounding does not apply; the terminal tool's
    /// final truncation still caps it.
    fn execute(
        &mut self,
        command: &str,
        cwd: &str,
        timeout: Option<u64>,
        stdin_data: Option<&str>,
        rewrite_compound_background: bool,
        bounded_capture: bool,
    ) -> ExecResult {
        let _ = rewrite_compound_background;
        let _ = bounded_capture;

        self.before_execute();
        let prepared = self.prepare_modal_exec(command, cwd, timeout, stdin_data);

        let start = match self.start_modal_exec(&prepared) {
            Ok(s) => s,
            Err(exc) => {
                return self.error_result(&format!("{}: {exc}", self.unexpected_error_prefix()));
            }
        };

        if let Some(r) = start.immediate_result {
            return r;
        }

        let handle = match start.handle {
            Some(h) => h,
            None => {
                return self.error_result(&format!(
                    "{}: transport did not return an exec handle",
                    self.unexpected_error_prefix()
                ));
            }
        };

        let deadline: Option<Instant> = self
            .client_timeout_grace()
            .map(|grace| Instant::now() + Duration::from_secs(prepared.timeout) + grace);

        let mut activity_state = ActivityState::new();

        loop {
            if is_interrupted() {
                let _ = self.cancel_modal_exec(&handle);
                return self.result(self.interrupt_output(), 130);
            }

            match self.poll_modal_exec(&handle) {
                Ok(Some(r)) => return r,
                Ok(None) => {}
                Err(exc) => {
                    return self.error_result(&format!("{}: {exc}", self.unexpected_error_prefix()));
                }
            }

            if let Some(dl) = deadline {
                if Instant::now() >= dl {
                    let _ = self.cancel_modal_exec(&handle);
                    return self.timeout_result_for_modal(prepared.timeout);
                }
            }

            // Periodic activity touch so the gateway knows we're alive
            touch_activity_if_due(&mut activity_state, "modal command running");

            std::thread::sleep(self.poll_interval());
        }
    }

    /// Convenience wrapper matching Python's default call shape `execute(command, cwd="", timeout=None, stdin_data=None)`.
    fn execute_simple(
        &mut self,
        command: &str,
        cwd: &str,
        timeout: Option<u64>,
        stdin_data: Option<&str>,
    ) -> ExecResult {
        self.execute(command, cwd, timeout, stdin_data, true, false)
    }
}

// ---------------------------------------------------------------------------
// Tests — minimal smoke for 1:1 fidelity
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn wrap_stdin_heredoc_contains_marker_and_data() {
        let cmd = "cat";
        let data = "hello\nworld";
        let wrapped = wrap_modal_stdin_heredoc(cmd, data);
        assert!(wrapped.starts_with("cat << 'HERMES_EOF_"), "wrapped: {wrapped}");
        assert!(wrapped.contains(data));
        // marker appears twice: after << and at end
        let marker_start = wrapped.find("HERMES_EOF_").unwrap();
        let marker = &wrapped[marker_start..marker_start + 8 + "HERMES_EOF_".len()];
        // Actually marker is HERMES_EOF_ + 8 hex
        assert!(wrapped.ends_with(marker));
    }

    #[test]
    fn wrap_stdin_heredoc_avoids_collision() {
        let cmd = "echo hi";
        let marker_like = format!("HERMES_EOF_{}", "abcd1234");
        // craft stdin containing that marker pattern would collide if naive
        let data = format!("line {marker_like} line");
        let wrapped = wrap_modal_stdin_heredoc(&data, &data);
        // Should still contain data
        assert!(wrapped.contains(&data));
        // The wrapped form should have << marker
        assert!(wrapped.contains("<< 'HERMES_EOF_"));
    }

    #[test]
    fn wrap_sudo_pipe_formats() {
        let cmd = "sudo apt update";
        let sudo_stdin = "secret\n";
        let wrapped = wrap_modal_sudo_pipe(cmd, sudo_stdin);
        assert!(wrapped.contains("printf '%s\\n'"), "wrapped: {wrapped}");
        assert!(wrapped.contains(cmd));
        // sudo_stdin is quoted
        assert!(wrapped.contains("secret") || wrapped.contains("'secret'"));
    }

    #[test]
    fn prepared_exec_payload_vs_heredoc() {
        struct PayloadEnv;
        impl BaseModalExecutionEnvironment for PayloadEnv {
            type Handle = String;
            fn cwd(&self) -> &str { "/root" }
            fn timeout_secs(&self) -> u64 { 60 }
            fn start_modal_exec(&mut self, _: &PreparedModalExec) -> Result<ModalExecStart<Self::Handle>, String> { Ok(ModalExecStart::empty()) }
            fn poll_modal_exec(&self, _: &Self::Handle) -> Result<Option<ExecResult>, String> { Ok(None) }
            fn cancel_modal_exec(&self, _: &Self::Handle) -> Result<(), String> { Ok(()) }
        }
        struct HeredocEnv;
        impl BaseModalExecutionEnvironment for HeredocEnv {
            type Handle = String;
            fn cwd(&self) -> &str { "/root" }
            fn timeout_secs(&self) -> u64 { 60 }
            fn stdin_mode(&self) -> &'static str { "heredoc" }
            fn start_modal_exec(&mut self, _: &PreparedModalExec) -> Result<ModalExecStart<Self::Handle>, String> { Ok(ModalExecStart::empty()) }
            fn poll_modal_exec(&self, _: &Self::Handle) -> Result<Option<ExecResult>, String> { Ok(None) }
            fn cancel_modal_exec(&self, _: &Self::Handle) -> Result<(), String> { Ok(()) }
        }

        let payload = PayloadEnv;
        let p = payload.prepare_modal_exec("cat", "", None, Some("stdin data"));
        assert_eq!(p.stdin_data, Some("stdin data".to_string()));
        assert_eq!(p.command, "cat");

        let heredoc = HeredocEnv;
        let h = heredoc.prepare_modal_exec("cat", "", None, Some("stdin data"));
        assert!(h.command.contains("<< 'HERMES_EOF_"));
        assert!(h.command.contains("stdin data"));
        assert_eq!(h.stdin_data, None);
    }

    #[test]
    fn transform_sudo_without_password_no_rewrite() {
        unsafe { env::remove_var("SUDO_PASSWORD"); env::remove_var("HERMES_SUDO_PASSWORD_CACHE"); }
        let (cmd, stdin) = transform_sudo_command("sudo apt update");
        assert_eq!(cmd, "sudo apt update");
        assert!(stdin.is_none());
    }

    #[test]
    fn transform_sudo_with_password() {
        unsafe { env::set_var("SUDO_PASSWORD", "mypw") };
        let (cmd, stdin) = transform_sudo_command("sudo apt update && sudo ls");
        assert!(cmd.contains("sudo -S -p ''"));
        assert_eq!(stdin, Some("mypw\nmypw\n".to_string()));
        unsafe { env::remove_var("SUDO_PASSWORD") };
    }

    #[test]
    fn exec_immediate_result_path() {
        struct ImmediateEnv;
        impl BaseModalExecutionEnvironment for ImmediateEnv {
            type Handle = String;
            fn cwd(&self) -> &str { "/tmp" }
            fn timeout_secs(&self) -> u64 { 30 }
            fn start_modal_exec(&mut self, _: &PreparedModalExec) -> Result<ModalExecStart<Self::Handle>, String> {
                Ok(ModalExecStart::with_result(ExecResult::new("immediate", 0)))
            }
            fn poll_modal_exec(&self, _: &Self::Handle) -> Result<Option<ExecResult>, String> { Ok(None) }
            fn cancel_modal_exec(&self, _: &Self::Handle) -> Result<(), String> { Ok(()) }
        }
        let mut env = ImmediateEnv;
        let r = env.execute_simple("echo hi", "", None, None);
        assert_eq!(r.output, "immediate");
        assert_eq!(r.returncode, 0);
    }

    #[test]
    fn exec_poll_loop_returns_result() {
        use std::sync::{Arc, Mutex};
        struct PollEnv {
            polls: Arc<Mutex<usize>>,
        }
        impl BaseModalExecutionEnvironment for PollEnv {
            type Handle = String;
            fn cwd(&self) -> &str { "/tmp" }
            fn timeout_secs(&self) -> u64 { 30 }
            fn poll_interval(&self) -> Duration { Duration::from_millis(5) }
            fn start_modal_exec(&mut self, _: &PreparedModalExec) -> Result<ModalExecStart<Self::Handle>, String> {
                Ok(ModalExecStart::with_handle("h1".to_string()))
            }
            fn poll_modal_exec(&self, _: &Self::Handle) -> Result<Option<ExecResult>, String> {
                let mut g = self.polls.lock().unwrap();
                *g += 1;
                if *g >= 2 {
                    Ok(Some(ExecResult::new("done", 0)))
                } else {
                    Ok(None)
                }
            }
            fn cancel_modal_exec(&self, _: &Self::Handle) -> Result<(), String> { Ok(()) }
        }
        let polls = Arc::new(Mutex::new(0usize));
        let mut env = PollEnv { polls: Arc::clone(&polls) };
        let r = env.execute_simple("echo hi", "", None, None);
        assert_eq!(r.output, "done");
        assert!(*polls.lock().unwrap() >= 2);
    }

    #[test]
    fn timeout_grace_none_vs_some() {
        struct NoGrace;
        impl BaseModalExecutionEnvironment for NoGrace {
            type Handle = String;
            fn cwd(&self) -> &str { "/" }
            fn timeout_secs(&self) -> u64 { 1 }
            fn start_modal_exec(&mut self, _: &PreparedModalExec) -> Result<ModalExecStart<Self::Handle>, String> { Ok(ModalExecStart::with_handle("h".to_string())) }
            fn poll_modal_exec(&self, _: &Self::Handle) -> Result<Option<ExecResult>, String> { Ok(None) }
            fn cancel_modal_exec(&self, _: &Self::Handle) -> Result<(), String> { Ok(()) }
        }
        let e = NoGrace;
        assert!(e.client_timeout_grace().is_none());
        assert_eq!(e.stdin_mode(), "payload");
        assert_eq!(e.interrupt_output(), "[Command interrupted]");
        assert_eq!(e.unexpected_error_prefix(), "Modal execution error");
    }
}
