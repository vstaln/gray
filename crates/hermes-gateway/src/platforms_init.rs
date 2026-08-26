//! Platform adapters for messaging integrations.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/platforms/__init__.py` (45 LOC).
//!
//! Python source docstring (preserved):
//! ```text
//! Platform adapters for messaging integrations.
//!
//! Each adapter handles:
//! - Receiving messages from a platform
//! - Sending messages/responses back
//! - Platform-specific authentication
//! - Message formatting and media handling
//! ```
//!
//! Python source (preserved verbatim, 45 LOC):
//! ```python
//! """
//! Platform adapters for messaging integrations.
//!
//! Each adapter handles:
//! - Receiving messages from a platform
//! - Sending messages/responses back
//! - Platform-specific authentication
//! - Message formatting and media handling
//! """
//!
//! from .base import BasePlatformAdapter, MessageEvent, SendResult
//!
//! # QQAdapter and YuanbaoAdapter were previously imported eagerly here, but
//! # nothing in the codebase consumes ``from gateway.platforms import
//! # QQAdapter`` (every real call site uses the long-form path
//! # ``from gateway.platforms.qqbot import QQAdapter``). The eager imports
//! # pulled in qqbot's chunked-upload + keyboards + onboard machinery and
//! # yuanbao's websocket stack — about 48 ms wall and ~8 MB RSS on every
//! # CLI invocation, even ones that never touch a gateway adapter.
//! #
//! # Use PEP 562 module ``__getattr__`` to keep the public re-export working
//! # while deferring the actual import to first attribute access. This is
//! # 100% backward-compatible for any external code that still imports the
//! # adapters from the package root.
//! __all__ = [
//!     "BasePlatformAdapter",
//!     "MessageEvent",
//!     "SendResult",
//!     "QQAdapter",
//!     "YuanbaoAdapter",
//! ]
//!
//!
//! def __getattr__(name):
//!     if name == "QQAdapter":
//!         from .qqbot import QQAdapter  # noqa: F401
//!         return QQAdapter
//!     if name == "YuanbaoAdapter":
//!         from .yuanbao import YuanbaoAdapter  # noqa: F401
//!         return YuanbaoAdapter
//!     raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
//!
//!
//! def __dir__():
//!     return sorted(__all__)
//! ```
//!
//! Mapping:
//! - `from .base import BasePlatformAdapter, MessageEvent, SendResult` → [`BasePlatformAdapter`], [`MessageEvent`], [`SendResult`] (eager re-exports; stub types until `crate::platforms_base` lands)
//! - `__all__` → [`ALL`] / [`__ALL__`]
//! - `def __getattr__(name)` (PEP 562 lazy re-exports for `QQAdapter`/`YuanbaoAdapter`) → [`__getattr__`] / [`getattr`] + [`is_lazy_export`] / [`LAZY_EXPORTS`]
//! - `def __dir__()` → [`__dir__`] / [`dir_sorted`]
//! - Eager vs lazy split (48 ms wall, ~8 MB RSS saved on CLI invocations that never touch gateway) → [`EAGER_EXPORTS`] vs [`LAZY_EXPORTS`] docs + [`__getattr__`] deferral
//!
//! Rust notes:
//! - Until `crate::platforms_base`, `crate::qqbot_adapter`, `crate::yuanbao_adapter` are ported,
//!   this module documents the unified public surface and exposes `ALL` plus stub types/fns
//!   for 1:1 discoverability. Re-exports will be wired as `pub use crate::platforms_base::{BasePlatformAdapter, MessageEvent, SendResult}`
//!   and `pub use crate::qqbot_adapter::QQAdapter` / `crate::yuanbao_adapter::YuanbaoAdapter` once those modules exist.
//!   ponytail: stub surface until submodules land; wire pub use when modules land.
//! - `__getattr__` in Python is module-level PEP 562; in Rust it is a free function returning
//!   `Result<&'static str, String>` mirroring `AttributeError(f"module {__name__!r} has no attribute {name!r}")`.

// ---------------------------------------------------------------------------
// Public surface — mirrors `__all__` (5 entries)
// ---------------------------------------------------------------------------

/// Unified public surface, mirroring Python `__all__` (5 entries).
pub const ALL: &[&str] = &[
    "BasePlatformAdapter",
    "MessageEvent",
    "SendResult",
    "QQAdapter",
    "YuanbaoAdapter",
];

/// Alias matching Python `__all__` name for grep discoverability.
pub const __ALL__: &[&str] = ALL;

/// Eager re-exports from `.base` — always available without lazy indirection.
///
/// Mirrors `from .base import BasePlatformAdapter, MessageEvent, SendResult`.
/// These three are the hot path; they stay eager to avoid `__getattr__` overhead
/// on the common `from gateway.platforms import BasePlatformAdapter` case.
pub const EAGER_EXPORTS: &[&str] = &["BasePlatformAdapter", "MessageEvent", "SendResult"];

/// Lazy re-exports deferred via PEP 562 `__getattr__`.
///
/// Mirrors the two adapter imports that were hoisted out of the eager `from .qqbot import QQAdapter`
/// / `from .yuanbao import YuanbaoAdapter` path to save ~48 ms wall + ~8 MB RSS per CLI invocation.
/// Every real call site uses the long-form path (`gateway.platforms.qqbot.QQAdapter`), so the
/// package-root re-export is backward-compatible but cold until first attribute access.
pub const LAZY_EXPORTS: &[&str] = &["QQAdapter", "YuanbaoAdapter"];

// Re-exports (future):
// Once `crate::platforms_base`, `crate::qqbot_adapter`, `crate::yuanbao_adapter` exist, wire:
//   pub use crate::platforms_base::{BasePlatformAdapter, MessageEvent, SendResult};
//   pub use crate::qqbot_adapter::QQAdapter;
//   pub use crate::yuanbao_adapter::YuanbaoAdapter;
// Until then this module exposes `ALL` plus stub types/fns below.

// ---------------------------------------------------------------------------
// Stub types — eager base exports (gateway/platforms/base.py)
// ---------------------------------------------------------------------------

/// Base platform adapter interface.
///
/// Mirrors `gateway/platforms/base.py::BasePlatformAdapter` (ABC).
/// All platform adapters (Telegram, Discord, WhatsApp, Weixin, and more) inherit from this
/// and implement the required methods.
#[derive(Debug, Clone, Default)]
pub struct BasePlatformAdapter {
    /// Platform identifier (e.g. `"telegram"`, `"qqbot"`, `"yuanbao"`).
    pub platform: String,
    /// Optional config snapshot.
    pub config: Option<serde_json::Value>,
}

/// Incoming message from a platform (normalized representation all adapters produce).
///
/// Mirrors `gateway/platforms/base.py::MessageEvent` (`@dataclass`).
#[derive(Debug, Clone, Default)]
pub struct MessageEvent {
    /// Message text content.
    pub text: String,
    /// Author user id (if available).
    pub user_id: Option<String>,
    /// Author display name (if available).
    pub user_name: Option<String>,
    /// Platform-specific message id.
    pub message_id: Option<String>,
    /// Platform update id (e.g. Telegram `update_id`).
    pub platform_update_id: Option<i64>,
    /// Local file paths for media attachments (image cache).
    pub media_urls: Vec<String>,
    /// Media MIME types parallel to `media_urls`.
    pub media_types: Vec<String>,
    /// Reply target message id (if this message is a reply).
    pub reply_to_message_id: Option<String>,
    /// Free-form per-event metadata (platform-specific signals).
    pub metadata: serde_json::Value,
    /// Whether this event may resolve gateway commands or pending control prompts.
    pub allow_gateway_control: bool,
}

impl MessageEvent {
    /// Check if this is a command message (e.g., `/new`, `/reset`).
    ///
    /// Mirrors `MessageEvent.is_command() -> bool`.
    pub fn is_command(&self) -> bool {
        self.allow_gateway_control && self.text.trim_start().starts_with('/')
    }

    /// Extract command name if this is a command message.
    ///
    /// Mirrors `MessageEvent.get_command() -> Optional[str]`.
    pub fn get_command(&self) -> Option<String> {
        if !self.is_command() {
            return None;
        }
        let command_text = self.text.trim_start();
        let mut parts = command_text.splitn(2, ' ');
        let raw_full = parts.next()?;
        let mut raw = raw_full.get(1..)?.to_lowercase();
        if raw.contains('@') {
            raw = raw.split('@').next().unwrap_or("").to_string();
        }
        if raw.contains('/') {
            return None;
        }
        if raw.is_empty() {
            None
        } else {
            Some(raw)
        }
    }

    /// Get the arguments after a command.
    ///
    /// Mirrors `MessageEvent.get_command_args() -> str`.
    pub fn get_command_args(&self) -> String {
        if !self.is_command() {
            return self.text.clone();
        }
        let command_text = self.text.trim_start();
        let mut parts = command_text.splitn(2, ' ');
        parts.next();
        let args = parts.next().unwrap_or("").to_string();
        args.replace("\u{2014}\u{2014}", "--")
            .replace('\u{2014}', "--")
            .replace('\u{2013}', "-")
    }
}

/// Result of sending a message.
///
/// Mirrors `gateway/platforms/base.py::SendResult` (`@dataclass`).
#[derive(Debug, Clone, Default)]
pub struct SendResult {
    /// Whether the send succeeded.
    pub success: bool,
    /// Platform message id on success.
    pub message_id: Option<String>,
    /// Human-readable error detail on failure.
    pub error: Option<String>,
    /// Raw platform response (if any).
    pub raw_response: Option<serde_json::Value>,
    /// True for transient connection errors — base will retry automatically.
    pub retryable: bool,
    /// Server-requested retry delay in seconds (e.g. Telegram FloodWait).
    pub retry_after: Option<f64>,
    /// Additional message ids when the adapter split an oversized payload.
    pub continuation_message_ids: Vec<String>,
    /// Machine-readable failure category (`SEND_ERROR_KINDS` value).
    pub error_kind: Option<String>,
}

// ---------------------------------------------------------------------------
// Stub types — lazy adapter exports (qqbot / yuanbao)
// ---------------------------------------------------------------------------

/// QQ Bot platform adapter (lazy re-export).
///
/// Mirrors `gateway/platforms/qqbot/adapter.py::QQAdapter` re-exported via PEP 562.
/// Eager import was removed to avoid pulling qqbot's chunked-upload + keyboards + onboard
/// machinery (~48 ms / ~8 MB) on every CLI invocation.
#[derive(Debug, Clone, Default)]
pub struct QQAdapter {
    /// Placeholder config snapshot.
    pub config: Option<serde_json::Value>,
}

/// Yuanbao platform adapter (lazy re-export).
///
/// Mirrors `gateway/platforms/yuanbao.py::YuanbaoAdapter` re-exported via PEP 562.
/// Eager import was removed to avoid pulling yuanbao's websocket stack on every CLI invocation.
#[derive(Debug, Clone, Default)]
pub struct YuanbaoAdapter {
    /// Placeholder config snapshot.
    pub config: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// PEP 562 `__getattr__` / `__dir__` — mirrors Python module-level hooks
// ---------------------------------------------------------------------------

/// PEP 562 `__getattr__` — lazy re-export resolver.
///
/// Mirrors:
/// ```python
/// def __getattr__(name):
///     if name == "QQAdapter":
///         from .qqbot import QQAdapter  # noqa: F401
///         return QQAdapter
///     if name == "YuanbaoAdapter":
///         from .yuanbao import YuanbaoAdapter  # noqa: F401
///         return YuanbaoAdapter
///     raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
/// ```
///
/// Returns `Ok(symbol_name)` for the two lazy adapters, `Err(AttributeError message)` otherwise.
/// The `Ok` payload is the symbol name string (callers can match on it to construct the typed
/// adapter once `crate::qqbot_adapter` / `crate::yuanbao_adapter` land).
pub fn __getattr__(name: &str) -> Result<&'static str, String> {
    match name {
        "QQAdapter" => Ok("QQAdapter"),
        "YuanbaoAdapter" => Ok("YuanbaoAdapter"),
        _ => Err(format!("module 'gateway.platforms' has no attribute {name:?}")),
    }
}

/// Idiomatic alias for [`__getattr__`].
pub fn getattr(name: &str) -> Result<&'static str, String> {
    __getattr__(name)
}

/// Return true if `name` is a lazy PEP 562 export (defers import to first access).
///
/// Mirrors the `if name == "QQAdapter"` / `if name == "YuanbaoAdapter"` branches.
pub fn is_lazy_export(name: &str) -> bool {
    matches!(name, "QQAdapter" | "YuanbaoAdapter")
}

/// Return true if `name` is an eager base export.
pub fn is_eager_export(name: &str) -> bool {
    matches!(name, "BasePlatformAdapter" | "MessageEvent" | "SendResult")
}

/// Return true if `name` is present in `__all__` (eager or lazy).
pub fn is_exported(name: &str) -> bool {
    ALL.contains(&name)
}

/// PEP 562 `__dir__` — sorted public surface.
///
/// Mirrors:
/// ```python
/// def __dir__():
///     return sorted(__all__)
/// ```
pub fn __dir__() -> Vec<&'static str> {
    let mut out = ALL.to_vec();
    out.sort();
    out
}

/// Idiomatic alias for [`__dir__`].
pub fn dir_sorted() -> Vec<&'static str> {
    __dir__()
}

/// Sorted `__all__` as owned `String`s (convenience for callers needing `Vec<String>`).
pub fn dir_sorted_strings() -> Vec<String> {
    let mut out: Vec<String> = ALL.iter().map(|s| s.to_string()).collect();
    out.sort();
    out
}

// Private aliases mirroring Python's double-underscore names for grep traceability
#[allow(dead_code)]
fn _getattr(name: &str) -> Result<&'static str, String> {
    __getattr__(name)
}

#[allow(dead_code)]
fn _dir() -> Vec<&'static str> {
    __dir__()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_matches_python() {
        assert_eq!(
            ALL,
            [
                "BasePlatformAdapter",
                "MessageEvent",
                "SendResult",
                "QQAdapter",
                "YuanbaoAdapter",
            ]
        );
        assert_eq!(__ALL__, ALL);
        assert_eq!(ALL.len(), 5);
        assert_eq!(EAGER_EXPORTS, ["BasePlatformAdapter", "MessageEvent", "SendResult"]);
        assert_eq!(LAZY_EXPORTS, ["QQAdapter", "YuanbaoAdapter"]);
    }

    #[test]
    fn dir_is_sorted_all() {
        let dir = __dir__();
        let mut expected = ALL.to_vec();
        expected.sort();
        assert_eq!(dir, expected);
        assert_eq!(dir_sorted(), expected);
        // String variant matches as well
        let dir_s = dir_sorted_strings();
        let expected_s: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(dir_s, expected_s);
    }

    #[test]
    fn getattr_lazy_ok() {
        assert_eq!(__getattr__("QQAdapter"), Ok("QQAdapter"));
        assert_eq!(__getattr__("YuanbaoAdapter"), Ok("YuanbaoAdapter"));
        assert_eq!(getattr("QQAdapter"), Ok("QQAdapter"));
        assert!(is_lazy_export("QQAdapter"));
        assert!(is_lazy_export("YuanbaoAdapter"));
        assert!(!is_lazy_export("BasePlatformAdapter"));
    }

    #[test]
    fn getattr_eager_not_lazy() {
        // Eager exports are not resolved via __getattr__ — they are direct imports from .base.
        // Mirrors Python: __getattr__ only handles QQAdapter/YuanbaoAdapter.
        assert!(__getattr__("BasePlatformAdapter").is_err());
        assert!(__getattr__("MessageEvent").is_err());
        assert!(__getattr__("SendResult").is_err());
        assert!(is_eager_export("BasePlatformAdapter"));
        assert!(is_eager_export("MessageEvent"));
        assert!(is_eager_export("SendResult"));
        assert!(!is_lazy_export("MessageEvent"));
    }

    #[test]
    fn getattr_unknown_err_matches_python_message() {
        let err = __getattr__("NotExist").unwrap_err();
        assert_eq!(err, "module 'gateway.platforms' has no attribute 'NotExist'");
        let err2 = __getattr__("").unwrap_err();
        assert_eq!(err2, "module 'gateway.platforms' has no attribute ''");
        assert!(!is_exported("NotExist"));
        assert!(is_exported("QQAdapter"));
        assert!(is_exported("MessageEvent"));
    }

    #[test]
    fn message_event_command_helpers() {
        let mut ev = MessageEvent {
            text: "/new hello world".to_string(),
            allow_gateway_control: true,
            ..Default::default()
        };
        assert!(ev.is_command());
        assert_eq!(ev.get_command(), Some("new".to_string()));
        assert_eq!(ev.get_command_args(), "hello world");

        // With @bot suffix (Telegram group)
        ev.text = "/new@mybot arg".to_string();
        assert_eq!(ev.get_command(), Some("new".to_string()));
        assert_eq!(ev.get_command_args(), "arg");

        // File path rejected (contains /)
        ev.text = "/etc/passwd".to_string();
        assert_eq!(ev.get_command(), None);

        // allow_gateway_control = false disables command
        ev.text = "/new".to_string();
        ev.allow_gateway_control = false;
        assert!(!ev.is_command());
        assert_eq!(ev.get_command(), None);

        // iOS dash normalization in args
        ev.text = "/queue hello\u{2014}world".to_string();
        ev.allow_gateway_control = true;
        assert_eq!(ev.get_command_args(), "hello--world");
    }

    #[test]
    fn send_result_defaults() {
        let r = SendResult::default();
        assert!(!r.success);
        assert!(r.message_id.is_none());
        assert!(!r.retryable);
        assert!(r.continuation_message_ids.is_empty());
    }
}
