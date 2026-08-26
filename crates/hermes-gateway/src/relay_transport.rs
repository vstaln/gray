//! Relay transport protocol — the gateway<->connector wire contract. EXPERIMENTAL.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/relay/transport.py` (143 LOC).
//!
//! The `RelayAdapter` (gateway side) delegates all wire I/O to a `RelayTransport`.
//! The gateway dials OUT to the connector, so a production transport is a WebSocket
//! client; in tests it is an in-memory stub (`tests/gateway/relay/stub_connector.py`).
//!
//! This module defines the protocol surface only — no concrete transport. The
//! contract has four concerns:
//!
//!   1. Lifecycle: `connect` / `disconnect`.
//!   2. Handshake: `handshake` returns the `CapabilityDescriptor` the connector
//!      advertises for the platform this adapter fronts.
//!   3. Inbound: `set_inbound_handler` registers a callback the transport invokes
//!      with each normalized `MessageEvent` the connector delivers.
//!   4. Outbound: `send_outbound` carries send/edit/typing actions back to the
//!      connector; `get_chat_info` proxies a chat-info lookup; `send_interrupt`
//!      routes a mid-turn /stop down the socket that owns the session_key.
//!
//! EXPERIMENTAL: may change without a deprecation cycle until >=2 Class-1 platforms
//! validate it. See docs/relay-connector-contract.md.
//!
//! Python source docstring (preserved):
//! ```text
//! Relay transport protocol — the gateway<->connector wire contract. EXPERIMENTAL.
//!
//! The ``RelayAdapter`` (gateway side) delegates all wire I/O to a ``RelayTransport``.
//! The gateway dials OUT to the connector, so a production transport is a WebSocket
//! client; in tests it is an in-memory stub (``tests/gateway/relay/stub_connector.py``).
//!
//! This module defines the protocol surface only — no concrete transport. The
//! contract has four concerns:
//!
//!   1. Lifecycle: ``connect`` / ``disconnect``.
//!   2. Handshake: ``handshake`` returns the ``CapabilityDescriptor`` the connector
//!      advertises for the platform this adapter fronts.
//!   3. Inbound: ``set_inbound_handler`` registers a callback the transport invokes
//!      with each normalized ``MessageEvent`` the connector delivers.
//!   4. Outbound: ``send_outbound`` carries send/edit/typing actions back to the
//!      connector; ``get_chat_info`` proxies a chat-info lookup; ``send_interrupt``
//!      routes a mid-turn /stop down the socket that owns the session_key.
//!
//! EXPERIMENTAL: may change without a deprecation cycle until >=2 Class-1 platforms
//! validate it. See docs/relay-connector-contract.md.
//! ```

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// CapabilityDescriptor — mirrors `gateway.relay.descriptor.CapabilityDescriptor`
// Minimal projection needed for the transport handshake. Full definition lives
// in `gateway/relay/descriptor.py`; duplicated here so this protocol module
// stays self-contained (mirrors Python's `from gateway.relay.descriptor import
// CapabilityDescriptor` without introducing a circular file dep).
// ---------------------------------------------------------------------------

/// Immutable capability descriptor negotiated at relay handshake.
///
/// Frozen in Python (`@dataclass(frozen=True)`); `Clone` + no interior
/// mutability preserve that invariant in Rust. Mirrors
/// `gateway.relay.descriptor.CapabilityDescriptor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityDescriptor {
    pub contract_version: i32,
    pub platform: String,
    pub label: String,
    pub max_message_length: i32,
    pub supports_draft_streaming: bool,
    pub supports_edit: bool,
    pub supports_threads: bool,
    pub markdown_dialect: String,
    pub len_unit: String,
    #[serde(default = "default_emoji")]
    pub emoji: String,
    #[serde(default)]
    pub platform_hint: String,
    #[serde(default)]
    pub pii_safe: bool,
    #[serde(default)]
    pub supports_context: bool,
    #[serde(default)]
    pub supports_inchannel_continuable: bool,
    #[serde(default)]
    pub supports_block_formatting: bool,
    #[serde(default)]
    pub supported_ops: Vec<String>,
}

fn default_emoji() -> String {
    "\u{1f50c}".to_string()
}

impl CapabilityDescriptor {
    pub const LEGACY_OPS: &'static [&'static str] = &["send", "edit", "typing", "follow_up"];

    /// Whether the connector advertises the outbound op `op`.
    ///
    /// Fail-open for legacy connectors: empty `supported_ops` means the connector
    /// predates op discovery, so assume the legacy op set. Mirrors
    /// `CapabilityDescriptor.supports_op`.
    pub fn supports_op(&self, op: &str) -> bool {
        if self.supported_ops.is_empty() {
            return Self::LEGACY_OPS.contains(&op);
        }
        self.supported_ops.iter().any(|o| o == op)
    }
}

// ---------------------------------------------------------------------------
// MessageEvent — mirrors `gateway.platforms.base.MessageEvent`
// Minimal normalized inbound event. Full definition lives in
// `gateway/platforms/base.py`; this projection carries the fields the relay
// plane actually routes (text + transport metadata). Callers needing the full
// surface should use the real `MessageEvent` from the base module.
// ---------------------------------------------------------------------------

/// Normalized inbound message event delivered by the connector.
///
/// Mirrors `gateway.platforms.base.MessageEvent` (dataclass, 30+ fields).
/// This minimal projection keeps the transport protocol free of a heavy base
/// import; the `metadata` / `raw_message` catch-alls preserve extensibility
/// for fields not projected here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEvent {
    pub text: String,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub user_name: Option<String>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub raw_message: Option<serde_json::Value>,
    #[serde(default)]
    pub timestamp: Option<f64>,
}

impl MessageEvent {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            message_id: None,
            user_id: None,
            user_name: None,
            chat_id: None,
            metadata: serde_json::Value::Object(Default::default()),
            raw_message: None,
            timestamp: None,
        }
    }
}

// ---------------------------------------------------------------------------
// PassthroughForward — mirrors `gateway/relay/ws_transport.py::PassthroughForward`
// Typed as `Any` in the protocol module to keep this file free of a concrete-
// transport import (ws_transport imports FROM this module). Rust keeps the same
// decoupled posture by aliasing to `serde_json::Value`.
// ---------------------------------------------------------------------------

/// Forwarded passthrough request (Discord interactions, Twilio, …).
///
/// Mirrors `gateway.relay.ws_transport.PassthroughForward`. Typed as
/// `serde_json::Value` here (like Python's `Any`) to avoid a concrete-transport
/// import cycle.
pub type PassthroughForward = serde_json::Value;

// ---------------------------------------------------------------------------
// Callback aliases — mirrors Python `InboundHandler` / `PassthroughHandler`
// ---------------------------------------------------------------------------

/// Callback the transport invokes for each inbound normalized event.
/// Mirrors `InboundHandler = Callable[[MessageEvent], Awaitable[None]]`.
pub type InboundHandler =
    Arc<dyn Fn(MessageEvent) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// Callback the transport invokes for each forwarded passthrough request (§5.1).
/// Mirrors `PassthroughHandler = Callable[[Any, Optional[str]], Awaitable[None]]`.
/// First arg is a `PassthroughForward` (typed as `Any` in Python to break the
/// `ws_transport` import cycle); second is an optional `bufferId` (Phase 5 §5.3
/// buffered flip) the handler acks after durable handoff.
pub type PassthroughHandler = Arc<
    dyn Fn(PassthroughForward, Option<String>) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

// ---------------------------------------------------------------------------
// RelayTransport — mirrors `class RelayTransport(Protocol)` / `@runtime_checkable`
// ---------------------------------------------------------------------------

/// Full gateway<->connector transport contract.
///
/// Mirrors `gateway.relay.transport.RelayTransport` (Protocol, runtime_checkable).
/// `runtime_checkable` has no Rust equivalent; trait-object downcasting via
/// `Any` is the closest analogue if needed — not wired here because no
/// concrete transport in this module performs protocol checks at runtime.
#[async_trait::async_trait]
pub trait RelayTransport: Send + Sync {
    /// Open the connection to the connector; return true on success.
    /// Mirrors `async def connect(self) -> bool`.
    async fn connect(&mut self) -> bool;

    /// Close the connection.
    /// Mirrors `async def disconnect(self) -> None`.
    async fn disconnect(&mut self);

    /// Return the capability descriptor the connector advertises.
    /// Mirrors `async def handshake(self) -> CapabilityDescriptor`.
    async fn handshake(&mut self) -> CapabilityDescriptor;

    /// Register the callback invoked with each inbound `MessageEvent`.
    /// Mirrors `def set_inbound_handler(self, handler: InboundHandler) -> None`.
    fn set_inbound_handler(&mut self, handler: InboundHandler);

    /// Register the callback invoked with each forwarded passthrough request.
    ///
    /// Phase 5 §5.1: the passthrough plane (Discord interactions, Twilio, …)
    /// answers the provider's edge ACK at the connector, then forwards the real
    /// request to the gateway over this same outbound socket (a hosted gateway
    /// has no public inbound port). The transport invokes `handler(forward,
    /// buffer_id)` for each `passthrough_forward` frame. Optional on a
    /// transport (an in-memory stub may not implement it).
    ///
    /// Mirrors `def set_passthrough_handler(self, handler: PassthroughHandler) -> None`.
    fn set_passthrough_handler(&mut self, handler: PassthroughHandler);

    /// Carry an outbound action (send/edit/typing) to the connector.
    ///
    /// Returns a result dict; for `op == "send"` it carries `success` and
    /// optionally `message_id` / `error`.
    ///
    /// `platform` (Phase 1.5) tags WHICH fronted platform this reply targets,
    /// carried on the OutboundFrame envelope so a gateway fronting N platforms
    /// egresses each reply through the right sender (the transport resolves the
    /// matching advertised botId). Omitted ⇒ the connector falls back to the
    /// session's default platform (single-platform deploys unchanged).
    ///
    /// Mirrors `async def send_outbound(self, action: Dict[str, Any], *, platform: Optional[str] = None) -> Dict[str, Any]`.
    async fn send_outbound(
        &self,
        action: serde_json::Value,
        platform: Option<String>,
    ) -> serde_json::Value;

    /// Proxy a chat-info lookup to the connector.
    /// Mirrors `async def get_chat_info(self, chat_id: str) -> Dict[str, Any]`.
    async fn get_chat_info(&self, chat_id: String) -> serde_json::Value;

    /// Route a mid-turn /stop to the connector for `session_key`.
    ///
    /// The connector forwards it down the socket owned by the gateway
    /// instance running that session (the /stop routing invariant). On the
    /// gateway side this is the OUTBOUND direction; the actual task
    /// cancellation happens when the connector echoes an interrupt inbound
    /// (handled in Task 1.4).
    ///
    /// Mirrors `async def send_interrupt(self, session_key: str, reason: Optional[str] = None) -> None`.
    async fn send_interrupt(&self, session_key: String, reason: Option<String>);

    /// Ask the connector to flip this instance to buffered-only (Phase 5 §5.3).
    ///
    /// Sends `going_idle` and awaits the connector's `going_idle_ack` — the
    /// connector-authoritative confirmation that live delivery stopped and inbound
    /// now buffers durably for replay on reconnect (Q-5.3c). Returns true on ack,
    /// false on timeout / not-connected (the caller proceeds to close regardless;
    /// without §5.3 wiring there is simply no buffering). Optional on a transport
    /// (an in-memory stub may not implement it). Emitted as part of the gateway's
    /// EXISTING drain transition — not a new idle path.
    ///
    /// Mirrors `async def go_idle(self, timeout_s: float = 10.0) -> bool`.
    async fn go_idle(&self, timeout_s: f64) -> bool;

    /// Act on a shared-identity capability bound to a session (A2 outbound).
    ///
    /// Some platforms hand the connector a credential that acts on the SHARED
    /// bot identity (e.g. a Discord interaction follow-up token, valid ~15min).
    /// Under A2 that credential NEVER reaches the gateway — the connector
    /// stripped it at the edge and bound it in its capability vault keyed by
    /// the session. To use it, the gateway issues a SEMANTIC action against the
    /// session it is already in; it never names or holds a token.
    ///
    /// The action dict carries:
    ///   `op`          == `"follow_up"`
    ///   `session_key` the session whose bound capability to wield
    ///   `kind`        the capability kind (e.g. `"discord.interaction_token"`)
    ///   `content`     the message content to send via that capability
    ///   `metadata?`   optional extras
    ///
    /// The connector resolves the real capability (`resolveOutboundCapability`
    /// on its side), enforces the tenant match (tenant B can never wield tenant
    /// A's capability), and egresses. Returns `{success, message_id?, error?}`;
    /// `success` is false when the capability is absent/expired or the tenant
    /// doesn't match — the gateway then has nothing to retry with (by design: a
    /// leaked gateway holds zero capability material).
    ///
    /// Mirrors `async def send_follow_up(self, action: Dict[str, Any], *, platform: Optional[str] = None) -> Dict[str, Any]`.
    async fn send_follow_up(
        &self,
        action: serde_json::Value,
        platform: Option<String>,
    ) -> serde_json::Value;
}

// ---------------------------------------------------------------------------
// Defaults — mirrors Python default argument values
// ---------------------------------------------------------------------------

/// Default `timeout_s` for `go_idle`. Mirrors `go_idle(timeout_s: float = 10.0)`.
pub const DEFAULT_GO_IDLE_TIMEOUT_S: f64 = 10.0;

// Provide private aliases mirroring Python's underscore-prefixed helpers for traceability
#[allow(dead_code)]
fn _default_go_idle_timeout() -> f64 {
    DEFAULT_GO_IDLE_TIMEOUT_S
}
