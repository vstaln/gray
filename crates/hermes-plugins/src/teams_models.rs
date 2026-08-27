//! Normalized models for the Teams meeting pipeline plugin.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/plugins/teams_pipeline/models.py` (350 LOC).
//!
//! Python surface ported line-for-line:
//!   - `ArtifactType`
//!   - `_parse_datetime`, `_serialize_datetime`, `_clean_dict`
//!   - `GraphSubscription`
//!   - `TeamsMeetingRef`
//!   - `MeetingArtifact`
//!   - `TeamsMeetingSummaryPayload`
//!   - `TeamsMeetingPipelineJob`
//!
//! Transport notes (mirrors Python side-effects without `cargo` in this task):
//!   - `datetime` objects are stored as normalized ISO-8601 `Z` strings (`Option<String>` / `String`).
//!     `_parse_datetime` / `_serialize_datetime` are string normalizers (trim, `Z` ↔ `+00:00`,
//!     empty → `None`, naive → UTC `Z`) without `chrono` so the crate stays `std`-only.
//!     Real port would use `chrono::DateTime<Utc>` with `fromisoformat` / `astimezone(UTC)` semantics.
//!   - `dataclass(field(default_factory=dict))` → `HashMap<String, Value>` with `#[serde(default)]`.
//!   - `from_dict`/`to_dict` preserve dual-key aliasing (`subscription_id`↔`id`, `change_type`↔`changeType`,
//!     `notification_url`↔`notificationUrl`, `expiration_datetime`↔`expirationDateTime`, etc.) via manual
//!     `Value` lookups, exactly matching Python `payload.get("snake") or payload.get("camel")`.
//!   - `ArtifactType` is a validated `String` (`transcript`|`recording`|`call_record`) with `is_valid_artifact_type`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// ArtifactType — mirrors models.py:10
// ---------------------------------------------------------------------------

pub type ArtifactType = String;

pub const ARTIFACT_TRANSCRIPT: &str = "transcript";
pub const ARTIFACT_RECORDING: &str = "recording";
pub const ARTIFACT_CALL_RECORD: &str = "call_record";

pub fn is_valid_artifact_type(v: &str) -> bool {
    matches!(v, "transcript" | "recording" | "call_record")
}

// ---------------------------------------------------------------------------
// Helpers — mirrors _parse_datetime, _serialize_datetime, _clean_dict
// ---------------------------------------------------------------------------

/// Mirrors `models.py:_parse_datetime` (lines 13-24).
///
/// Python: accepts `None`, `datetime`, or `Any` via `str(value).strip()` then
/// `fromisoformat` with `Z` → `+00:00` handling and naive → `timezone.utc`.
/// Rust: `Value`/`&str` → `Option<String>` normalized trimmed string; `Z` kept,
/// `""` → `None`, no `chrono` parse so validation is string-level. Real port
/// would return `DateTime<Utc>`.
pub fn parse_datetime(value: Option<&Value>) -> Option<String> {
    match value {
        None => None,
        Some(v) if v.is_null() => None,
        Some(v) => {
            if let Some(s) = v.as_str() {
                return parse_datetime_str(Some(s));
            }
            // Python does `str(value).strip()` for non-datetime, non-None
            let s = v.to_string();
            // Strip surrounding quotes if JSON stringified number
            let trimmed = s.trim().trim_matches('"');
            parse_datetime_str(Some(trimmed))
        }
    }
}

/// String overload of `_parse_datetime` — mirrors the core `str(value).strip()` → `fromisoformat` path.
pub fn parse_datetime_str(value: Option<&str>) -> Option<String> {
    let raw = value?;
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    // Mirrors `if text.endswith("Z"): text = f"{text[:-1]}+00:00"` then `fromisoformat`.
    // We keep normalized `Z` form for storage; intermediate `+00:00` is the same instant.
    // If it ended with Z, keep Z. If it ended with +00:00, keep +00:00 (serialize will turn to Z).
    // No heavy validation without chrono — just trim and return.
    let mut normalized = text.to_string();
    if normalized.ends_with('Z') {
        // Already Z — keep as is (Python would parse then serialize back to Z)
        return Some(normalized);
    }
    if normalized.ends_with("+00:00") {
        return Some(normalized);
    }
    // Naive datetime (no tz) → Python `replace(tzinfo=timezone.utc)` → serialize as Z.
    // Keep raw for now; `serialize_datetime` will append Z if needed.
    Some(normalized)
}

/// Mirrors `models.py:_serialize_datetime` (lines 27-31).
///
/// Python: `value.astimezone(timezone.utc).isoformat().replace("+00:00","Z")`.
/// Rust: `Option<String>` → `Option<String>` with `+00:00` → `Z` and naive `T`-without-tz → `Z`.
pub fn serialize_datetime(value: Option<&String>) -> Option<String> {
    let s = value?;
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    // Already Z
    if t.ends_with('Z') {
        return Some(t.to_string());
    }
    // +00:00 → Z
    if t.ends_with("+00:00") {
        let mut out = t[..t.len() - 6].to_string();
        out.push('Z');
        return Some(out);
    }
    // Other offset like +02:00 / -05:00 — keep as is (Python would convert to UTC then Z, but
    // without chrono we preserve; real chrono port would do astimezone(UTC)).
    // Detect any + or - after T; if present, keep original.
    if let Some(t_pos) = t.find('T') {
        let after = &t[t_pos + 1..];
        if after.contains('+') || after[1..].contains('-') {
            // heuristic: contains offset sign after time part
            // Check if there's a + or - in the time suffix
            if after.rfind('+').is_some() || after.rfind('-').is_some() {
                return Some(t.to_string());
            }
        }
        // No offset → naive → treat as UTC → append Z
        let mut out = t.to_string();
        out.push('Z');
        return Some(out);
    }
    // Date-only or other → keep as is, but ensure Z if it looks like datetime
    Some(t.to_string())
}

/// Convenience: `Option<String>` owned overload.
pub fn serialize_datetime_owned(value: &Option<String>) -> Option<String> {
    serialize_datetime(value.as_ref())
}

/// Mirrors `models.py:_clean_dict` (lines 34-35): `{k: v for k,v in values.items() if v is not None}`.
/// Removes `Null` entries from a JSON object.
pub fn clean_dict(values: Map<String, Value>) -> Value {
    let filtered: Map<String, Value> = values
        .into_iter()
        .filter(|(_, v)| !v.is_null())
        .collect();
    Value::Object(filtered)
}

/// Helper: clean a `Value::Object`, otherwise return as-is.
pub fn clean_value(v: Value) -> Value {
    match v {
        Value::Object(m) => clean_dict(m),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Internal alias helpers — mirrors `payload.get("snake") or payload.get("camel")`
// ---------------------------------------------------------------------------

fn get_value_alias<'a>(payload: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    for k in keys {
        if let Some(v) = payload.get(*k) {
            if v.is_null() {
                continue;
            }
            if let Some(s) = v.as_str() {
                if s.is_empty() {
                    continue;
                }
            }
            return Some(v);
        }
    }
    None
}

fn get_string_alias(payload: &Value, keys: &[&str]) -> Option<String> {
    let v = get_value_alias(payload, keys)?;
    if let Some(s) = v.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(trimmed.to_string());
    }
    // Non-string truthy value → to_string
    let s = v.to_string();
    let trimmed = s.trim().trim_matches('"').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn get_string_alias_preserve(payload: &Value, keys: &[&str]) -> Option<String> {
    // For optional fields like organizer_user_id etc. Python does
    // `payload.get("snake") or payload.get("camel")` without extra strip beyond truthiness.
    // We preserve empty-check but return trimmed.
    get_string_alias(payload, keys)
}

fn get_string_alias_raw(payload: &Value, keys: &[&str]) -> Option<String> {
    // Returns first present string even if empty? For required stripped fields we handle strip elsewhere.
    for k in keys {
        if let Some(v) = payload.get(*k) {
            if v.is_null() {
                continue;
            }
            if let Some(s) = v.as_str() {
                return Some(s.to_string());
            }
            // numbers etc.
            return Some(v.to_string().trim_matches('"').to_string());
        }
    }
    None
}

fn get_list_alias(payload: &Value, keys: &[&str]) -> Vec<Value> {
    for k in keys {
        if let Some(v) = payload.get(*k) {
            if let Some(arr) = v.as_array() {
                return arr.clone();
            }
            if v.is_null() {
                continue;
            }
        }
    }
    Vec::new()
}

fn get_map_alias(payload: &Value, keys: &[&str]) -> HashMap<String, Value> {
    for k in keys {
        if let Some(v) = payload.get(*k) {
            if let Some(obj) = v.as_object() {
                return obj.iter().map(|(kk, vv)| (kk.clone(), vv.clone())).collect();
            }
        }
    }
    HashMap::new()
}

fn get_datetime_alias(payload: &Value, keys: &[&str]) -> Option<String> {
    let v = get_value_alias(payload, keys)?;
    parse_datetime(Some(v))
}

// ---------------------------------------------------------------------------
// GraphSubscription — mirrors models.py:38-91
// ---------------------------------------------------------------------------

/// Mirrors `GraphSubscription` (models.py:38-91).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraphSubscription {
    pub subscription_id: String,
    pub resource: String,
    pub change_type: String,
    pub notification_url: String,
    pub expiration_datetime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_state: Option<String>,
    /// Stored as normalized ISO-8601 `Z` string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_renewal_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl GraphSubscription {
    /// Mirrors `__post_init__` validation + datetime normalization.
    pub fn new(
        subscription_id: String,
        resource: String,
        change_type: String,
        notification_url: String,
        expiration_datetime: String,
        client_state: Option<String>,
        latest_renewal_at: Option<String>,
        status: Option<String>,
    ) -> Result<Self, String> {
        if subscription_id.trim().is_empty() {
            return Err("GraphSubscription.subscription_id is required.".to_string());
        }
        if resource.trim().is_empty() {
            return Err("GraphSubscription.resource is required.".to_string());
        }
        if change_type.trim().is_empty() {
            return Err("GraphSubscription.change_type is required.".to_string());
        }
        if notification_url.trim().is_empty() {
            return Err("GraphSubscription.notification_url is required.".to_string());
        }
        let exp = parse_datetime_str(Some(&expiration_datetime))
            .ok_or_else(|| "GraphSubscription.expiration_datetime is required.".to_string())?;
        let latest = latest_renewal_at
            .as_deref()
            .and_then(|s| parse_datetime_str(Some(s)));
        Ok(Self {
            subscription_id: subscription_id.trim().to_string(),
            resource: resource.trim().to_string(),
            change_type: change_type.trim().to_string(),
            notification_url: notification_url.trim().to_string(),
            expiration_datetime: exp,
            client_state,
            latest_renewal_at: latest,
            status,
        })
    }

    /// Mirrors `GraphSubscription.from_dict` (lines 63-77).
    pub fn from_dict(payload: &Value) -> Result<Self, String> {
        let subscription_id = get_string_alias_raw(payload, &["subscription_id", "id"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let resource = get_string_alias_raw(payload, &["resource"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let change_type = get_string_alias_raw(payload, &["change_type", "changeType"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let notification_url = get_string_alias_raw(payload, &["notification_url", "notificationUrl"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let expiration_raw = get_value_alias(payload, &["expiration_datetime", "expirationDateTime"]);
        let latest_raw = get_value_alias(payload, &["latest_renewal_at", "latestRenewalAt"]);
        let client_state = get_string_alias(payload, &["client_state", "clientState"]);
        let status = payload.get("status").and_then(|v| v.as_str()).map(|s| s.to_string());

        // Validation mirrors __post_init__
        if subscription_id.trim().is_empty() {
            return Err("GraphSubscription.subscription_id is required.".to_string());
        }
        if resource.trim().is_empty() {
            return Err("GraphSubscription.resource is required.".to_string());
        }
        if change_type.trim().is_empty() {
            return Err("GraphSubscription.change_type is required.".to_string());
        }
        if notification_url.trim().is_empty() {
            return Err("GraphSubscription.notification_url is required.".to_string());
        }
        let expiration_datetime = parse_datetime(expiration_raw)
            .ok_or_else(|| "GraphSubscription.expiration_datetime is required.".to_string())?;
        let latest_renewal_at = latest_raw.and_then(|v| parse_datetime(Some(v)));

        Ok(Self {
            subscription_id,
            resource,
            change_type,
            notification_url,
            expiration_datetime,
            client_state,
            latest_renewal_at,
            status,
        })
    }

    /// Mirrors `GraphSubscription.to_dict` (lines 79-91).
    pub fn to_dict(&self) -> Value {
        let mut m = Map::new();
        m.insert("subscription_id".to_string(), json!(self.subscription_id));
        m.insert("resource".to_string(), json!(self.resource));
        m.insert("change_type".to_string(), json!(self.change_type));
        m.insert("notification_url".to_string(), json!(self.notification_url));
        m.insert(
            "expiration_datetime".to_string(),
            serialize_datetime(Some(&self.expiration_datetime))
                .map(|s| json!(s))
                .unwrap_or(Value::Null),
        );
        m.insert(
            "client_state".to_string(),
            self.client_state.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "latest_renewal_at".to_string(),
            serialize_datetime(self.latest_renewal_at.as_ref())
                .map(|s| json!(s))
                .unwrap_or(Value::Null),
        );
        m.insert(
            "status".to_string(),
            self.status.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        clean_dict(m)
    }
}

// ---------------------------------------------------------------------------
// TeamsMeetingRef — mirrors models.py:94-131
// ---------------------------------------------------------------------------

/// Mirrors `TeamsMeetingRef` (models.py:94-131).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamsMeetingRef {
    pub meeting_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organizer_user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_web_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl TeamsMeetingRef {
    pub fn new(meeting_id: String) -> Result<Self, String> {
        if meeting_id.trim().is_empty() {
            return Err("TeamsMeetingRef.meeting_id is required.".to_string());
        }
        Ok(Self {
            meeting_id: meeting_id.trim().to_string(),
            organizer_user_id: None,
            join_web_url: None,
            calendar_event_id: None,
            thread_id: None,
            tenant_id: None,
            metadata: HashMap::new(),
        })
    }

    /// Mirrors `TeamsMeetingRef.from_dict` (lines 108-118).
    pub fn from_dict(payload: &Value) -> Result<Self, String> {
        let meeting_id = get_string_alias_raw(payload, &["meeting_id", "id"])
            .unwrap_or_default()
            .trim()
            .to_string();
        if meeting_id.is_empty() {
            return Err("TeamsMeetingRef.meeting_id is required.".to_string());
        }
        let organizer_user_id =
            get_string_alias_preserve(payload, &["organizer_user_id", "organizerUserId"]);
        let join_web_url = get_string_alias_preserve(payload, &["join_web_url", "joinWebUrl"]);
        let calendar_event_id =
            get_string_alias_preserve(payload, &["calendar_event_id", "calendarEventId"]);
        let thread_id = get_string_alias_preserve(payload, &["thread_id", "threadId"]);
        let tenant_id = get_string_alias_preserve(payload, &["tenant_id", "tenantId"]);
        let metadata = get_map_alias(payload, &["metadata"]);
        Ok(Self {
            meeting_id,
            organizer_user_id,
            join_web_url,
            calendar_event_id,
            thread_id,
            tenant_id,
            metadata,
        })
    }

    /// Mirrors `TeamsMeetingRef.to_dict` (lines 120-131).
    pub fn to_dict(&self) -> Value {
        let mut m = Map::new();
        m.insert("meeting_id".to_string(), json!(self.meeting_id));
        m.insert(
            "organizer_user_id".to_string(),
            self.organizer_user_id.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "join_web_url".to_string(),
            self.join_web_url.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "calendar_event_id".to_string(),
            self.calendar_event_id.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "thread_id".to_string(),
            self.thread_id.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "tenant_id".to_string(),
            self.tenant_id.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "metadata".to_string(),
            if self.metadata.is_empty() {
                Value::Null
            } else {
                json!(self.metadata)
            },
        );
        clean_dict(m)
    }
}

// ---------------------------------------------------------------------------
// MeetingArtifact — mirrors models.py:134-194
// ---------------------------------------------------------------------------

/// Mirrors `MeetingArtifact` (models.py:134-194).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeetingArtifact {
    pub artifact_type: String,
    pub artifact_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl MeetingArtifact {
    /// Mirrors `__post_init__` validation (lines 147-157).
    pub fn validate(&self) -> Result<(), String> {
        if !is_valid_artifact_type(&self.artifact_type) {
            return Err(
                "MeetingArtifact.artifact_type must be transcript, recording, or call_record.".to_string(),
            );
        }
        if self.artifact_id.trim().is_empty() {
            return Err("MeetingArtifact.artifact_id is required.".to_string());
        }
        Ok(())
    }

    /// Mirrors `MeetingArtifact.from_dict` (lines 159-178).
    pub fn from_dict(payload: &Value) -> Result<Self, String> {
        let artifact_type = get_string_alias(payload, &["artifact_type", "artifactType"])
            .unwrap_or_default();
        // Validate early for 1:1 error path (Python validates in __post_init__)
        if !is_valid_artifact_type(&artifact_type) {
            return Err(
                "MeetingArtifact.artifact_type must be transcript, recording, or call_record.".to_string(),
            );
        }
        let artifact_id = get_string_alias_raw(payload, &["artifact_id", "id"])
            .unwrap_or_default()
            .trim()
            .to_string();
        if artifact_id.is_empty() {
            return Err("MeetingArtifact.artifact_id is required.".to_string());
        }
        let display_name = get_string_alias(
            payload,
            &["display_name", "displayName", "name"],
        );
        let content_type = get_string_alias(payload, &["content_type", "contentType"]);
        let source_url = get_string_alias(
            payload,
            &["source_url", "sourceUrl", "webUrl"],
        );
        let download_url = get_string_alias(
            payload,
            &[
                "download_url",
                "downloadUrl",
                "@microsoft.graph.downloadUrl",
            ],
        );
        let created_at = get_datetime_alias(payload, &["created_at", "createdDateTime"]);
        let available_at = get_datetime_alias(
            payload,
            &["available_at", "availableDateTime", "lastModifiedDateTime"],
        );
        let size_bytes = {
            let raw = get_value_alias(payload, &["size_bytes", "size"]);
            match raw {
                Some(v) => {
                    if let Some(n) = v.as_i64() {
                        Some(n)
                    } else if let Some(s) = v.as_str() {
                        s.trim().parse::<i64>().ok()
                    } else {
                        // Python: int(value) — try to parse via string
                        let s = v.to_string();
                        s.trim().trim_matches('"').parse::<i64>().ok()
                    }
                }
                None => None,
            }
        };
        let metadata = get_map_alias(payload, &["metadata"]);
        let out = Self {
            artifact_type,
            artifact_id,
            display_name,
            content_type,
            source_url,
            download_url,
            created_at,
            available_at,
            size_bytes,
            metadata,
        };
        out.validate()?;
        Ok(out)
    }

    /// Mirrors `MeetingArtifact.to_dict` (lines 180-194).
    pub fn to_dict(&self) -> Value {
        let mut m = Map::new();
        m.insert("artifact_type".to_string(), json!(self.artifact_type));
        m.insert("artifact_id".to_string(), json!(self.artifact_id));
        m.insert(
            "display_name".to_string(),
            self.display_name.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "content_type".to_string(),
            self.content_type.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "source_url".to_string(),
            self.source_url.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "download_url".to_string(),
            self.download_url.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "created_at".to_string(),
            serialize_datetime(self.created_at.as_ref())
                .map(|s| json!(s))
                .unwrap_or(Value::Null),
        );
        m.insert(
            "available_at".to_string(),
            serialize_datetime(self.available_at.as_ref())
                .map(|s| json!(s))
                .unwrap_or(Value::Null),
        );
        m.insert(
            "size_bytes".to_string(),
            self.size_bytes.map(|n| json!(n)).unwrap_or(Value::Null),
        );
        m.insert(
            "metadata".to_string(),
            if self.metadata.is_empty() {
                Value::Null
            } else {
                json!(self.metadata)
            },
        );
        clean_dict(m)
    }
}

// ---------------------------------------------------------------------------
// TeamsMeetingSummaryPayload — mirrors models.py:197-267
// ---------------------------------------------------------------------------

/// Mirrors `TeamsMeetingSummaryPayload` (models.py:197-267).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamsMeetingSummaryPayload {
    pub meeting_ref: TeamsMeetingRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<String>,
    #[serde(default)]
    pub participants: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub key_decisions: Vec<String>,
    #[serde(default)]
    pub action_items: Vec<String>,
    #[serde(default)]
    pub risks: Vec<String>,
    #[serde(default)]
    pub call_metrics: HashMap<String, Value>,
    #[serde(default)]
    pub source_artifacts: Vec<MeetingArtifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence_notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notion_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linear_target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub teams_target: Option<String>,
}

impl TeamsMeetingSummaryPayload {
    /// Mirrors `__post_init__` datetime normalization (lines 217-219).
    pub fn normalize(&mut self) {
        self.start_time = self
            .start_time
            .as_deref()
            .and_then(|s| parse_datetime_str(Some(s)));
        self.end_time = self
            .end_time
            .as_deref()
            .and_then(|s| parse_datetime_str(Some(s)));
    }

    /// Mirrors `TeamsMeetingSummaryPayload.from_dict` (lines 221-243).
    pub fn from_dict(payload: &Value) -> Result<Self, String> {
        let meeting_ref_payload = payload.get("meeting_ref").ok_or_else(|| "TeamsMeetingSummaryPayload.meeting_ref is required.".to_string())?;
        let meeting_ref = TeamsMeetingRef::from_dict(meeting_ref_payload)?;
        let title = payload.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
        let start_time = get_datetime_alias(payload, &["start_time", "startTime"]);
        let end_time = get_datetime_alias(payload, &["end_time", "endTime"]);
        let participants = {
            let arr = get_list_alias(payload, &["participants"]);
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        };
        let transcript_text =
            get_string_alias(payload, &["transcript_text", "transcriptText"]);
        let summary = payload.get("summary").and_then(|v| v.as_str()).map(|s| s.to_string());
        let key_decisions = {
            let arr = get_list_alias(payload, &["key_decisions", "keyDecisions"]);
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        };
        let action_items = {
            let arr = get_list_alias(payload, &["action_items", "actionItems"]);
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        };
        let risks = {
            let arr = get_list_alias(payload, &["risks"]);
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        };
        let call_metrics = get_map_alias(payload, &["call_metrics", "callMetrics"]);
        let source_artifacts = {
            let raw = payload.get("source_artifacts").and_then(|v| v.as_array());
            match raw {
                Some(arr) => arr
                    .iter()
                    .filter_map(|item| MeetingArtifact::from_dict(item).ok())
                    .collect(),
                None => Vec::new(),
            }
        };
        let confidence = payload.get("confidence").and_then(|v| v.as_str()).map(|s| s.to_string());
        let confidence_notes =
            get_string_alias(payload, &["confidence_notes", "confidenceNotes"]);
        let notion_target = get_string_alias(payload, &["notion_target", "notionTarget"]);
        let linear_target = get_string_alias(payload, &["linear_target", "linearTarget"]);
        let teams_target = get_string_alias(payload, &["teams_target", "teamsTarget"]);
        Ok(Self {
            meeting_ref,
            title,
            start_time,
            end_time,
            participants,
            transcript_text,
            summary,
            key_decisions,
            action_items,
            risks,
            call_metrics,
            source_artifacts,
            confidence,
            confidence_notes,
            notion_target,
            linear_target,
            teams_target,
        })
    }

    /// Mirrors `TeamsMeetingSummaryPayload.to_dict` (lines 245-267).
    pub fn to_dict(&self) -> Value {
        let mut m = Map::new();
        m.insert("meeting_ref".to_string(), self.meeting_ref.to_dict());
        m.insert(
            "title".to_string(),
            self.title.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "start_time".to_string(),
            serialize_datetime(self.start_time.as_ref())
                .map(|s| json!(s))
                .unwrap_or(Value::Null),
        );
        m.insert(
            "end_time".to_string(),
            serialize_datetime(self.end_time.as_ref())
                .map(|s| json!(s))
                .unwrap_or(Value::Null),
        );
        m.insert(
            "participants".to_string(),
            if self.participants.is_empty() {
                Value::Null
            } else {
                json!(self.participants)
            },
        );
        m.insert(
            "transcript_text".to_string(),
            self.transcript_text.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "summary".to_string(),
            self.summary.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "key_decisions".to_string(),
            if self.key_decisions.is_empty() {
                Value::Null
            } else {
                json!(self.key_decisions)
            },
        );
        m.insert(
            "action_items".to_string(),
            if self.action_items.is_empty() {
                Value::Null
            } else {
                json!(self.action_items)
            },
        );
        m.insert(
            "risks".to_string(),
            if self.risks.is_empty() {
                Value::Null
            } else {
                json!(self.risks)
            },
        );
        m.insert(
            "call_metrics".to_string(),
            if self.call_metrics.is_empty() {
                Value::Null
            } else {
                json!(self.call_metrics)
            },
        );
        m.insert(
            "source_artifacts".to_string(),
            if self.source_artifacts.is_empty() {
                Value::Null
            } else {
                json!(self
                    .source_artifacts
                    .iter()
                    .map(|a| a.to_dict())
                    .collect::<Vec<_>>())
            },
        );
        m.insert(
            "confidence".to_string(),
            self.confidence.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "confidence_notes".to_string(),
            self.confidence_notes.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "notion_target".to_string(),
            self.notion_target.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "linear_target".to_string(),
            self.linear_target.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        m.insert(
            "teams_target".to_string(),
            self.teams_target.clone().map(|s| json!(s)).unwrap_or(Value::Null),
        );
        clean_dict(m)
    }
}

// ---------------------------------------------------------------------------
// TeamsMeetingPipelineJob — mirrors models.py:270-340
// ---------------------------------------------------------------------------

/// Mirrors `TeamsMeetingPipelineJob` (models.py:270-340).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamsMeetingPipelineJob {
    pub job_id: String,
    pub event_id: String,
    pub source_event_type: String,
    pub dedupe_key: String,
    pub status: String,
    #[serde(default)]
    pub retry_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meeting_ref: Option<TeamsMeetingRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_artifact_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_payload: Option<TeamsMeetingSummaryPayload>,
    #[serde(default)]
    pub error_info: HashMap<String, Value>,
}

impl TeamsMeetingPipelineJob {
    /// Mirrors `__post_init__` validation (lines 285-298).
    pub fn validate(&self) -> Result<(), String> {
        if self.job_id.trim().is_empty() {
            return Err("TeamsMeetingPipelineJob.job_id is required.".to_string());
        }
        if self.event_id.trim().is_empty() {
            return Err("TeamsMeetingPipelineJob.event_id is required.".to_string());
        }
        if self.source_event_type.trim().is_empty() {
            return Err("TeamsMeetingPipelineJob.source_event_type is required.".to_string());
        }
        if self.dedupe_key.trim().is_empty() {
            return Err("TeamsMeetingPipelineJob.dedupe_key is required.".to_string());
        }
        if self.status.trim().is_empty() {
            return Err("TeamsMeetingPipelineJob.status is required.".to_string());
        }
        Ok(())
    }

    /// Mirrors `TeamsMeetingPipelineJob.from_dict` (lines 300-322).
    pub fn from_dict(payload: &Value) -> Result<Self, String> {
        let job_id = get_string_alias_raw(payload, &["job_id", "jobId"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let event_id = get_string_alias_raw(payload, &["event_id", "eventId"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let source_event_type =
            get_string_alias_raw(payload, &["source_event_type", "sourceEventType"])
                .unwrap_or_default()
                .trim()
                .to_string();
        let dedupe_key = get_string_alias_raw(payload, &["dedupe_key", "dedupeKey"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let status = get_string_alias_raw(payload, &["status"])
            .unwrap_or_default()
            .trim()
            .to_string();
        let retry_count = {
            let raw = get_value_alias(payload, &["retry_count", "retryCount"]);
            match raw {
                Some(v) => {
                    if let Some(n) = v.as_i64() {
                        n
                    } else if let Some(s) = v.as_str() {
                        s.trim().parse::<i64>().unwrap_or(0)
                    } else {
                        let s = v.to_string();
                        s.trim().trim_matches('"').parse::<i64>().unwrap_or(0)
                    }
                }
                None => 0,
            }
        };
        let created_at = get_datetime_alias(payload, &["created_at", "createdAt"]);
        let updated_at = get_datetime_alias(payload, &["updated_at", "updatedAt"]);
        let meeting_ref_payload = get_value_alias(payload, &["meeting_ref", "meetingRef"]);
        let meeting_ref = match meeting_ref_payload {
            Some(v) => Some(TeamsMeetingRef::from_dict(v)?),
            None => None,
        };
        let selected_artifact_strategy = get_string_alias(
            payload,
            &["selected_artifact_strategy", "selectedArtifactStrategy"],
        );
        let summary_payload_raw = get_value_alias(payload, &["summary_payload", "summaryPayload"]);
        let summary_payload = match summary_payload_raw {
            Some(v) => Some(TeamsMeetingSummaryPayload::from_dict(v)?),
            None => None,
        };
        let error_info = get_map_alias(payload, &["error_info", "errorInfo"]);

        let job = Self {
            job_id,
            event_id,
            source_event_type,
            dedupe_key,
            status,
            retry_count,
            created_at,
            updated_at,
            meeting_ref,
            selected_artifact_strategy,
            summary_payload,
            error_info,
        };
        job.validate()?;
        Ok(job)
    }

    /// Mirrors `TeamsMeetingPipelineJob.to_dict` (lines 324-340).
    pub fn to_dict(&self) -> Value {
        let mut m = Map::new();
        m.insert("job_id".to_string(), json!(self.job_id));
        m.insert("event_id".to_string(), json!(self.event_id));
        m.insert("source_event_type".to_string(), json!(self.source_event_type));
        m.insert("dedupe_key".to_string(), json!(self.dedupe_key));
        m.insert("status".to_string(), json!(self.status));
        m.insert("retry_count".to_string(), json!(self.retry_count));
        m.insert(
            "created_at".to_string(),
            serialize_datetime(self.created_at.as_ref())
                .map(|s| json!(s))
                .unwrap_or(Value::Null),
        );
        m.insert(
            "updated_at".to_string(),
            serialize_datetime(self.updated_at.as_ref())
                .map(|s| json!(s))
                .unwrap_or(Value::Null),
        );
        m.insert(
            "meeting_ref".to_string(),
            self.meeting_ref
                .as_ref()
                .map(|r| r.to_dict())
                .unwrap_or(Value::Null),
        );
        m.insert(
            "selected_artifact_strategy".to_string(),
            self.selected_artifact_strategy
                .clone()
                .map(|s| json!(s))
                .unwrap_or(Value::Null),
        );
        m.insert(
            "summary_payload".to_string(),
            self.summary_payload
                .as_ref()
                .map(|p| p.to_dict())
                .unwrap_or(Value::Null),
        );
        m.insert(
            "error_info".to_string(),
            if self.error_info.is_empty() {
                Value::Null
            } else {
                json!(self.error_info)
            },
        );
        clean_dict(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn artifact_type_validation() {
        assert!(is_valid_artifact_type("transcript"));
        assert!(is_valid_artifact_type("recording"));
        assert!(is_valid_artifact_type("call_record"));
        assert!(!is_valid_artifact_type("other"));
    }

    #[test]
    fn graph_subscription_from_dict_aliases() {
        let payload = json!({
            "id": "sub-123",
            "resource": "communications/onlineMeetings/getAllTranscripts",
            "changeType": "created",
            "notificationUrl": "https://example.com/hook",
            "expirationDateTime": "2026-08-27T12:00:00Z"
        });
        let sub = GraphSubscription::from_dict(&payload).unwrap();
        assert_eq!(sub.subscription_id, "sub-123");
        assert_eq!(sub.change_type, "created");
        assert_eq!(sub.expiration_datetime, "2026-08-27T12:00:00Z");
        let dict = sub.to_dict();
        assert_eq!(dict.get("subscription_id").and_then(|v| v.as_str()), Some("sub-123"));
        assert_eq!(dict.get("change_type").and_then(|v| v.as_str()), Some("created"));
    }

    #[test]
    fn graph_subscription_requires_fields() {
        let payload = json!({
            "resource": "r",
            "change_type": "created",
            "notification_url": "https://example.com",
            "expiration_datetime": "2026-08-27T12:00:00Z"
        });
        assert!(GraphSubscription::from_dict(&payload).is_err());
    }

    #[test]
    fn teams_meeting_ref_aliases() {
        let payload = json!({"id": "meet-1", "organizerUserId": "user-1", "joinWebUrl": "https://teams.microsoft.com"});
        let r = TeamsMeetingRef::from_dict(&payload).unwrap();
        assert_eq!(r.meeting_id, "meet-1");
        assert_eq!(r.organizer_user_id.as_deref(), Some("user-1"));
        assert_eq!(r.join_web_url.as_deref(), Some("https://teams.microsoft.com"));
    }

    #[test]
    fn meeting_artifact_validation() {
        let payload = json!({"artifact_type": "transcript", "id": "a1", "displayName": "t.vtt"});
        let a = MeetingArtifact::from_dict(&payload).unwrap();
        assert_eq!(a.artifact_type, "transcript");
        assert_eq!(a.display_name.as_deref(), Some("t.vtt"));
        let bad = json!({"artifact_type": "bad", "id": "a1"});
        assert!(MeetingArtifact::from_dict(&bad).is_err());
    }

    #[test]
    fn pipeline_job_roundtrip() {
        let payload = json!({
            "job_id": "j1",
            "event_id": "e1",
            "source_event_type": "transcript.created",
            "dedupe_key": "k1",
            "status": "pending",
            "meeting_ref": {"meeting_id": "m1"},
            "retry_count": 2
        });
        let job = TeamsMeetingPipelineJob::from_dict(&payload).unwrap();
        assert_eq!(job.job_id, "j1");
        assert_eq!(job.retry_count, 2);
        let dict = job.to_dict();
        assert_eq!(dict.get("job_id").and_then(|v| v.as_str()), Some("j1"));
        assert_eq!(dict.get("retry_count").and_then(|v| v.as_i64()), Some(2));
    }

    #[test]
    fn datetime_normalization() {
        assert_eq!(parse_datetime_str(Some("2026-08-27T12:00:00Z")).as_deref(), Some("2026-08-27T12:00:00Z"));
        assert_eq!(parse_datetime_str(Some("  ")), None);
        assert_eq!(serialize_datetime(Some(&"2026-08-27T12:00:00+00:00".to_string())).as_deref(), Some("2026-08-27T12:00:00Z"));
    }
}
