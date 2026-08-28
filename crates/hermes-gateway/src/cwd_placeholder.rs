//! Resolve gateway `terminal.cwd` placeholder values to `TERMINAL_CWD`.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/cwd_placeholder.py` (49 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! Resolve gateway ``terminal.cwd`` placeholder values to ``TERMINAL_CWD``.
//!
//! When ``terminal.cwd`` is unset or a placeholder (``.``, ``auto``, ``cwd``),
//! the gateway must not blindly map host ``Path.home()`` into container backends.
//! Docker with workspace mounting still needs an explicit host path signal
//! (``MESSAGING_CWD`` or an absolute config path) for ``terminal_tool`` to map
//! ``/host/project`` → ``/workspace``.
//! ```
//!
//! Mapping:
//! - `CWD_PLACEHOLDERS = frozenset({".", "auto", "cwd"})` → [`CWD_PLACEHOLDERS`] + [`is_cwd_placeholder`]
//! - `def _truthy_env(value: str | None) -> bool` → [`_truthy_env`]
//! - `def resolve_placeholder_terminal_cwd(*, configured_cwd, terminal_backend, messaging_cwd, docker_mount_cwd_to_workspace, home_fallback) -> str | None` → [`resolve_placeholder_terminal_cwd`]

/// Placeholder values that mean "no explicit cwd configured".
///
/// Mirrors `CWD_PLACEHOLDERS = frozenset({".", "auto", "cwd"})`.
pub const CWD_PLACEHOLDERS: &[&str] = &[".", "auto", "cwd"];

/// Return true if `value` is a placeholder (exact match, no trimming).
///
/// Mirrors `value in CWD_PLACEHOLDERS`.
pub fn is_cwd_placeholder(value: &str) -> bool {
    // Exact match only — Python does `configured_cwd not in CWD_PLACEHOLDERS` without strip.
    matches!(value, "." | "auto" | "cwd")
}

// ---------------------------------------------------------------------------
// _truthy_env — mirrors Python `def _truthy_env(value: str | None) -> bool`
// ---------------------------------------------------------------------------

/// Return true for truthy env-string values.
///
/// Mirrors:
/// ```python
/// def _truthy_env(value: str | None) -> bool:
///     return (value or "").strip().lower() in {"true", "1", "yes"}
/// ```
pub fn _truthy_env(value: Option<&str>) -> bool {
    matches!(
        value.unwrap_or("").trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes"
    )
}

/// Alias without leading underscore for callers that prefer idiomatic Rust.
pub fn truthy_env(value: Option<&str>) -> bool {
    _truthy_env(value)
}

// ---------------------------------------------------------------------------
// resolve_placeholder_terminal_cwd — mirrors Python top-level function
// ---------------------------------------------------------------------------

/// Return the `TERMINAL_CWD` value to set, or `None` to leave it unset.
///
/// Mirrors:
/// ```python
/// def resolve_placeholder_terminal_cwd(
///     *,
///     configured_cwd: str,
///     terminal_backend: str,
///     messaging_cwd: str | None,
///     docker_mount_cwd_to_workspace: bool,
///     home_fallback: str,
/// ) -> str | None:
///     """Return the ``TERMINAL_CWD`` value to set, or ``None`` to leave it unset.
///
///     Cases:
///       - **local** + placeholder → ``MESSAGING_CWD`` or ``home_fallback``
///       - **docker** + placeholder + mount on + host ``MESSAGING_CWD`` → host path
///         (for ``terminal_tool`` ``/workspace`` mapping)
///       - **docker** + placeholder + mount off → ``None`` (sandbox default)
///       - other non-local backends + placeholder → ``None``
///     """
///     if configured_cwd and configured_cwd not in CWD_PLACEHOLDERS:
///         return configured_cwd
///
///     backend = (terminal_backend or "local").strip().lower()
///     if backend == "local":
///         messaging = (messaging_cwd or "").strip()
///         return messaging or home_fallback
///
///     if backend == "docker" and docker_mount_cwd_to_workspace:
///         messaging = (messaging_cwd or "").strip()
///         if messaging and messaging not in CWD_PLACEHOLDERS:
///             return messaging
///
///     return None
/// ```
pub fn resolve_placeholder_terminal_cwd(
    configured_cwd: &str,
    terminal_backend: &str,
    messaging_cwd: Option<&str>,
    docker_mount_cwd_to_workspace: bool,
    home_fallback: &str,
) -> Option<String> {
    // `if configured_cwd and configured_cwd not in CWD_PLACEHOLDERS: return configured_cwd`
    // Empty string is falsy in Python → falls through.
    if !configured_cwd.is_empty() && !is_cwd_placeholder(configured_cwd) {
        return Some(configured_cwd.to_string());
    }

    // `backend = (terminal_backend or "local").strip().lower()`
    // Python: `or` checks truthiness before strip, so whitespace string stays whitespace then becomes "".
    let backend_raw = if terminal_backend.is_empty() {
        "local"
    } else {
        terminal_backend
    };
    let backend = backend_raw.trim().to_ascii_lowercase();

    if backend == "local" {
        let messaging = messaging_cwd.unwrap_or("").trim();
        if !messaging.is_empty() {
            return Some(messaging.to_string());
        }
        return Some(home_fallback.to_string());
    }

    if backend == "docker" && docker_mount_cwd_to_workspace {
        let messaging = messaging_cwd.unwrap_or("").trim();
        if !messaging.is_empty() && !is_cwd_placeholder(messaging) {
            return Some(messaging.to_string());
        }
    }

    None
}
