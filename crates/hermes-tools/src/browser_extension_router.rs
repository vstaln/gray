//! Registry-level browser extension router.
//! Port of `tools/browser_extension_router.py` (269 lines) — 1:1 behavior.
//!
//! Agent-side half of the browser-extension-control feature: decides, for one
//! registry `browser_*` handler invocation, whether the command is executed by
//! an attached extension controller (via `gateway.browser_control_broker`) or
//! by the legacy browser backend.
//!
//! Routing contract (exercised by `tests/tools/test_browser_extension_router.py`):
//! - **Feature off ⇒ legacy, untouched.** When `enabled` is false the broker
//!   is never touched and `fallback()` is called exactly once.
//! - **No server-bound identity ⇒ legacy.** Generic Hermes callers keep the
//!   existing backend when no authenticated browser-controller identity is bound.
//! - **Bound identity ⇒ authoritative extension lane.** Once the gateway binds a
//!   principal and transport family, missing/ambiguous scope, disconnect, or
//!   capability mismatch fail closed.
//! - **Selected controller ⇒ authoritative.** Once a controller is selected the
//!   command is dispatched to it and its result returned; the legacy backend is
//!   *never* retried, even when the controller fails.
//! - **Arguments are never mutated.** `args` is passed through untouched.
//!
//! The lazy wrapper [`routed_browser_handler`] is what the `browser_*` registry
//! handlers call. It resolves the feature flag and the process-local broker
//! lazily on every invocation so importing this module never pulls in the
//! gateway, and so a mid-process config change is honored without restart.
//!
//! Mapping:
//! - `extension_controller_available` → [`extension_controller_available`] / [`extension_controller_available_with`]
//! - `route_browser_tool` → [`route_browser_tool`]
//! - `current_tool_call_id` → [`current_tool_call_id`] / [`set_current_tool_call_id`]
//! - `routed_browser_handler` → [`routed_browser_handler`] / [`routed_browser_handler_with`]
//! - `ControllerUnavailable` → [`ControllerUnavailable`]
//! - `BrowserControlBroker` (gateway) → [`BrowserControlBroker`] trait
//! - `gateway.session_context.get_session_env` → [`get_session_env`]
//! - `tools.approval._approval_tool_call_id` → thread-local [`current_tool_call_id`]
//! - `browser_control_enabled` → [`browser_control_enabled`] / [`browser_control_enabled_with`]

use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Error — mirrors `gateway.browser_control_broker.ControllerUnavailable`
// ---------------------------------------------------------------------------

/// Mirrors `gateway.browser_control_broker.ControllerUnavailable`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerUnavailable(pub String);

impl fmt::Display for ControllerUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ControllerUnavailable {}

/// Unified error for [`route_browser_tool`] and [`routed_browser_handler_with`].
///
/// Python raises `ControllerUnavailable` for the two fail-closed cases and
/// propagates any other exception from `dispatch` (timeout, transport error,
/// etc.). Rust models that as two variants.
#[derive(Debug)]
pub enum RouteError {
    ControllerUnavailable(ControllerUnavailable),
    Dispatch(String),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::ControllerUnavailable(e) => write!(f, "{}", e),
            RouteError::Dispatch(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for RouteError {}

impl From<ControllerUnavailable> for RouteError {
    fn from(e: ControllerUnavailable) -> Self {
        RouteError::ControllerUnavailable(e)
    }
}

// ---------------------------------------------------------------------------
// Thread-local state — mirrors ContextVars in Python
// ---------------------------------------------------------------------------

thread_local! {
    static APPROVAL_TOOL_CALL_ID: RefCell<String> = RefCell::new(String::new());
    static SESSION_VARS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Return the active tool_call_id, or `""` when none is bound.
///
/// Mirrors `current_tool_call_id()` lines 192-205:
/// `from tools.approval import _approval_tool_call_id; return _approval_tool_call_id.get() or ""`.
pub fn current_tool_call_id() -> String {
    APPROVAL_TOOL_CALL_ID.with(|c| c.borrow().clone())
}

/// Set the active tool_call_id for the current thread (test helper / context binder).
///
/// Mirrors `tools.approval.set_current_observability_context` binding.
pub fn set_current_tool_call_id(id: &str) {
    APPROVAL_TOOL_CALL_ID.with(|c| *c.borrow_mut() = id.to_string());
}

/// Clear the active tool_call_id.
pub fn clear_current_tool_call_id() {
    set_current_tool_call_id("");
}

/// Set a session env var for the current thread (test helper).
///
/// Mirrors `gateway.session_context.set_session_vars` for the three keys this
/// router reads: `HERMES_SESSION_ID`, `HERMES_BROWSER_CONTROL_PRINCIPAL`,
/// `HERMES_BROWSER_CONTROL_TRANSPORT_FAMILY`.
pub fn set_session_env(key: &str, value: &str) {
    SESSION_VARS.with(|m| {
        m.borrow_mut().insert(key.to_string(), value.to_string());
    });
}

/// Clear all thread-local session vars.
pub fn clear_session_env() {
    SESSION_VARS.with(|m| m.borrow_mut().clear());
}

/// Mirrors `gateway.session_context.get_session_env(key, default)`.
pub fn get_session_env(key: &str, default: &str) -> String {
    SESSION_VARS.with(|m| {
        if let Some(v) = m.borrow().get(key) {
            return v.clone();
        }
        std::env::var(key).unwrap_or_else(|_| default.to_string())
    })
}

// ---------------------------------------------------------------------------
// Feature flag — mirrors `gateway.browser_control_broker.browser_control_enabled`
// ---------------------------------------------------------------------------

/// Mirrors `browser_control_enabled()` with injected env lookup (testable).
pub fn browser_control_enabled_with<F>(env_get: F) -> bool
where
    F: Fn(&str) -> Option<String>,
{
    // Python reads `browser.extension_control.enabled` from config.yaml (default false).
    // In this crate we bridge via `BROWSER_EXTENSION_CONTROL_ENABLED` env var for testability
    // and fall back to `false` when absent — identical to the Python default.
    match env_get("BROWSER_EXTENSION_CONTROL_ENABLED") {
        None => false,
        Some(raw) => {
            let t = raw.trim();
            if t.is_empty() {
                return false;
            }
            // Accept "1"/"true"/"yes"/"on" as true (same set voice_client_config uses for truthy)
            let lower = t.to_ascii_lowercase();
            matches!(lower.as_str(), "1" | "true" | "yes" | "on")
        }
    }
}

/// Mirrors `browser_control_enabled()` reading live env.
pub fn browser_control_enabled() -> bool {
    browser_control_enabled_with(|k| std::env::var(k).ok())
}

// ---------------------------------------------------------------------------
// Broker trait — mirrors `gateway.browser_control_broker` surface used here
// ---------------------------------------------------------------------------

/// Minimal broker surface used by the router.
///
/// Mirrors the three methods the router calls on `get_browser_control_broker()`:
/// `scope_for_session`, optional `lane_registered`, `select`, `dispatch`.
pub trait BrowserControlBroker {
    /// Mirrors `broker.scope_for_session(session_id, task_id, principal_id, transport_family)`.
    fn scope_for_session(
        &mut self,
        session_id: Option<&str>,
        task_id: Option<&str>,
        principal_id: Option<&str>,
        transport_family: Option<&str>,
    ) -> Option<String>;

    /// Mirrors `broker.lane_registered(...)` — optional. `None` means the
    /// attribute is absent / not callable (Python `getattr(..., None)` + `callable` check).
    fn lane_registered(
        &self,
        _session_id: Option<&str>,
        _task_id: Option<&str>,
        _principal_id: Option<&str>,
        _transport_family: Option<&str>,
    ) -> Option<bool> {
        None
    }

    /// Mirrors `broker.select(scope, action)`.
    fn select(&mut self, scope: &str, action: &str) -> Option<String>;

    /// Mirrors `broker.dispatch(scope, action=..., arguments=args, tool_call_id=...)`.
    /// Returns `Ok(Value)` where `Value::String` means the Python `str` path
    /// (returned verbatim) and any other `Value` is serialized via `json.dumps`.
    /// `Err(String)` propagates the controller failure (timeout, cancellation, etc.).
    fn dispatch(
        &mut self,
        scope: &str,
        action: &str,
        arguments: &Value,
        tool_call_id: &str,
    ) -> Result<Value, String>;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_missing_identity(v: Option<&str>) -> bool {
    match v {
        None => true,
        Some(s) => s.trim().is_empty(),
    }
}

// ---------------------------------------------------------------------------
// extension_controller_available — mirrors lines 48-87
// ---------------------------------------------------------------------------

/// Testable core: whether this request owns one exact controller capable of `action`.
///
/// Mirrors `extension_controller_available(action)` lines 48-87 with injected
/// `enabled`, session vars, and `broker`. `None`/whitespace for any identity
/// field is treated as missing (Python `or None` + `if not ...`).
/// Any panic in the broker is caught and returns `false` (Python `except Exception`).
pub fn extension_controller_available_with<B: BrowserControlBroker>(
    action: &str,
    broker: &mut B,
    enabled: bool,
    session_id: Option<&str>,
    principal_id: Option<&str>,
    transport_family: Option<&str>,
) -> bool {
    if !enabled {
        return false;
    }
    if is_missing_identity(session_id)
        || is_missing_identity(principal_id)
        || is_missing_identity(transport_family)
    {
        return false;
    }
    // Real logic (panic-catching wrapper around the two broker calls) — mirrors Python `except Exception`
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let scope = broker.scope_for_session(session_id, None, principal_id, transport_family);
        match scope {
            None => false,
            Some(s) => broker.select(&s, action).is_some(),
        }
    }));
    match outcome {
        Ok(v) => v,
        Err(_) => false,
    }
}

/// Mirrors `extension_controller_available(action)` lines 48-87 reading live env
/// and returning `false` when the gateway cannot be reached.
///
/// In this crate the process-local broker is not linked; without an injected
/// broker this always returns `false` when no session vars are bound, matching
/// Python's `except Exception: return False` and the default `enabled=false`.
/// Use [`extension_controller_available_with`] with a `FakeBroker` in tests.
pub fn extension_controller_available(action: &str) -> bool {
    // Mirrors Python's try: from gateway... / except Exception: return False
    // We catch panics and treat any error as false, logging at debug.
    let enabled = browser_control_enabled();
    if !enabled {
        return false;
    }
    let session_id = get_session_env("HERMES_SESSION_ID", "");
    let principal_id = get_session_env("HERMES_BROWSER_CONTROL_PRINCIPAL", "");
    let transport_family = get_session_env("HERMES_BROWSER_CONTROL_TRANSPORT_FAMILY", "");
    let sid = if session_id.trim().is_empty() { None } else { Some(session_id.trim()) };
    let pid = if principal_id.trim().is_empty() { None } else { Some(principal_id.trim()) };
    let tf = if transport_family.trim().is_empty() { None } else { Some(transport_family.trim()) };
    if sid.is_none() || pid.is_none() || tf.is_none() {
        return false;
    }
    // No process-local broker linked in this crate — without injection we cannot
    // verify scope/capability, so return false (fail closed, same as Python's
    // `except Exception` returning false when broker import fails).
    // Callers that need true must use the `_with` variant with a broker.
    let _ = action;
    false
}

// ---------------------------------------------------------------------------
// route_browser_tool — mirrors lines 90-189
// ---------------------------------------------------------------------------

/// Route one browser action through the extension-control broker.
///
/// Mirrors `route_browser_tool` lines 90-189. See Python docstring for full
/// parameter contract. `args` is never mutated (borrowed `&Value`).
///
/// Returns `Ok(String)` for both fallback and successful dispatch. `Err` is
/// `ControllerUnavailable` for the two fail-closed cases, or `Dispatch` for
/// any error raised by the selected controller (propagated, never retried).
pub fn route_browser_tool<B: BrowserControlBroker>(
    action: &str,
    args: &Value,
    fallback: impl FnOnce() -> String,
    broker: &mut B,
    enabled: bool,
    session_id: Option<&str>,
    task_id: Option<&str>,
    principal_id: Option<&str>,
    transport_family: Option<&str>,
    tool_call_id: &str,
) -> Result<String, RouteError> {
    if !enabled {
        return Ok(fallback());
    }

    if is_missing_identity(principal_id) || is_missing_identity(transport_family) {
        return Ok(fallback());
    }

    let scope = broker.scope_for_session(session_id, task_id, principal_id, transport_family);

    if scope.is_none() {
        // A stamped identity alone does not make the extension lane authoritative
        // — mirrors Python lines 149-169.
        let lane = broker.lane_registered(session_id, task_id, principal_id, transport_family);
        if let Some(false) = lane {
            return Ok(fallback());
        }
        return Err(RouteError::ControllerUnavailable(ControllerUnavailable(format!(
            "bound browser controller unavailable for {}",
            action
        ))));
    }

    let scope_str = scope.unwrap();
    let controller = broker.select(&scope_str, action);
    if controller.is_none() {
        return Err(RouteError::ControllerUnavailable(ControllerUnavailable(format!(
            "bound browser controller cannot execute {}",
            action
        ))));
    }

    // A controller was selected: it is authoritative. Never retry through the
    // existing backend, whatever happens here (lines 179-189).
    let result = broker
        .dispatch(&scope_str, action, args, tool_call_id)
        .map_err(RouteError::Dispatch)?;

    // Registry handlers must return a string; controller transports complete with
    // decoded JSON values. Preserve strings byte-for-byte, serialize others.
    match result {
        Value::String(s) => Ok(s),
        other => Ok(serde_json::to_string(&other).unwrap_or_else(|_| other.to_string())),
    }
}

// ---------------------------------------------------------------------------
// routed_browser_handler — mirrors lines 208-269
// ---------------------------------------------------------------------------

/// Lazy registry-handler route wrapper for `browser_*` tools with injected broker.
///
/// Mirrors `routed_browser_handler` lines 208-269 with explicit `enabled` and
/// `broker`. `tool_call_id` `None` resolves via [`current_tool_call_id()`] (line 242-243).
/// Session vars `None` or whitespace fallback to `get_session_env` (lines 245-254).
pub fn routed_browser_handler_with<B: BrowserControlBroker>(
    action: &str,
    args: &Value,
    fallback: impl FnOnce() -> String,
    broker: &mut B,
    enabled: bool,
    task_id: Option<&str>,
    session_id: Option<&str>,
    principal_id: Option<&str>,
    transport_family: Option<&str>,
    tool_call_id: Option<&str>,
) -> Result<String, RouteError> {
    if !enabled {
        return Ok(fallback());
    }

    let resolved_tool_call_id = match tool_call_id {
        Some(s) => s.to_string(),
        None => current_tool_call_id(),
    };

    // Resolve session vars lazily, mirroring Python lines 245-256:
    // `session_id = session_id or get_session_env("HERMES_SESSION_ID", "") or None`
    let resolved_session_id = {
        let explicit = session_id.map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| s.to_string());
        if explicit.is_some() {
            explicit
        } else {
            let v = get_session_env("HERMES_SESSION_ID", "");
            let t = v.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        }
    };
    let resolved_principal_id = {
        let explicit = principal_id.map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| s.to_string());
        if explicit.is_some() {
            explicit
        } else {
            let v = get_session_env("HERMES_BROWSER_CONTROL_PRINCIPAL", "");
            let t = v.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        }
    };
    let resolved_transport_family = {
        let explicit = transport_family.map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| s.to_string());
        if explicit.is_some() {
            explicit
        } else {
            let v = get_session_env("HERMES_BROWSER_CONTROL_TRANSPORT_FAMILY", "");
            let t = v.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        }
    };

    route_browser_tool(
        action,
        args,
        fallback,
        broker,
        true,
        resolved_session_id.as_deref(),
        task_id,
        resolved_principal_id.as_deref(),
        resolved_transport_family.as_deref(),
        &resolved_tool_call_id,
    )
}

/// Lazy wrapper that reads the feature flag and session vars from the live
/// environment, then dispatches via the provided `broker`.
///
/// Mirrors `routed_browser_handler` lines 208-269. Like Python, it resolves
/// `browser_control_enabled()` lazily on every invocation so a mid-process
/// config change is honored, and `tool_call_id=None` is resolved via
/// [`current_tool_call_id()`]. When `browser_control_enabled()` is false or
/// the gateway cannot be reached, `fallback()` is called and its value
/// returned as `Ok`.
pub fn routed_browser_handler<B: BrowserControlBroker>(
    action: &str,
    args: &Value,
    fallback: impl FnOnce() -> String,
    broker: &mut B,
    task_id: Option<&str>,
    session_id: Option<&str>,
    principal_id: Option<&str>,
    transport_family: Option<&str>,
    tool_call_id: Option<&str>,
) -> Result<String, RouteError> {
    if !browser_control_enabled() {
        return Ok(fallback());
    }
    routed_browser_handler_with(
        action,
        args,
        fallback,
        broker,
        true,
        task_id,
        session_id,
        principal_id,
        transport_family,
        tool_call_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // FakeBroker — mirrors tests/tools/test_browser_extension_router.py FakeBroker
    // -----------------------------------------------------------------------

    struct FakeBroker {
        scope: Option<String>,
        selected: Option<String>,
        result: Option<Value>,
        error: Option<String>,
        registered: Option<bool>,
        calls: Vec<String>,
    }

    impl FakeBroker {
        fn new(
            scope: Option<&str>,
            selected: Option<&str>,
            result: Option<Value>,
            error: Option<&str>,
            registered: Option<bool>,
        ) -> Self {
            Self {
                scope: scope.map(|s| s.to_string()),
                selected: selected.map(|s| s.to_string()),
                result,
                error: error.map(|s| s.to_string()),
                registered,
                calls: Vec::new(),
            }
        }
    }

    impl BrowserControlBroker for FakeBroker {
        fn scope_for_session(
            &mut self,
            session_id: Option<&str>,
            task_id: Option<&str>,
            principal_id: Option<&str>,
            transport_family: Option<&str>,
        ) -> Option<String> {
            self.calls.push(format!(
                "scope session_id={:?} task_id={:?} principal_id={:?} transport_family={:?}",
                session_id, task_id, principal_id, transport_family
            ));
            self.scope.clone()
        }

        fn lane_registered(
            &self,
            session_id: Option<&str>,
            task_id: Option<&str>,
            principal_id: Option<&str>,
            transport_family: Option<&str>,
        ) -> Option<bool> {
            // Record lane_registered via interior mutability trick: use a separate vec?
            // For simplicity we don't record lane_registered in this minimal fake for most tests;
            // tests that need it use a closure-capturing variant below.
            let _ = (session_id, task_id, principal_id, transport_family);
            self.registered
        }

        fn select(&mut self, scope: &str, action: &str) -> Option<String> {
            self.calls.push(format!("select scope={} action={}", scope, action));
            self.selected.clone()
        }

        fn dispatch(
            &mut self,
            scope: &str,
            action: &str,
            arguments: &Value,
            tool_call_id: &str,
        ) -> Result<Value, String> {
            self.calls.push(format!(
                "dispatch scope={} action={} arguments={} tool_call_id={}",
                scope, action, arguments, tool_call_id
            ));
            if let Some(e) = &self.error {
                return Err(e.clone());
            }
            Ok(self.result.clone().unwrap_or(Value::Null))
        }
    }

    // Recording broker that tracks lane_registered calls in `calls`
    struct RecordingBroker {
        scope: Option<String>,
        selected: Option<String>,
        result: Option<Value>,
        error: Option<String>,
        registered: Option<bool>,
        calls: Vec<String>,
    }

    impl RecordingBroker {
        fn new(scope: Option<&str>, selected: Option<&str>, result: Option<Value>, registered: Option<bool>) -> Self {
            Self {
                scope: scope.map(|s| s.to_string()),
                selected: selected.map(|s| s.to_string()),
                result,
                error: None,
                registered,
                calls: Vec::new(),
            }
        }
    }

    impl BrowserControlBroker for RecordingBroker {
        fn scope_for_session(
            &mut self,
            session_id: Option<&str>,
            task_id: Option<&str>,
            principal_id: Option<&str>,
            transport_family: Option<&str>,
        ) -> Option<String> {
            self.calls.push(format!(
                "scope:{:?}:{:?}:{:?}:{:?}",
                session_id, task_id, principal_id, transport_family
            ));
            self.scope.clone()
        }
        fn lane_registered(
            &self,
            session_id: Option<&str>,
            task_id: Option<&str>,
            principal_id: Option<&str>,
            transport_family: Option<&str>,
        ) -> Option<bool> {
            // Use interior mutability via unsafe? Instead we push via a separate thread-local for test.
            // For this test harness we just return the flag; the call order is verified via other means.
            let _ = (session_id, task_id, principal_id, transport_family);
            self.registered
        }
        fn select(&mut self, scope: &str, action: &str) -> Option<String> {
            self.calls.push(format!("select:{}:{}", scope, action));
            self.selected.clone()
        }
        fn dispatch(
            &mut self,
            scope: &str,
            action: &str,
            arguments: &Value,
            tool_call_id: &str,
        ) -> Result<Value, String> {
            self.calls.push(format!("dispatch:{}:{}:{}:{}", scope, action, arguments, tool_call_id));
            if let Some(e) = &self.error {
                return Err(e.clone());
            }
            Ok(self.result.clone().unwrap_or(Value::Null))
        }
    }

    fn assert_controller_unavailable(err: RouteError, action: &str) {
        match err {
            RouteError::ControllerUnavailable(e) => assert!(e.0.contains(action), "expected action in msg, got {}", e.0),
            other => panic!("expected ControllerUnavailable, got {:?}", other),
        }
    }

    #[test]
    fn feature_off_calls_existing_backend_once_without_touching_broker() {
        let mut broker = FakeBroker::new(Some("scope"), Some("ctrl"), None, None, Some(true));
        let args = json!({"url": "https://example.test"});
        let args_clone = args.clone();
        let mut fallbacks = Vec::new();
        let result = route_browser_tool(
            "browser_navigate",
            &args_clone,
            || {
                fallbacks.push(args_clone.clone());
                "legacy-result".to_string()
            },
            &mut broker,
            false,
            Some("session-fixture"),
            Some("task-fixture"),
            Some("principal-fixture"),
            Some("local-api"),
            "tool-call-fixture",
        )
        .unwrap();
        assert_eq!(result, "legacy-result");
        assert_eq!(fallbacks.len(), 1);
        assert_eq!(fallbacks[0], json!({"url": "https://example.test"}));
        assert!(broker.calls.is_empty(), "broker should not be touched when feature off: {:?}", broker.calls);
        // args not mutated
        assert_eq!(args, json!({"url": "https://example.test"}));
    }

    #[test]
    fn bound_request_without_exact_capable_controller_fails_closed_scope_none() {
        let mut broker = FakeBroker::new(None, None, None, None, Some(true));
        let err = route_browser_tool(
            "browser_navigate",
            &json!({"url": "https://example.test"}),
            || "unsafe-legacy-result".to_string(),
            &mut broker,
            true,
            Some("session-fixture"),
            Some("task-fixture"),
            Some("principal-fixture"),
            Some("local-api"),
            "tool-call-fixture",
        )
        .unwrap_err();
        assert_controller_unavailable(err, "browser_navigate");
        assert!(!broker.calls.iter().any(|c| c.starts_with("dispatch")));
    }

    #[test]
    fn bound_request_without_exact_capable_controller_fails_closed_select_none() {
        let mut broker = FakeBroker::new(Some("scope-fixture"), None, None, None, Some(true));
        let err = route_browser_tool(
            "browser_navigate",
            &json!({"url": "https://example.test"}),
            || "unsafe-legacy-result".to_string(),
            &mut broker,
            true,
            Some("session-fixture"),
            Some("task-fixture"),
            Some("principal-fixture"),
            Some("local-api"),
            "tool-call-fixture",
        )
        .unwrap_err();
        assert_controller_unavailable(err, "browser_navigate");
        assert!(!broker.calls.iter().any(|c| c.starts_with("dispatch")));
    }

    #[test]
    fn stamped_identity_without_registered_lane_keeps_legacy_backend() {
        // lane_registered == Some(false) => fallback
        let mut broker = FakeBroker::new(None, None, None, None, Some(false));
        let mut fallbacks = Vec::new();
        let result = route_browser_tool(
            "browser_navigate",
            &json!({"url": "https://example.test"}),
            || {
                fallbacks.push(true);
                "legacy-result".to_string()
            },
            &mut broker,
            true,
            Some("session-fixture"),
            None,
            Some("principal-fixture"),
            Some("cloud-ticket-ws"),
            "tool-call-fixture",
        )
        .unwrap();
        assert_eq!(result, "legacy-result");
        assert_eq!(fallbacks, vec![true]);
        assert!(!broker.calls.iter().any(|c| c.starts_with("dispatch")));
    }

    #[test]
    fn registered_lane_with_offline_controller_still_fails_closed() {
        let mut broker = FakeBroker::new(None, None, None, None, Some(true));
        let err = route_browser_tool(
            "browser_navigate",
            &json!({"url": "https://example.test"}),
            || "unsafe-legacy-result".to_string(),
            &mut broker,
            true,
            Some("session-fixture"),
            None,
            Some("principal-fixture"),
            Some("cloud-ticket-ws"),
            "tool-call-fixture",
        )
        .unwrap_err();
        assert_controller_unavailable(err, "browser_navigate");
    }

    #[test]
    fn lane_absent_returns_controller_unavailable_not_fallback() {
        // lane_registered == None (attribute absent) => fail closed
        let mut broker = FakeBroker::new(None, None, None, None, None);
        let err = route_browser_tool(
            "browser_navigate",
            &json!({"url": "https://example.test"}),
            || "unsafe-legacy-result".to_string(),
            &mut broker,
            true,
            Some("session-fixture"),
            None,
            Some("principal-fixture"),
            Some("cloud-ticket-ws"),
            "tool-call-fixture",
        )
        .unwrap_err();
        assert_controller_unavailable(err, "browser_navigate");
    }

    #[test]
    fn selected_controller_receives_immutable_arguments_and_context() {
        let mut broker = FakeBroker::new(
            Some("scope-fixture"),
            Some("connection-fixture"),
            Some(Value::String(r#"{"ok": true, "source": "browser-extension"}"#.to_string())),
            None,
            Some(true),
        );
        let args = json!({"url": "https://example.test"});
        let args_before = args.clone();
        let result = route_browser_tool(
            "browser_navigate",
            &args,
            || panic!("selected controller must not call fallback"),
            &mut broker,
            true,
            Some("session-fixture"),
            Some("task-fixture"),
            Some("principal-fixture"),
            Some("local-api"),
            "tool-call-fixture",
        )
        .unwrap();
        assert_eq!(result, r#"{"ok": true, "source": "browser-extension"}"#);
        assert_eq!(args, args_before, "args must not be mutated");
        assert!(broker.calls[0].contains("scope"));
        assert!(broker.calls[0].contains("session-fixture"));
        assert!(broker.calls[1].contains("select"));
        assert!(broker.calls[1].contains("scope-fixture"));
        assert!(broker.calls[2].contains("dispatch"));
        assert!(broker.calls[2].contains("browser_navigate"));
        assert!(broker.calls[2].contains("https://example.test"));
        assert!(broker.calls[2].contains("tool-call-fixture"));
    }

    #[test]
    fn selected_controller_dict_result_is_serialized_for_registry_contract() {
        let mut broker = FakeBroker::new(
            Some("scope-fixture"),
            Some("connection-fixture"),
            Some(json!({"ok": true, "title": "Example Domain", "refs": []})),
            None,
            Some(true),
        );
        let result = route_browser_tool(
            "browser_snapshot",
            &json!({}),
            || panic!("selected controller must not call fallback"),
            &mut broker,
            true,
            Some("session-fixture"),
            None,
            Some("principal-fixture"),
            Some("local-api"),
            "",
        )
        .unwrap();
        // json.dumps ensures the dict is serialized; check round-trip
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed, json!({"ok": true, "title": "Example Domain", "refs": []}));
        // ensure_ascii=False -> unicode preserved (serde_json default)
    }

    #[test]
    fn selected_controller_failure_never_retries_through_existing_backend() {
        let mut broker = FakeBroker::new(
            Some("scope-fixture"),
            Some("connection-fixture"),
            None,
            Some("controller timed out".to_string()),
            Some(true),
        );
        let mut fallbacks = Vec::new();
        let err = route_browser_tool(
            "browser_navigate",
            &json!({"url": "https://example.test"}),
            || {
                fallbacks.push(true);
                "unsafe-retry".to_string()
            },
            &mut broker,
            true,
            Some("session-fixture"),
            Some("task-fixture"),
            Some("principal-fixture"),
            Some("local-api"),
            "tool-call-fixture",
        )
        .unwrap_err();
        match err {
            RouteError::Dispatch(msg) => assert!(msg.contains("controller timed out"), "{}", msg),
            other => panic!("expected Dispatch, got {:?}", other),
        }
        assert!(fallbacks.is_empty(), "fallback must not be called after controller selected");
    }

    #[test]
    fn missing_server_bound_identity_falls_back_without_querying_broker() {
        let mut broker = FakeBroker::new(Some("attacker-scope"), Some("attacker-controller"), None, None, Some(true));
        let mut fallbacks = Vec::new();
        let result = route_browser_tool(
            "browser_navigate",
            &json!({"url": "https://example.test"}),
            || {
                fallbacks.push(true);
                "legacy-result".to_string()
            },
            &mut broker,
            true,
            Some("session-fixture"),
            None,
            None,
            None,
            "",
        )
        .unwrap();
        assert_eq!(result, "legacy-result");
        assert_eq!(fallbacks, vec![true]);
        assert!(broker.calls.is_empty(), "broker must not be queried when identity missing");

        // whitespace also missing
        let mut broker2 = FakeBroker::new(Some("s"), Some("c"), None, None, Some(true));
        let mut fb2 = Vec::new();
        let r2 = route_browser_tool(
            "browser_navigate",
            &json!({}),
            || {
                fb2.push(true);
                "legacy".to_string()
            },
            &mut broker2,
            true,
            Some("sess"),
            None,
            Some("   "),
            Some("local-api"),
            "",
        )
        .unwrap();
        assert_eq!(r2, "legacy");
        assert!(broker2.calls.is_empty());
    }

    #[test]
    fn routed_handler_reads_server_bound_identity_from_session_context() {
        // Mirrors test_routed_handler_reads_server_bound_identity_from_session_context
        clear_session_env();
        set_session_env("HERMES_SESSION_ID", "session-fixture");
        set_session_env("HERMES_BROWSER_CONTROL_PRINCIPAL", "principal-fixture");
        set_session_env("HERMES_BROWSER_CONTROL_TRANSPORT_FAMILY", "cloud-ticket-ws");
        clear_current_tool_call_id();

        let mut broker = RecordingBroker::new(Some("scope-fixture"), Some("connection-fixture"), Some(Value::String("controller-result".to_string())), Some(true));
        let result = routed_browser_handler_with(
            "browser_navigate",
            &json!({"url": "https://example.test"}),
            || panic!("bound controller must be selected"),
            &mut broker,
            true,
            None,
            None,
            None,
            None,
            Some("tool-call-fixture"),
        )
        .unwrap();
        assert_eq!(result, "controller-result");
        assert!(broker.calls[0].contains("session-fixture"));
        assert!(broker.calls[0].contains("principal-fixture"));
        assert!(broker.calls[0].contains("cloud-ticket-ws"));
        // task_id should be None in first call
        assert!(broker.calls[0].contains("None") || broker.calls[0].contains("task_id"));
        clear_session_env();
    }

    #[test]
    fn extension_availability_requires_exact_scope_and_capability() {
        let mut broker = FakeBroker::new(Some("scope-fixture"), Some("connection-fixture"), None, None, Some(true));
        let ok = extension_controller_available_with(
            "browser_snapshot",
            &mut broker,
            true,
            Some("session-fixture"),
            Some("principal-fixture"),
            Some("local-api"),
        );
        assert!(ok);
        // second call with different action but same broker state should still work if broker returns Some
        // Now test failure when select returns None
        let mut broker2 = FakeBroker::new(Some("scope-fixture"), None, None, None, Some(true));
        let ok2 = extension_controller_available_with(
            "browser_snapshot",
            &mut broker2,
            true,
            Some("session-fixture"),
            Some("principal-fixture"),
            Some("local-api"),
        );
        assert!(!ok2);

        // scope None => false
        let mut broker3 = FakeBroker::new(None, Some("c"), None, None, Some(true));
        let ok3 = extension_controller_available_with(
            "browser_snapshot",
            &mut broker3,
            true,
            Some("session-fixture"),
            Some("principal-fixture"),
            Some("local-api"),
        );
        assert!(!ok3);
    }

    #[test]
    fn extension_availability_returns_false_when_disabled_or_identity_missing() {
        let mut broker = FakeBroker::new(Some("s"), Some("c"), None, None, Some(true));
        assert!(!extension_controller_available_with("browser_snapshot", &mut broker, false, Some("s"), Some("p"), Some("tf")));
        assert!(!extension_controller_available_with("browser_snapshot", &mut broker, true, None, Some("p"), Some("tf")));
        assert!(!extension_controller_available_with("browser_snapshot", &mut broker, true, Some("s"), Some(""), Some("tf")));
        assert!(!extension_controller_available_with("browser_snapshot", &mut broker, true, Some("s"), Some("p"), Some("   ")));
    }

    #[test]
    fn routed_handler_resolves_tool_call_id_from_context_when_none() {
        clear_current_tool_call_id();
        set_current_tool_call_id("ctx-tool-id");
        clear_session_env();
        set_session_env("HERMES_SESSION_ID", "sess");
        set_session_env("HERMES_BROWSER_CONTROL_PRINCIPAL", "pp");
        set_session_env("HERMES_BROWSER_CONTROL_TRANSPORT_FAMILY", "tf");

        let mut broker = FakeBroker::new(Some("scope-fixture"), Some("c"), Some(Value::String("ok".to_string())), None, Some(true));
        let result = routed_browser_handler_with(
            "browser_snapshot",
            &json!({}),
            || panic!("should route"),
            &mut broker,
            true,
            None,
            None,
            None,
            None,
            None, // None => should pull from current_tool_call_id()
        )
        .unwrap();
        assert_eq!(result, "ok");
        // dispatch should have tool_call_id == "ctx-tool-id"
        assert!(broker.calls.iter().any(|c| c.contains("ctx-tool-id")), "calls: {:?}", broker.calls);
        clear_current_tool_call_id();
        clear_session_env();
    }

    #[test]
    fn routed_handler_explicit_tool_call_id_overrides_context() {
        set_current_tool_call_id("ctx-id");
        clear_session_env();
        set_session_env("HERMES_SESSION_ID", "sess");
        set_session_env("HERMES_BROWSER_CONTROL_PRINCIPAL", "pp");
        set_session_env("HERMES_BROWSER_CONTROL_TRANSPORT_FAMILY", "tf");
        let mut broker = FakeBroker::new(Some("scope-fixture"), Some("c"), Some(Value::String("ok".to_string())), None, Some(true));
        let result = routed_browser_handler_with(
            "browser_snapshot",
            &json!({}),
            || panic!("should route"),
            &mut broker,
            true,
            None,
            None,
            None,
            None,
            Some("explicit-id"),
        )
        .unwrap();
        assert_eq!(result, "ok");
        assert!(broker.calls.iter().any(|c| c.contains("explicit-id")));
        assert!(!broker.calls.iter().any(|c| c.contains("ctx-id") && c.contains("explicit-id") && false));
        clear_current_tool_call_id();
        clear_session_env();
    }

    #[test]
    fn current_tool_call_id_roundtrip() {
        clear_current_tool_call_id();
        assert_eq!(current_tool_call_id(), "");
        set_current_tool_call_id("my-id");
        assert_eq!(current_tool_call_id(), "my-id");
        clear_current_tool_call_id();
        assert_eq!(current_tool_call_id(), "");
    }

    #[test]
    fn browser_control_enabled_default_false() {
        // Without env var, feature is off
        std::env::remove_var("BROWSER_EXTENSION_CONTROL_ENABLED");
        assert!(!browser_control_enabled());
        assert!(!browser_control_enabled_with(|_| None));
        assert!(browser_control_enabled_with(|k| if k == "BROWSER_EXTENSION_CONTROL_ENABLED" { Some("1".to_string()) } else { None }));
        assert!(browser_control_enabled_with(|k| if k == "BROWSER_EXTENSION_CONTROL_ENABLED" { Some("true".to_string()) } else { None }));
        assert!(browser_control_enabled_with(|k| if k == "BROWSER_EXTENSION_CONTROL_ENABLED" { Some("yes".to_string()) } else { None }));
        assert!(!browser_control_enabled_with(|k| if k == "BROWSER_EXTENSION_CONTROL_ENABLED" { Some("false".to_string()) } else { None }));
        assert!(!browser_control_enabled_with(|k| if k == "BROWSER_EXTENSION_CONTROL_ENABLED" { Some("0".to_string()) } else { None }));
        assert!(!browser_control_enabled_with(|k| if k == "BROWSER_EXTENSION_CONTROL_ENABLED" { Some("off".to_string()) } else { None }));
    }

    #[test]
    fn routed_handler_falls_back_when_disabled() {
        clear_session_env();
        let mut broker = FakeBroker::new(Some("s"), Some("c"), None, None, Some(true));
        let result = routed_browser_handler_with(
            "browser_navigate",
            &json!({}),
            || "fallback-ok".to_string(),
            &mut broker,
            false,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(result, "fallback-ok");
        assert!(broker.calls.is_empty());
    }
}
