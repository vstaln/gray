//! Typed gateway monitoring events.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/monitoring/events.py` (86 lines).
//!
//! Content-free service-health and redacted diagnostic events for the gateway
//! daemon. These are the only event shapes the monitoring plane emits: no
//! prompts, messages, tool args/results, session history, or usage analytics.
//!
//! Python source docstring (preserved):
//! ```text
//! Typed gateway monitoring events.
//!
//! Content-free service-health and redacted diagnostic events for the gateway
//! daemon. These are the only event shapes the monitoring plane emits: no
//! prompts, messages, tool args/results, session history, or usage analytics.
//! ```

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// helpers — mirrors `_now_ns` (lines 15-16)
// ---------------------------------------------------------------------------

/// Mirrors `_now_ns() -> int` (lines 15-16): `return time.time_ns()`.
#[inline]
pub fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

#[allow(dead_code)]
fn _now_ns() -> i64 {
    now_ns()
}

// ---------------------------------------------------------------------------
// Dict value — type-erased dict entry to mirror Python's `Dict[str, Any]`
// returned by `to_dict()` (heterogeneous: str/int/bool/None). Keeps this
// crate dependency-free (no serde) while preserving type info for callers
// that need it. Call `.to_string_value()` for a stringified view.
// ---------------------------------------------------------------------------

/// Heterogeneous dict value mirroring Python's `Any` in `Dict[str, Any]`.
/// Mirrors the `asdict(self)` flattening in each `to_dict()` (lines 41-42,
///
/// 62-63, 78-79).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    String(String),
    Int(i64),
    Bool(bool),
    Null,
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}
impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n)
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl Value {
    /// Stringified view (for logging / map-of-strings consumers).
    pub fn to_string_value(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Int(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => "null".to_string(),
        }
    }
}

fn opt_str(v: &Option<String>) -> Value {
    match v {
        Some(s) => Value::String(s.clone()),
        None => Value::Null,
    }
}
fn opt_bool(v: &Option<bool>) -> Value {
    match v {
        Some(b) => Value::Bool(*b),
        None => Value::Null,
    }
}
fn opt_int(v: &Option<i64>) -> Value {
    match v {
        Some(n) => Value::Int(*n),
        None => Value::Null,
    }
}

// ---------------------------------------------------------------------------
// GatewayHealthEvent — mirrors `@dataclass(slots=True) class GatewayHealthEvent`
// (lines 19-42)
// ---------------------------------------------------------------------------

/// Content-free gateway health snapshot or lifecycle event.
/// Mirrors `GatewayHealthEvent` (lines 19-42).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayHealthEvent {
    pub name: String,
    pub gateway_state: Option<String>,
    pub old_state: Option<String>,
    pub new_state: Option<String>,
    pub exit_reason: Option<String>,
    pub restart_requested: Option<bool>,
    pub active_agents: i64,
    pub gateway_busy: bool,
    pub gateway_drainable: bool,
    pub platform_count: i64,
    pub fatal_platform_count: i64,
    pub profile: Option<String>,
    pub install_id: Option<String>,
    pub version: Option<String>,
    pub supervision_mode: Option<String>,
    pub pid: Option<i64>,
    pub ts_ns: i64,
}

impl GatewayHealthEvent {
    /// Mirrors `GatewayHealthEvent(name=..., ts_ns=_now_ns())` construction.
    /// Required `name`; everything else defaults as in Python (lines 23-39).
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            gateway_state: None,
            old_state: None,
            new_state: None,
            exit_reason: None,
            restart_requested: None,
            active_agents: 0,
            gateway_busy: false,
            gateway_drainable: false,
            platform_count: 0,
            fatal_platform_count: 0,
            profile: None,
            install_id: None,
            version: None,
            supervision_mode: None,
            pid: None,
            ts_ns: now_ns(),
        }
    }

    /// Full constructor mirroring dataclass fields (lines 23-39).
    #[allow(clippy::too_many_arguments)]
    pub fn with_fields(
        name: impl Into<String>,
        gateway_state: Option<String>,
        old_state: Option<String>,
        new_state: Option<String>,
        exit_reason: Option<String>,
        restart_requested: Option<bool>,
        active_agents: i64,
        gateway_busy: bool,
        gateway_drainable: bool,
        platform_count: i64,
        fatal_platform_count: i64,
        profile: Option<String>,
        install_id: Option<String>,
        version: Option<String>,
        supervision_mode: Option<String>,
        pid: Option<i64>,
        ts_ns: Option<i64>,
    ) -> Self {
        Self {
            name: name.into(),
            gateway_state,
            old_state,
            new_state,
            exit_reason,
            restart_requested,
            active_agents,
            gateway_busy,
            gateway_drainable,
            platform_count,
            fatal_platform_count,
            profile,
            install_id,
            version,
            supervision_mode,
            pid,
            ts_ns: ts_ns.unwrap_or_else(now_ns),
        }
    }

    /// Mirrors `to_dict(self) -> Dict[str, Any]` (lines 41-42):
    /// `return {"event": "gateway_health", **asdict(self)}`.
    pub fn to_dict(&self) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert("event".to_string(), Value::String("gateway_health".to_string()));
        m.insert("name".to_string(), Value::String(self.name.clone()));
        m.insert("gateway_state".to_string(), opt_str(&self.gateway_state));
        m.insert("old_state".to_string(), opt_str(&self.old_state));
        m.insert("new_state".to_string(), opt_str(&self.new_state));
        m.insert("exit_reason".to_string(), opt_str(&self.exit_reason));
        m.insert(
            "restart_requested".to_string(),
            opt_bool(&self.restart_requested),
        );
        m.insert(
            "active_agents".to_string(),
            Value::Int(self.active_agents),
        );
        m.insert("gateway_busy".to_string(), Value::Bool(self.gateway_busy));
        m.insert(
            "gateway_drainable".to_string(),
            Value::Bool(self.gateway_drainable),
        );
        m.insert(
            "platform_count".to_string(),
            Value::Int(self.platform_count),
        );
        m.insert(
            "fatal_platform_count".to_string(),
            Value::Int(self.fatal_platform_count),
        );
        m.insert("profile".to_string(), opt_str(&self.profile));
        m.insert("install_id".to_string(), opt_str(&self.install_id));
        m.insert("version".to_string(), opt_str(&self.version));
        m.insert(
            "supervision_mode".to_string(),
            opt_str(&self.supervision_mode),
        );
        m.insert("pid".to_string(), opt_int(&self.pid));
        m.insert("ts_ns".to_string(), Value::Int(self.ts_ns));
        m
    }

    /// Convenience: event discriminator (the `"event"` key in `to_dict`).
    pub fn event_name() -> &'static str {
        "gateway_health"
    }
}

impl Default for GatewayHealthEvent {
    fn default() -> Self {
        Self {
            name: String::new(),
            gateway_state: None,
            old_state: None,
            new_state: None,
            exit_reason: None,
            restart_requested: None,
            active_agents: 0,
            gateway_busy: false,
            gateway_drainable: false,
            platform_count: 0,
            fatal_platform_count: 0,
            profile: None,
            install_id: None,
            version: None,
            supervision_mode: None,
            pid: None,
            ts_ns: now_ns(),
        }
    }
}

// ---------------------------------------------------------------------------
// GatewayDiagnosticEvent — mirrors `@dataclass(slots=True) class GatewayDiagnosticEvent`
// (lines 45-63)
// ---------------------------------------------------------------------------

/// Redacted gateway diagnostic event for operator-owned observability.
/// Mirrors `GatewayDiagnosticEvent` (lines 45-63).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayDiagnosticEvent {
    pub name: String,
    pub subsystem: String,
    pub error_class: String,
    pub error_code: Option<String>,
    pub platform: Option<String>,
    pub old_state: Option<String>,
    pub new_state: Option<String>,
    pub profile: Option<String>,
    pub version: Option<String>,
    pub severity: String,
    pub ts_ns: i64,
    pub source_logger: Option<String>,
}

impl GatewayDiagnosticEvent {
    /// Mirrors construction with defaults (lines 49-60).
    pub fn new(name: impl Into<String>, subsystem: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            subsystem: subsystem.into(),
            error_class: "unknown".to_string(),
            error_code: None,
            platform: None,
            old_state: None,
            new_state: None,
            profile: None,
            version: None,
            severity: "warning".to_string(),
            ts_ns: now_ns(),
            source_logger: None,
        }
    }

    /// Full constructor mirroring dataclass fields (lines 49-60).
    #[allow(clippy::too_many_arguments)]
    pub fn with_fields(
        name: impl Into<String>,
        subsystem: impl Into<String>,
        error_class: Option<String>,
        error_code: Option<String>,
        platform: Option<String>,
        old_state: Option<String>,
        new_state: Option<String>,
        profile: Option<String>,
        version: Option<String>,
        severity: Option<String>,
        ts_ns: Option<i64>,
        source_logger: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            subsystem: subsystem.into(),
            error_class: error_class.unwrap_or_else(|| "unknown".to_string()),
            error_code,
            platform,
            old_state,
            new_state,
            profile,
            version,
            severity: severity.unwrap_or_else(|| "warning".to_string()),
            ts_ns: ts_ns.unwrap_or_else(now_ns),
            source_logger,
        }
    }

    /// Mirrors `to_dict(self) -> Dict[str, Any]` (lines 62-63):
    /// `return {"event": "gateway_diagnostic", **asdict(self)}`.
    pub fn to_dict(&self) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert(
            "event".to_string(),
            Value::String("gateway_diagnostic".to_string()),
        );
        m.insert("name".to_string(), Value::String(self.name.clone()));
        m.insert(
            "subsystem".to_string(),
            Value::String(self.subsystem.clone()),
        );
        m.insert(
            "error_class".to_string(),
            Value::String(self.error_class.clone()),
        );
        m.insert("error_code".to_string(), opt_str(&self.error_code));
        m.insert("platform".to_string(), opt_str(&self.platform));
        m.insert("old_state".to_string(), opt_str(&self.old_state));
        m.insert("new_state".to_string(), opt_str(&self.new_state));
        m.insert("profile".to_string(), opt_str(&self.profile));
        m.insert("version".to_string(), opt_str(&self.version));
        m.insert(
            "severity".to_string(),
            Value::String(self.severity.clone()),
        );
        m.insert("ts_ns".to_string(), Value::Int(self.ts_ns));
        m.insert(
            "source_logger".to_string(),
            opt_str(&self.source_logger),
        );
        m
    }

    pub fn event_name() -> &'static str {
        "gateway_diagnostic"
    }
}

impl Default for GatewayDiagnosticEvent {
    fn default() -> Self {
        Self {
            name: String::new(),
            subsystem: String::new(),
            error_class: "unknown".to_string(),
            error_code: None,
            platform: None,
            old_state: None,
            new_state: None,
            profile: None,
            version: None,
            severity: "warning".to_string(),
            ts_ns: now_ns(),
            source_logger: None,
        }
    }
}

// ---------------------------------------------------------------------------
// CronExecutionEvent — mirrors `@dataclass(slots=True) class CronExecutionEvent`
// (lines 66-79)
// ---------------------------------------------------------------------------

/// Content-free durable cron execution lifecycle projection.
/// Mirrors `CronExecutionEvent` (lines 66-79).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExecutionEvent {
    pub status: String,
    pub job_key: String,
    pub source: String,
    pub duration_ms: Option<i64>,
    pub delivery_outcome: Option<String>,
    pub error_class: Option<String>,
    pub ts_ns: i64,
}

impl CronExecutionEvent {
    /// Mirrors construction with defaults (lines 70-76).
    pub fn new(status: impl Into<String>, job_key: impl Into<String>) -> Self {
        Self {
            status: status.into(),
            job_key: job_key.into(),
            source: "unknown".to_string(),
            duration_ms: None,
            delivery_outcome: None,
            error_class: None,
            ts_ns: now_ns(),
        }
    }

    /// Full constructor mirroring dataclass fields (lines 70-76).
    pub fn with_fields(
        status: impl Into<String>,
        job_key: impl Into<String>,
        source: Option<String>,
        duration_ms: Option<i64>,
        delivery_outcome: Option<String>,
        error_class: Option<String>,
        ts_ns: Option<i64>,
    ) -> Self {
        Self {
            status: status.into(),
            job_key: job_key.into(),
            source: source.unwrap_or_else(|| "unknown".to_string()),
            duration_ms,
            delivery_outcome,
            error_class,
            ts_ns: ts_ns.unwrap_or_else(now_ns),
        }
    }

    /// Mirrors `to_dict(self) -> Dict[str, Any]` (lines 78-79):
    /// `return {"event": "cron_execution", **asdict(self)}`.
    pub fn to_dict(&self) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert(
            "event".to_string(),
            Value::String("cron_execution".to_string()),
        );
        m.insert("status".to_string(), Value::String(self.status.clone()));
        m.insert("job_key".to_string(), Value::String(self.job_key.clone()));
        m.insert("source".to_string(), Value::String(self.source.clone()));
        m.insert("duration_ms".to_string(), opt_int(&self.duration_ms));
        m.insert(
            "delivery_outcome".to_string(),
            opt_str(&self.delivery_outcome),
        );
        m.insert("error_class".to_string(), opt_str(&self.error_class));
        m.insert("ts_ns".to_string(), Value::Int(self.ts_ns));
        m
    }

    pub fn event_name() -> &'static str {
        "cron_execution"
    }
}

impl Default for CronExecutionEvent {
    fn default() -> Self {
        Self {
            status: String::new(),
            job_key: String::new(),
            source: "unknown".to_string(),
            duration_ms: None,
            delivery_outcome: None,
            error_class: None,
            ts_ns: now_ns(),
        }
    }
}
