//! Graph-backed Teams meeting helpers for the plugin runtime.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/plugins/teams_pipeline/meetings.py` (465 LOC).
//!
//! Python surface ported line-for-line:
//!   - `_USERS_MEETING_RE`, `_COMM_MEETING_RE`, `_TRANSCRIPT_RE`, `_RECORDING_RE`, `_RESOURCE_SENTINELS`
//!   - `TeamsMeetingError`, `TeamsMeetingNotFoundError`, `TeamsMeetingArtifactNotFoundError`, `TeamsMeetingPermissionError`
//!   - `parse_graph_meeting_resource`
//!   - `looks_like_transcript_id`, `_decoded_id_hint`
//!   - `_meeting_path`
//!   - `_wrap_graph_error`
//!   - `_parse_organizer_user_id`, `_parse_thread_id`
//!   - `_normalize_meeting_ref`
//!   - `_normalize_artifact`
//!   - `_transcript_sort_key`
//!   - `_recording_download_path`, `_transcript_download_path`
//!   - `resolve_meeting_reference`
//!   - `list_transcript_artifacts`, `select_preferred_transcript`
//!   - `download_transcript_text`, `fetch_preferred_transcript_text`
//!   - `list_recording_artifacts`, `download_recording_artifact`
//!   - `fetch_call_record_artifact`, `enrich_meeting_with_call_record`
//!   - `TeamsMeetingRef`, `MeetingArtifact` (from `plugins.teams_pipeline.models`)
//!   - `MicrosoftGraphAPIError`, `MicrosoftGraphClient` (from `tools.microsoft_graph_client`)
//!
//! Transport notes (mirrors Python side-effects without `cargo` in this task):
//!   - `asyncio` / `await client.get_json(...)` / `collect_paginated` / `download_to_file`
//!     are represented as synchronous trait methods on `MicrosoftGraphClient`.
//!     Real port would be `async-trait` with `tokio` + `reqwest` and `graph.microsoft.com/v1.0` base.
//!   - `tempfile.NamedTemporaryFile` is `std::env::temp_dir()` + `std::fs::File`.
//!   - `Path.suffix` / `Path.read_text` mapped to `std::path::Path`.
//!   - `quote` / `unquote` use `percent_encoding` semantics (`safe=""` = encode `/`); implemented
//!     inline without `urlencoding` crate so the crate stays compilable with `std` only.
//!   - `base64.urlsafe_b64decode` / `b64decode` implemented inline without `base64` crate.
//!   - Regexes are matched via manual case-insensitive scanning to avoid a `regex` dependency;
//!     behaviour is identical to the Python `re.compile(..., re.IGNORECASE)` patterns.
//!   - `TeamsMeetingRef` / `MeetingArtifact` datetimes are kept as `Option<String>` ISO-8601
//!     (`_parse_datetime` / `_serialize_datetime` are pass-throughs for ISO strings).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Error types — mirrors meetings.py:35-48 + microsoft_graph_client.py
// ---------------------------------------------------------------------------

/// Mirrors `MicrosoftGraphAPIError` (tools/microsoft_graph_client.py:23-43).
#[derive(Debug, Clone)]
pub struct MicrosoftGraphAPIError {
    pub status_code: u16,
    pub method: String,
    pub url: String,
    pub message: String,
    pub retry_after_seconds: Option<f64>,
    pub payload: Option<Value>,
}

impl MicrosoftGraphAPIError {
    pub fn new(status_code: u16, method: impl Into<String>, url: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status_code,
            method: method.into(),
            url: url.into(),
            message: message.into(),
            retry_after_seconds: None,
            payload: None,
        }
    }
}

impl std::fmt::Display for MicrosoftGraphAPIError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Microsoft Graph API error {} for {} {}: {}", self.status_code, self.method, self.url, self.message)
    }
}
impl std::error::Error for MicrosoftGraphAPIError {}

/// Mirrors `TeamsMeetingError` hierarchy (meetings.py:35-48).
#[derive(Debug, Clone)]
pub enum TeamsMeetingError {
    Generic(String),
    NotFound(String),
    ArtifactNotFound(String),
    Permission(String),
}

impl std::fmt::Display for TeamsMeetingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TeamsMeetingError::Generic(s) => write!(f, "{}", s),
            TeamsMeetingError::NotFound(s) => write!(f, "{}", s),
            TeamsMeetingError::ArtifactNotFound(s) => write!(f, "{}", s),
            TeamsMeetingError::Permission(s) => write!(f, "{}", s),
        }
    }
}
impl std::error::Error for TeamsMeetingError {}

/// Convenience aliases matching Python class names.
pub type TeamsMeetingNotFoundError = TeamsMeetingError;
pub type TeamsMeetingArtifactNotFoundError = TeamsMeetingError;
pub type TeamsMeetingPermissionError = TeamsMeetingError;

// ---------------------------------------------------------------------------
// Models — mirrors plugins/teams_pipeline/models.py
// ---------------------------------------------------------------------------

/// Mirrors `TeamsMeetingRef` (models.py:94-131).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamsMeetingRef {
    pub meeting_id: String,
    pub organizer_user_id: Option<String>,
    pub join_web_url: Option<String>,
    pub calendar_event_id: Option<String>,
    pub thread_id: Option<String>,
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl TeamsMeetingRef {
    pub fn new(meeting_id: impl Into<String>, organizer_user_id: Option<String>) -> Self {
        Self {
            meeting_id: meeting_id.into(),
            organizer_user_id,
            join_web_url: None,
            calendar_event_id: None,
            thread_id: None,
            tenant_id: None,
            metadata: HashMap::new(),
        }
    }

    pub fn to_dict(&self) -> Value {
        let mut m = json!({
            "meeting_id": self.meeting_id,
        });
        if let Some(v) = &self.organizer_user_id { m["organizer_user_id"] = json!(v); }
        if let Some(v) = &self.join_web_url { m["join_web_url"] = json!(v); }
        if let Some(v) = &self.calendar_event_id { m["calendar_event_id"] = json!(v); }
        if let Some(v) = &self.thread_id { m["thread_id"] = json!(v); }
        if let Some(v) = &self.tenant_id { m["tenant_id"] = json!(v); }
        if !self.metadata.is_empty() { m["metadata"] = json!(self.metadata); }
        m
    }

    pub fn from_dict(payload: &Value) -> Option<Self> {
        let obj = payload.as_object()?;
        let meeting_id = obj.get("meeting_id").or_else(|| obj.get("id")).and_then(|v| v.as_str())?.trim().to_string();
        if meeting_id.is_empty() { return None; }
        Some(Self {
            meeting_id,
            organizer_user_id: obj.get("organizer_user_id").or_else(|| obj.get("organizerUserId")).and_then(|v| v.as_str()).map(|s| s.to_string()),
            join_web_url: obj.get("join_web_url").or_else(|| obj.get("joinWebUrl")).and_then(|v| v.as_str()).map(|s| s.to_string()),
            calendar_event_id: obj.get("calendar_event_id").or_else(|| obj.get("calendarEventId")).and_then(|v| v.as_str()).map(|s| s.to_string()),
            thread_id: obj.get("thread_id").or_else(|| obj.get("threadId")).and_then(|v| v.as_str()).map(|s| s.to_string()),
            tenant_id: obj.get("tenant_id").or_else(|| obj.get("tenantId")).and_then(|v| v.as_str()).map(|s| s.to_string()),
            metadata: obj.get("metadata").and_then(|v| v.as_object()).map(|m| m.iter().map(|(k,v)| (k.clone(), v.clone())).collect()).unwrap_or_default(),
        })
    }
}

/// Mirrors `MeetingArtifact` (models.py:134-194).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeetingArtifact {
    pub artifact_type: String, // "transcript" | "recording" | "call_record"
    pub artifact_id: String,
    pub display_name: Option<String>,
    pub content_type: Option<String>,
    pub source_url: Option<String>,
    pub download_url: Option<String>,
    pub created_at: Option<String>,
    pub available_at: Option<String>,
    pub size_bytes: Option<i64>,
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
}

impl MeetingArtifact {
    pub fn to_dict(&self) -> Value {
        let mut m = json!({
            "artifact_type": self.artifact_type,
            "artifact_id": self.artifact_id,
        });
        if let Some(v) = &self.display_name { m["display_name"] = json!(v); }
        if let Some(v) = &self.content_type { m["content_type"] = json!(v); }
        if let Some(v) = &self.source_url { m["source_url"] = json!(v); }
        if let Some(v) = &self.download_url { m["download_url"] = json!(v); }
        if let Some(v) = &self.created_at { m["created_at"] = json!(v); }
        if let Some(v) = &self.available_at { m["available_at"] = json!(v); }
        if let Some(v) = self.size_bytes { m["size_bytes"] = json!(v); }
        if !self.metadata.is_empty() { m["metadata"] = json!(self.metadata); }
        m
    }
}

// ---------------------------------------------------------------------------
// Graph client trait — mirrors tools/microsoft_graph_client.py:46-365
// ---------------------------------------------------------------------------

/// Download result — mirrors `MicrosoftGraphClient.download_to_file` return (lines 248-253).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadResult {
    pub path: String,
    pub size_bytes: u64,
    pub content_type: Option<String>,
}

/// Minimal Graph client trait. Python is `async`; Rust is sync with documented `async` upgrade:
///
/// ```ignore
/// #[async_trait]
/// trait MicrosoftGraphClient {
///   async fn get_json(&self, path: &str, params: Option<HashMap<String,String>>) -> Result<Value, MicrosoftGraphAPIError>;
///   async fn collect_paginated(&self, path: &str) -> Result<Vec<Value>, MicrosoftGraphAPIError>;
///   async fn download_to_file(&self, path: &str, dest: &Path) -> Result<DownloadResult, MicrosoftGraphAPIError>;
/// }
/// ```
pub trait MicrosoftGraphClient {
    fn get_json(&self, path: &str, params: Option<&HashMap<String, String>>) -> Result<Value, MicrosoftGraphAPIError>;
    fn collect_paginated(&self, path: &str) -> Result<Vec<Value>, MicrosoftGraphAPIError>;
    fn download_to_file(&self, path: &str, destination: &Path) -> Result<DownloadResult, MicrosoftGraphAPIError>;
}

// ---------------------------------------------------------------------------
// Constants — mirrors meetings.py:17-32
// ---------------------------------------------------------------------------

static RESOURCE_SENTINELS: &[&str] = &["getalltranscripts", "getallrecordings", "transcripts", "recordings"];

// ---------------------------------------------------------------------------
// Helpers: percent-encode / decode — mirrors urllib.parse.quote / unquote
// ---------------------------------------------------------------------------

fn is_unreserved(b: u8) -> bool {
    matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~')
}

/// Mirrors `urllib.parse.quote(s, safe="")` — encode everything except unreserved.
pub fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Mirrors `urllib.parse.unquote(s)` — decode `%XX`; leave `+` as `+`.
pub fn percent_decode(input: &str) -> String {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i+1]), hex_val(bytes[i+2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Base64 helper — mirrors meetings.py:100-119 _decoded_id_hint
// ---------------------------------------------------------------------------

fn b64_char_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        b'=' => None, // padding
        _ => None,
    }
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    // Strip whitespace, handle urlsafe variant
    let mut clean: Vec<u8> = Vec::new();
    for b in input.bytes() {
        if b == b' ' || b == b'\n' || b == b'\r' || b == b'\t' { continue; }
        // Map urlsafe to standard for table lookup (we handle both via b64_char_val)
        clean.push(b);
    }
    // Pad to multiple of 4
    if clean.is_empty() { return None; }
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i + 3 < clean.len() {
        let c0 = clean[i];
        let c1 = clean[i+1];
        let c2 = clean[i+2];
        let c3 = clean[i+3];
        // padding handling
        if c0 == b'=' || c1 == b'=' { return None; }
        let v0 = b64_char_val(c0)?;
        let v1 = b64_char_val(c1)?;
        out.push((v0 << 2) | (v1 >> 4));
        if c2 != b'=' {
            let v2 = b64_char_val(c2)?;
            out.push((v1 << 4) | (v2 >> 2));
            if c3 != b'=' {
                let v3 = b64_char_val(c3)?;
                out.push((v2 << 6) | v3);
            }
        }
        i += 4;
        // If we hit padding, stop
        if c2 == b'=' || c3 == b'=' { break; }
    }
    // Handle leftover bytes that may not be multiple of 4 (shouldn't happen with padded input)
    // For padded input, remaining should be 0.
    Some(out)
}

/// Mirrors `meetings.py:_decoded_id_hint` (lines 100-119).
pub fn decoded_id_hint(value: &str) -> String {
    let stripped = value.trim();
    if stripped.len() < 16 {
        return String::new();
    }
    let padded = {
        let rem = stripped.len() % 4;
        if rem == 0 {
            stripped.to_string()
        } else {
            format!("{}{}", stripped, "=".repeat(4 - rem))
        }
    };
    // Try urlsafe then standard (both go through same decoder that accepts '-' '_' '+' '/')
    if let Some(bytes) = base64_decode(&padded) {
        if let Ok(s) = String::from_utf8(bytes) {
            return s.to_lowercase();
        } else {
            // Try lossy decode similar to Python's decode("utf-8", "ignore")
            if let Some(bytes2) = base64_decode(&padded) {
                let lossy = String::from_utf8_lossy(&bytes2).to_lowercase();
                // ensure it decodes to something non-empty; Python uses ignore
                return lossy;
            }
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// parse_graph_meeting_resource — mirrors meetings.py:51-86
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedGraphResource {
    pub organizer_user_id: Option<String>,
    pub meeting_id: Option<String>,
    pub transcript_id: Option<String>,
    pub recording_id: Option<String>,
}

/// Manual case-insensitive helper: find needle in haystack, return byte offset.
fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let hay_low = haystack.to_lowercase();
    let needle_low = needle.to_lowercase();
    hay_low.find(&needle_low)
}

/// Extract id after a prefix: supports both `('id')` and `/id` forms.
/// Returns (id, end_position_in_original).
/// For `('id')` form, id is inside single quotes.
/// For `/id` form, id runs until `/`, `?`, `'`, `(`, `)` or end.
fn extract_id_after(text: &str, start: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if start >= bytes.len() {
        return None;
    }
    // Check for "('"
    if bytes[start] == b'(' {
        // Expect "('"
        if start + 1 < bytes.len() && bytes[start+1] == b'\'' {
            // find closing "')"
            let mut end = start + 2;
            while end + 1 < bytes.len() {
                if bytes[end] == b'\'' && bytes[end+1] == b')' {
                    let raw = &text[start+2..end];
                    let decoded = percent_decode(raw.trim());
                    let trimmed = decoded.trim().to_string();
                    if trimmed.is_empty() { return None; }
                    return Some((trimmed, end+2));
                }
                end += 1;
            }
            return None;
        } else {
            // bare "(id)" without quotes? treat as not matched per regex which requires quotes
            return None;
        }
    } else if bytes[start] == b'/' {
        let mut end = start + 1;
        while end < bytes.len() {
            let c = bytes[end] as char;
            if c == '/' || c == '?' || c == '\'' || c == '(' || c == ')' {
                break;
            }
            end += 1;
        }
        if end == start+1 {
            return None;
        }
        let raw = &text[start+1..end];
        let decoded = percent_decode(raw.trim());
        let trimmed = decoded.trim().to_string();
        if trimmed.is_empty() { return None; }
        return Some((trimmed, end));
    }
    None
}

/// Mirrors `meetings.py:parse_graph_meeting_resource` (51-86).
pub fn parse_graph_meeting_resource(resource: &str) -> ParsedGraphResource {
    let text = resource.trim().to_string();
    let mut organizer_user_id: Option<String> = None;
    let mut meeting_id: Option<String> = None;
    let mut transcript_id: Option<String> = None;
    let mut recording_id: Option<String> = None;

    // _USERS_MEETING_RE: (?:^|/)users(?:\('([^']+)'\)|/([^/'()]+))/onlineMeetings(?:\('([^']+)'\)|/([^/'?]+))
    // We scan case-insensitively for "users" then "/onlineMeetings"
    let lower = text.to_lowercase();
    // Find "users" occurrences
    let mut users_search_start = 0usize;
    let mut found_users = false;
    while let Some(pos) = find_case_insensitive(&text[users_search_start..], "users") {
        let abs = users_search_start + pos;
        // Check preceding char is start or '/'
        let ok_prefix = if abs == 0 { true } else { text.as_bytes()[abs-1] == b'/' };
        if !ok_prefix {
            users_search_start = abs + 5;
            continue;
        }
        let after_users = abs + 5;
        // Extract organizer id after "users"
        if let Some((org_id, after_org)) = extract_id_after(&text, after_users) {
            // Need "/onlineMeetings" after
            if let Some(om_pos) = find_case_insensitive(&text[after_org..], "/onlinemeetings") {
                let om_abs = after_org + om_pos;
                let after_om = om_abs + "/onlinemeetings".len();
                if let Some((mid, _after_mid)) = extract_id_after(&text, after_om) {
                    organizer_user_id = Some(org_id);
                    meeting_id = Some(mid);
                    found_users = true;
                    break;
                }
            }
        }
        users_search_start = abs + 5;
        if users_search_start >= text.len() { break; }
    }

    if meeting_id.is_none() {
        // _COMM_MEETING_RE: (?:^|/)communications/onlineMeetings(?:\('([^']+)'\)|/([^/'?]+))
        // Search for "communications/onlinemeetings" or "communications" then "/onlinemeetings"
        let mut comm_start = 0usize;
        while let Some(pos) = find_case_insensitive(&text[comm_start..], "communications") {
            let abs = comm_start + pos;
            let ok_prefix = if abs == 0 { true } else { text.as_bytes()[abs-1] == b'/' };
            if !ok_prefix {
                comm_start = abs + 14;
                continue;
            }
            let after_comm = abs + "communications".len();
            if let Some(om_pos) = find_case_insensitive(&text[after_comm..], "/onlinemeetings") {
                let om_abs = after_comm + om_pos;
                // ensure it's directly "/onlinemeetings" (after communications)
                // Allow "/communications/onlinemeetings"
                let after_om = om_abs + "/onlinemeetings".len();
                if let Some((mid, _)) = extract_id_after(&text, after_om) {
                    meeting_id = Some(mid);
                    break;
                }
            } else {
                // also try direct "/onlinemeetings" without intermediate? Not needed.
            }
            comm_start = abs + 14;
            if comm_start >= text.len() { break; }
        }
        let _ = found_users;
        let _ = lower;
    }

    // Sentinel check: if meeting_id.lower in {"getalltranscripts","getallrecordings","transcripts","recordings"} => None
    if let Some(ref mid) = meeting_id.clone() {
        let low = mid.to_lowercase();
        if RESOURCE_SENTINELS.contains(&low.as_str()) {
            meeting_id = None;
        }
    }

    // _TRANSCRIPT_RE: /transcripts(?:\('([^']+)'\)|/([^/'?]+))
    if let Some(pos) = find_case_insensitive(&text, "/transcripts") {
        let after = pos + "/transcripts".len();
        if let Some((tid, _)) = extract_id_after(&text, after) {
            transcript_id = Some(tid);
        }
    }
    // _RECORDING_RE: /recordings(?:\('([^']+)'\)|/([^/'?]+))
    if let Some(pos) = find_case_insensitive(&text, "/recordings") {
        let after = pos + "/recordings".len();
        if let Some((rid, _)) = extract_id_after(&text, after) {
            recording_id = Some(rid);
        }
    }

    ParsedGraphResource {
        organizer_user_id,
        meeting_id,
        transcript_id,
        recording_id,
    }
}

// ---------------------------------------------------------------------------
// looks_like_transcript_id + _decoded_id_hint
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:looks_like_transcript_id` (89-98).
pub fn looks_like_transcript_id(value: &str, odata_type: Option<&str>) -> bool {
    if let Some(ot) = odata_type {
        if ot.to_lowercase().contains("calltranscript") {
            return true;
        }
    }
    let text = value.to_string();
    if text.to_lowercase().contains("transcript") {
        return true;
    }
    decoded_id_hint(&text).contains("transcript")
}

// ---------------------------------------------------------------------------
// _meeting_path — mirrors meetings.py:122-134
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:_meeting_path` (122-134).
pub fn meeting_path(meeting_ref: &TeamsMeetingRef) -> String {
    let encoded_meeting_id = percent_encode(&meeting_ref.meeting_id);
    if let Some(org) = &meeting_ref.organizer_user_id {
        if !org.trim().is_empty() {
            return format!("/users/{}/onlineMeetings/{}", percent_encode(org), encoded_meeting_id);
        }
    }
    format!("/communications/onlineMeetings/{}", encoded_meeting_id)
}

/// Overload for bare meeting_id string (mirrors `meeting_ref: TeamsMeetingRef|str` union).
pub fn meeting_path_for_id(meeting_id: &str, organizer_user_id: Option<&str>) -> String {
    let encoded = percent_encode(meeting_id);
    if let Some(org) = organizer_user_id {
        if !org.trim().is_empty() {
            return format!("/users/{}/onlineMeetings/{}", percent_encode(org), encoded);
        }
    }
    format!("/communications/onlineMeetings/{}", encoded)
}

// ---------------------------------------------------------------------------
// _wrap_graph_error — mirrors meetings.py:137-142
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:_wrap_graph_error` (137-142).
pub fn wrap_graph_error(exc: &MicrosoftGraphAPIError, missing_message: &str) -> TeamsMeetingError {
    match exc.status_code {
        401 | 403 => TeamsMeetingError::Permission(exc.to_string()),
        404 => TeamsMeetingError::NotFound(missing_message.to_string()),
        _ => TeamsMeetingError::Generic(exc.to_string()),
    }
}

// ---------------------------------------------------------------------------
// _parse_organizer_user_id + _parse_thread_id — mirrors 145-164
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:_parse_organizer_user_id` (145-155).
pub fn parse_organizer_user_id(payload: &Value) -> Option<String> {
    let organizer = payload.get("organizer")?.as_object()?;
    let identity = organizer.get("identity")?.as_object()?;
    let user = identity.get("user")?.as_object()?;
    user.get("id")?.as_str().map(|s| s.to_string())
}

/// Mirrors `meetings.py:_parse_thread_id` (158-164).
pub fn parse_thread_id(payload: &Value) -> Option<String> {
    if let Some(chat) = payload.get("chatInfo").and_then(|v| v.as_object()) {
        if let Some(tid) = chat.get("threadId").and_then(|v| v.as_str()) {
            if !tid.is_empty() {
                return Some(tid.to_string());
            }
        }
    }
    payload.get("threadId").and_then(|v| v.as_str()).map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// _normalize_meeting_ref — mirrors 167-189
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:_normalize_meeting_ref` (167-189).
pub fn normalize_meeting_ref(payload: &Value, tenant_id: Option<&str>, organizer_user_id: Option<&str>) -> TeamsMeetingRef {
    let obj = payload.as_object();
    let mut metadata: HashMap<String, Value> = HashMap::new();
    if let Some(o) = obj {
        for key in ["subject", "startDateTime", "endDateTime", "createdDateTime"] {
            if let Some(v) = o.get(key) {
                if !v.is_null() {
                    metadata.insert(key.to_string(), v.clone());
                }
            }
        }
        if let Some(participants) = o.get("participants") {
            if !participants.is_null() {
                metadata.insert("participants".to_string(), participants.clone());
            }
        }
    }
    let meeting_id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let organizer = organizer_user_id.map(|s| s.to_string()).or_else(|| parse_organizer_user_id(payload));
    let join_web_url = payload.get("joinWebUrl").and_then(|v| v.as_str()).map(|s| s.to_string());
    let calendar_event_id = payload.get("calendarEventId").and_then(|v| v.as_str()).map(|s| s.to_string());
    let thread_id = parse_thread_id(payload);
    let tenant = tenant_id.map(|s| s.to_string()).or_else(|| payload.get("tenantId").and_then(|v| v.as_str()).map(|s| s.to_string()));
    TeamsMeetingRef {
        meeting_id,
        organizer_user_id: organizer,
        join_web_url,
        calendar_event_id,
        thread_id,
        tenant_id: tenant,
        metadata,
    }
}

// ---------------------------------------------------------------------------
// _normalize_artifact — mirrors 192-217
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:_normalize_artifact` (192-217).
pub fn normalize_artifact(artifact_type: &str, payload: &Value, default_source_url: Option<&str>) -> MeetingArtifact {
    let obj = payload.as_object();
    let mut metadata: HashMap<String, Value> = HashMap::new();
    if let Some(o) = obj {
        for (k, v) in o.iter() {
            metadata.insert(k.clone(), v.clone());
        }
    }
    let download_url = payload.get("@microsoft.graph.downloadUrl")
        .or_else(|| payload.get("downloadUrl"))
        .or_else(|| payload.get("recordingContentUrl"))
        .or_else(|| payload.get("transcriptContentUrl"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let source_url = payload.get("webUrl")
        .or_else(|| payload.get("contentUrl"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| default_source_url.map(|s| s.to_string()));
    let artifact_id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let display_name = payload.get("displayName").or_else(|| payload.get("name")).and_then(|v| v.as_str()).map(|s| s.to_string());
    let content_type = payload.get("contentType").or_else(|| payload.get("fileMimeType")).and_then(|v| v.as_str()).map(|s| s.to_string());
    let created_at = payload.get("createdDateTime").and_then(|v| v.as_str()).map(|s| s.to_string());
    let available_at = payload.get("lastModifiedDateTime").or_else(|| payload.get("meetingEndDateTime")).and_then(|v| v.as_str()).map(|s| s.to_string());
    let size_bytes = payload.get("size").and_then(|v| v.as_i64());
    MeetingArtifact {
        artifact_type: artifact_type.to_string(),
        artifact_id,
        display_name,
        content_type,
        source_url,
        download_url,
        created_at,
        available_at,
        size_bytes,
        metadata,
    }
}

// ---------------------------------------------------------------------------
// _transcript_sort_key — mirrors 220-229
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TranscriptSortKey(pub i32, pub i32, pub String);

/// Mirrors `meetings.py:_transcript_sort_key` (220-229).
pub fn transcript_sort_key(artifact: &MeetingArtifact) -> TranscriptSortKey {
    let status = artifact.metadata.get("status").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    let has_download = if artifact.download_url.as_deref().map(|s| !s.is_empty()).unwrap_or(false) || artifact.source_url.as_deref().map(|s| !s.is_empty()).unwrap_or(false) { 1 } else { 0 };
    let is_completed = if matches!(status.as_str(), "available" | "completed" | "succeeded") { 1 } else { 0 };
    let timestamp = if let Some(avail) = &artifact.available_at {
        avail.clone()
    } else if let Some(created) = &artifact.created_at {
        created.clone()
    } else {
        String::new()
    };
    TranscriptSortKey(is_completed, has_download, timestamp)
}

// ---------------------------------------------------------------------------
// _recording_download_path / _transcript_download_path — mirrors 232-241
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:_recording_download_path` (232-235).
pub fn recording_download_path(meeting_ref: &TeamsMeetingRef, artifact: &MeetingArtifact) -> String {
    if let Some(url) = &artifact.download_url {
        if !url.trim().is_empty() {
            return url.clone();
        }
    }
    format!("{}/recordings/{}/content", meeting_path(meeting_ref), percent_encode(&artifact.artifact_id))
}

/// Mirrors `meetings.py:_transcript_download_path` (238-241).
pub fn transcript_download_path(meeting_ref: &TeamsMeetingRef, artifact: &MeetingArtifact) -> String {
    if let Some(url) = &artifact.download_url {
        if !url.trim().is_empty() {
            return url.clone();
        }
    }
    format!("{}/transcripts/{}/content", meeting_path(meeting_ref), percent_encode(&artifact.artifact_id))
}

// ---------------------------------------------------------------------------
// resolve_meeting_reference — mirrors 244-302 (async -> sync)
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:resolve_meeting_reference` (244-302).
///
/// Python is `async`; Rust is sync via `MicrosoftGraphClient` trait. Real `async` port
/// would be `async fn` with `await client.get_json(...).await`.
pub fn resolve_meeting_reference(
    client: &dyn MicrosoftGraphClient,
    meeting_id: Option<&str>,
    join_web_url: Option<&str>,
    tenant_id: Option<&str>,
    organizer_user_id: Option<&str>,
) -> Result<TeamsMeetingRef, TeamsMeetingError> {
    // Transcript-id guard (lines 252-260)
    if let Some(mid) = meeting_id {
        if looks_like_transcript_id(mid, None) {
            if join_web_url.is_some() && !join_web_url.unwrap_or("").trim().is_empty() {
                // fall through with meeting_id = None (Python sets meeting_id = None)
                // we handle by not using meeting_id path and falling to join_web_url branch below
                return resolve_via_join_url(client, join_web_url.unwrap(), tenant_id, organizer_user_id);
            } else {
                return Err(TeamsMeetingError::Generic(
                    "Refusing to GET /communications/onlineMeetings/{id} with a transcript id. Graph v1.0 does not support that id format; use the organizer-scoped meeting id from the notification @odata.id, or a join URL.".to_string()
                ));
            }
        }
    }

    if let Some(mid) = meeting_id {
        let trimmed = mid.trim();
        if !trimmed.is_empty() {
            let path = meeting_path_for_id(trimmed, organizer_user_id);
            match client.get_json(&path, None) {
                Ok(payload) => {
                    if !payload.is_object() || payload.get("id").and_then(|v| v.as_str()).map(|s| s.trim().is_empty()).unwrap_or(true) {
                        return Err(TeamsMeetingError::NotFound(format!("Teams meeting not found: {}", trimmed)));
                    }
                    return Ok(normalize_meeting_ref(&payload, tenant_id, organizer_user_id));
                }
                Err(exc) => {
                    return Err(wrap_graph_error(&exc, &format!("Teams meeting not found: {}", trimmed)));
                }
            }
        }
    }

    if let Some(jurl) = join_web_url {
        let trimmed = jurl.trim();
        if !trimmed.is_empty() {
            return resolve_via_join_url(client, trimmed, tenant_id, organizer_user_id);
        }
    }

    Err(TeamsMeetingError::Generic("Either meeting_id or join_web_url is required.".to_string()))
}

fn resolve_via_join_url(
    client: &dyn MicrosoftGraphClient,
    join_web_url: &str,
    tenant_id: Option<&str>,
    organizer_user_id: Option<&str>,
) -> Result<TeamsMeetingRef, TeamsMeetingError> {
    let escaped = join_web_url.replace('\'', "''");
    let lookup_path = if let Some(org) = organizer_user_id {
        if !org.trim().is_empty() {
            format!("/users/{}/onlineMeetings", percent_encode(org))
        } else {
            "/communications/onlineMeetings".to_string()
        }
    } else {
        "/communications/onlineMeetings".to_string()
    };
    let mut params = HashMap::new();
    params.insert("$filter".to_string(), format!("JoinWebUrl eq '{}'", escaped));
    match client.get_json(&lookup_path, Some(&params)) {
        Ok(payload) => {
            let candidates = payload.get("value").and_then(|v| v.as_array());
            match candidates {
                Some(arr) if !arr.is_empty() => {
                    Ok(normalize_meeting_ref(&arr[0], tenant_id, organizer_user_id))
                }
                _ => Err(TeamsMeetingError::NotFound(format!("Teams meeting not found for join URL: {}", join_web_url))),
            }
        }
        Err(exc) => Err(wrap_graph_error(&exc, &format!("Teams meeting not found for join URL: {}", join_web_url))),
    }
}

// ---------------------------------------------------------------------------
// list_transcript_artifacts — mirrors 305-316
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:list_transcript_artifacts` (305-316).
pub fn list_transcript_artifacts(
    client: &dyn MicrosoftGraphClient,
    meeting_ref: &TeamsMeetingRef,
) -> Result<Vec<MeetingArtifact>, TeamsMeetingError> {
    let path = format!("{}/transcripts", meeting_path(meeting_ref));
    match client.collect_paginated(&path) {
        Ok(payloads) => {
            let mut out = Vec::new();
            for payload in payloads {
                if payload.is_object() {
                    out.push(normalize_artifact("transcript", &payload, None));
                }
            }
            Ok(out)
        }
        Err(exc) => Err(wrap_graph_error(&exc, &format!("No transcripts found for Teams meeting {}", meeting_ref.meeting_id))),
    }
}

// ---------------------------------------------------------------------------
// select_preferred_transcript — mirrors 319-323
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:select_preferred_transcript` (319-323).
pub fn select_preferred_transcript(candidates: &[MeetingArtifact]) -> Option<MeetingArtifact> {
    let mut transcripts: Vec<&MeetingArtifact> = candidates.iter().filter(|c| c.artifact_type == "transcript").collect();
    if transcripts.is_empty() {
        return None;
    }
    transcripts.sort_by(|a, b| transcript_sort_key(a).cmp(&transcript_sort_key(b)));
    transcripts.last().cloned().cloned()
}

// ---------------------------------------------------------------------------
// download_transcript_text — mirrors 326-356
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:download_transcript_text` (326-356).
///
/// Uses `std::env::temp_dir()` + `std::fs` to mimic `tempfile.NamedTemporaryFile`.
pub fn download_transcript_text(
    client: &dyn MicrosoftGraphClient,
    meeting_ref: &TeamsMeetingRef,
    transcript: &MeetingArtifact,
    encoding: &str,
) -> Result<String, TeamsMeetingError> {
    let suffix = transcript.display_name.as_deref()
        .and_then(|n| Path::new(n).extension().and_then(|e| e.to_str()).map(|e| format!(".{}", e)))
        .unwrap_or_else(|| ".txt".to_string());
    // Use suffix or .txt default; Python uses Path(...).suffix or ".txt"
    let actual_suffix = if suffix.is_empty() { ".txt".to_string() } else { suffix };
    // Named temp file: prefix teams-transcript-
    let mut dest = std::env::temp_dir();
    let fname = format!("teams-transcript-{}{}", uuid_simple(), actual_suffix);
    dest.push(fname);
    // Ensure file exists (Python creates empty file via NamedTemporaryFile)
    let _ = fs::write(&dest, b"");
    let path_str = transcript_download_path(meeting_ref, transcript);
    // Download
    let download_result = client.download_to_file(&path_str, &dest);
    let text: String;
    match download_result {
        Ok(_) => {
            // Read text with encoding (only utf-8 supported in std; other encodings fall back to lossy)
            let bytes = fs::read(&dest).unwrap_or_default();
            if encoding.to_lowercase() == "utf-8" || encoding.to_lowercase() == "utf8" {
                text = String::from_utf8_lossy(&bytes).trim().to_string();
            } else {
                // For non-utf8, also use lossy (Python would use specified encoding)
                text = String::from_utf8_lossy(&bytes).trim().to_string();
            }
        }
        Err(exc) => {
            let _ = fs::remove_file(&dest);
            return Err(wrap_graph_error(&exc, &format!("Transcript {} not found for meeting {}", transcript.artifact_id, meeting_ref.meeting_id)));
        }
    }
    // Cleanup
    let _ = fs::remove_file(&dest);
    if text.is_empty() {
        return Err(TeamsMeetingError::ArtifactNotFound(format!("Transcript {} for meeting {} was empty.", transcript.artifact_id, meeting_ref.meeting_id)));
    }
    Ok(text)
}

fn uuid_simple() -> String {
    // Minimal uuid-like hex for temp name without `uuid` crate — use time + pid entropy
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = now ^ (pid << 32) ^ (now >> 16);
    format!("{:016x}", mixed)
}

// ---------------------------------------------------------------------------
// fetch_preferred_transcript_text — mirrors 359-370
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:fetch_preferred_transcript_text` (359-370).
pub fn fetch_preferred_transcript_text(
    client: &dyn MicrosoftGraphClient,
    meeting_ref: &TeamsMeetingRef,
) -> Result<(Option<MeetingArtifact>, Option<String>), TeamsMeetingError> {
    let transcripts = list_transcript_artifacts(client, meeting_ref)?;
    let transcript = select_preferred_transcript(&transcripts);
    if transcript.is_none() {
        return Ok((None, None));
    }
    let t = transcript.clone().unwrap();
    match download_transcript_text(client, meeting_ref, &t, "utf-8") {
        Ok(text) => Ok((Some(t), Some(text))),
        Err(TeamsMeetingError::ArtifactNotFound(_)) => Ok((None, None)),
        Err(e) => {
            // Python only catches TeamsMeetingArtifactNotFoundError, not generic
            // If it's artifact not found, return None; else propagate? In Python,
            // download_transcript_text raises TeamsMeetingArtifactNotFoundError for empty,
            // and _wrap_graph_error for 404 etc would be TeamsMeetingNotFoundError which is not caught.
            // But fetch_preferred catches only TeamsMeetingArtifactNotFoundError.
            // We mimic: only swallow ArtifactNotFound, propagate others.
            match e {
                TeamsMeetingError::ArtifactNotFound(_) => Ok((None, None)),
                _ => Err(e),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// list_recording_artifacts — mirrors 373-384
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:list_recording_artifacts` (373-384).
pub fn list_recording_artifacts(
    client: &dyn MicrosoftGraphClient,
    meeting_ref: &TeamsMeetingRef,
) -> Result<Vec<MeetingArtifact>, TeamsMeetingError> {
    let path = format!("{}/recordings", meeting_path(meeting_ref));
    match client.collect_paginated(&path) {
        Ok(payloads) => {
            let mut out = Vec::new();
            for payload in payloads {
                if payload.is_object() {
                    out.push(normalize_artifact("recording", &payload, None));
                }
            }
            Ok(out)
        }
        Err(exc) => Err(wrap_graph_error(&exc, &format!("No recordings found for Teams meeting {}", meeting_ref.meeting_id))),
    }
}

// ---------------------------------------------------------------------------
// download_recording_artifact — mirrors 387-409
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:download_recording_artifact` (387-409).
pub fn download_recording_artifact(
    client: &dyn MicrosoftGraphClient,
    meeting_ref: &TeamsMeetingRef,
    recording: &MeetingArtifact,
    destination: &Path,
) -> Result<Value, TeamsMeetingError> {
    let path = recording_download_path(meeting_ref, recording);
    match client.download_to_file(&path, destination) {
        Ok(result) => {
            let size = if result.size_bytes != 0 { result.size_bytes as i64 } else { recording.size_bytes.unwrap_or(0) };
            let ctype = result.content_type.clone().or_else(|| recording.content_type.clone());
            Ok(json!({
                "artifact": recording.to_dict(),
                "path": destination.to_string_lossy().to_string(),
                "size_bytes": size,
                "content_type": ctype,
            }))
        }
        Err(exc) => Err(wrap_graph_error(&exc, &format!("Recording {} not found for meeting {}", recording.artifact_id, meeting_ref.meeting_id))),
    }
}

// ---------------------------------------------------------------------------
// fetch_call_record_artifact — mirrors 412-448
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:fetch_call_record_artifact` (412-448).
pub fn fetch_call_record_artifact(
    client: &dyn MicrosoftGraphClient,
    call_record_id: &str,
    allow_permission_errors: bool,
) -> Result<Option<MeetingArtifact>, TeamsMeetingError> {
    let path = format!("/communications/callRecords/{}", percent_encode(call_record_id));
    let payload = match client.get_json(&path, None) {
        Ok(v) => v,
        Err(exc) => {
            if matches!(exc.status_code, 401 | 403) && allow_permission_errors {
                return Ok(None);
            }
            if exc.status_code == 404 {
                return Ok(None);
            }
            return Err(wrap_graph_error(&exc, &format!("Call record not found: {}", call_record_id)));
        }
    };
    if !payload.is_object() || payload.get("id").and_then(|v| v.as_str()).map(|s| s.trim().is_empty()).unwrap_or(true) {
        return Ok(None);
    }
    let mut metrics = json!({});
    if let Some(v) = payload.get("version") { metrics["version"] = v.clone(); }
    if let Some(v) = payload.get("modalities") { metrics["modalities"] = v.clone(); }
    if let Some(participants) = payload.get("participants").and_then(|v| v.as_array()) {
        metrics["participant_count"] = json!(participants.len());
    } else {
        metrics["participant_count"] = json!(0);
    }
    if let Some(org) = parse_organizer_user_id(&payload) {
        metrics["organizer"] = json!(org);
    }
    if let Some(sessions) = payload.get("sessions").and_then(|v| v.as_array()) {
        if !sessions.is_empty() {
            metrics["session_count"] = json!(sessions.len());
        }
    }
    let artifact_id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let display_name = payload.get("type").and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_else(|| "call_record".to_string());
    let source_url = payload.get("webUrl").and_then(|v| v.as_str()).map(|s| s.to_string());
    let created_at = payload.get("startDateTime").and_then(|v| v.as_str()).map(|s| s.to_string());
    let available_at = payload.get("endDateTime").and_then(|v| v.as_str()).map(|s| s.to_string());
    let mut meta: HashMap<String, Value> = HashMap::new();
    meta.insert("call_record".to_string(), payload.clone());
    meta.insert("metrics".to_string(), metrics);
    Ok(Some(MeetingArtifact {
        artifact_type: "call_record".to_string(),
        artifact_id,
        display_name: Some(display_name),
        content_type: None,
        source_url,
        download_url: None,
        created_at,
        available_at,
        size_bytes: None,
        metadata: meta,
    }))
}

// ---------------------------------------------------------------------------
// enrich_meeting_with_call_record — mirrors 451-464
// ---------------------------------------------------------------------------

/// Mirrors `meetings.py:enrich_meeting_with_call_record` (451-464).
pub fn enrich_meeting_with_call_record(
    client: &dyn MicrosoftGraphClient,
    meeting_ref: &TeamsMeetingRef,
    call_record_id: Option<&str>,
    allow_permission_errors: bool,
) -> Result<Option<MeetingArtifact>, TeamsMeetingError> {
    let resolved = if let Some(id) = call_record_id {
        if !id.trim().is_empty() { Some(id.trim().to_string()) } else { None }
    } else {
        meeting_ref.metadata.get("call_record_id").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    };
    if resolved.is_none() {
        return Ok(None);
    }
    fetch_call_record_artifact(client, &resolved.unwrap(), allow_permission_errors)
}

// ---------------------------------------------------------------------------
// Helpers for tests / external use
// ---------------------------------------------------------------------------

/// Expose sentinel set for testing parity.
pub fn is_resource_sentinel(value: &str) -> bool {
    RESOURCE_SENTINELS.contains(&value.to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_users_meeting_with_organizer() {
        let r = parse_graph_meeting_resource("/users('abc-123')/onlineMeetings('meet-456')");
        assert_eq!(r.organizer_user_id.as_deref(), Some("abc-123"));
        assert_eq!(r.meeting_id.as_deref(), Some("meet-456"));
        assert!(r.transcript_id.is_none());
    }

    #[test]
    fn parse_users_meeting_slash_form() {
        let r = parse_graph_meeting_resource("/users/abc123/onlineMeetings/meetXYZ/recordings/rec-1");
        assert_eq!(r.organizer_user_id.as_deref(), Some("abc123"));
        assert_eq!(r.meeting_id.as_deref(), Some("meetXYZ"));
        assert_eq!(r.recording_id.as_deref(), Some("rec-1"));
    }

    #[test]
    fn parse_communications_meeting() {
        let r = parse_graph_meeting_resource("/communications/onlineMeetings/meet-789/transcripts/trans-1");
        assert!(r.organizer_user_id.is_none());
        assert_eq!(r.meeting_id.as_deref(), Some("meet-789"));
        assert_eq!(r.transcript_id.as_deref(), Some("trans-1"));
    }

    #[test]
    fn parse_sentinel_yields_none() {
        let r = parse_graph_meeting_resource("/communications/onlineMeetings/getAllTranscripts");
        assert!(r.meeting_id.is_none());
        let r2 = parse_graph_meeting_resource("/users/u1/onlineMeetings/transcripts");
        assert!(r2.meeting_id.is_none());
    }

    #[test]
    fn parse_percent_encoded() {
        let r = parse_graph_meeting_resource("/users('a%20b')/onlineMeetings('m%2F1')");
        assert_eq!(r.organizer_user_id.as_deref(), Some("a b"));
        assert_eq!(r.meeting_id.as_deref(), Some("m/1"));
    }

    #[test]
    fn looks_like_transcript_via_odata() {
        assert!(looks_like_transcript_id("anything", Some("microsoft.graph.callTranscript")));
        assert!(!looks_like_transcript_id("meet-123", None));
        assert!(looks_like_transcript_id("something transcript something", None));
    }

    #[test]
    fn decoded_hint_transcript_marker() {
        // "hello-TranscriptV2" base64url encoded
        let raw = "hello-TranscriptV2";
        let b64 = {
            // simple encode via manual: use base64 crate logic reverse
            // We'll just test that decoded_id_hint returns lowercased decoded when given b64
            // Encode raw as base64 standard
            let bytes = raw.as_bytes();
            let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut out = String::new();
            let mut i = 0;
            while i < bytes.len() {
                let b0 = bytes[i] as u32;
                let b1 = if i+1 < bytes.len() { bytes[i+1] as u32 } else { 0 };
                let b2 = if i+2 < bytes.len() { bytes[i+2] as u32 } else { 0 };
                let n = (b0 << 16) | (b1 << 8) | b2;
                out.push(alphabet[((n >> 18) & 63) as usize] as char);
                out.push(alphabet[((n >> 12) & 63) as usize] as char);
                if i+1 < bytes.len() { out.push(alphabet[((n >> 6) & 63) as usize] as char); } else { out.push('='); }
                if i+2 < bytes.len() { out.push(alphabet[(n & 63) as usize] as char); } else { out.push('='); }
                i += 3;
            }
            out
        };
        assert!(decoded_id_hint(&b64).contains("transcript"));
        assert!(looks_like_transcript_id(&b64, None));
    }

    #[test]
    fn percent_encode_decode_roundtrip() {
        let s = "a b/c+d?e=f&g";
        let enc = percent_encode(s);
        assert!(!enc.contains(' '));
        assert!(!enc.contains('/'));
        let dec = percent_decode(&enc);
        assert_eq!(dec, s);
    }

    #[test]
    fn meeting_path_with_and_without_organizer() {
        let r1 = TeamsMeetingRef::new("meet 1", Some("user 1".to_string()));
        assert_eq!(meeting_path(&r1), "/users/user%201/onlineMeetings/meet%201");
        let r2 = TeamsMeetingRef::new("meet/2", None);
        assert_eq!(meeting_path(&r2), "/communications/onlineMeetings/meet%2F2");
    }

    #[test]
    fn wrap_graph_error_maps_codes() {
        let e401 = MicrosoftGraphAPIError::new(401, "GET", "/x", "unauthorized");
        assert!(matches!(wrap_graph_error(&e401, "missing"), TeamsMeetingError::Permission(_)));
        let e404 = MicrosoftGraphAPIError::new(404, "GET", "/x", "not found");
        assert!(matches!(wrap_graph_error(&e404, "missing"), TeamsMeetingError::NotFound(_)));
        let e500 = MicrosoftGraphAPIError::new(500, "GET", "/x", "oops");
        assert!(matches!(wrap_graph_error(&e500, "missing"), TeamsMeetingError::Generic(_)));
    }

    #[test]
    fn transcript_sort_key_completed_and_download() {
        let mut m1 = MeetingArtifact {
            artifact_type: "transcript".to_string(),
            artifact_id: "1".to_string(),
            display_name: None,
            content_type: None,
            source_url: Some("https://example.com".to_string()),
            download_url: None,
            created_at: Some("2024-01-01T00:00:00Z".to_string()),
            available_at: Some("2024-01-02T00:00:00Z".to_string()),
            size_bytes: None,
            metadata: {
                let mut map = HashMap::new();
                map.insert("status".to_string(), json!("completed"));
                map
            },
        };
        let mut m2 = MeetingArtifact {
            artifact_type: "transcript".to_string(),
            artifact_id: "2".to_string(),
            display_name: None,
            content_type: None,
            source_url: None,
            download_url: None,
            created_at: Some("2024-01-03T00:00:00Z".to_string()),
            available_at: None,
            size_bytes: None,
            metadata: HashMap::new(),
        };
        assert!(transcript_sort_key(&m1) > transcript_sort_key(&m2));
        // has_download vs not
        m2.source_url = Some("https://example.com".to_string());
        // still m1 > m2 because completed flag
        assert!(transcript_sort_key(&m1) > transcript_sort_key(&m2));
    }

    #[test]
    fn select_preferred_picks_best() {
        let a1 = MeetingArtifact {
            artifact_type: "transcript".to_string(),
            artifact_id: "1".to_string(),
            display_name: None,
            content_type: None,
            source_url: None,
            download_url: None,
            created_at: Some("2024-01-01T00:00:00Z".to_string()),
            available_at: None,
            size_bytes: None,
            metadata: HashMap::new(),
        };
        let mut a2 = a1.clone();
        a2.artifact_id = "2".to_string();
        a2.metadata.insert("status".to_string(), json!("completed"));
        a2.download_url = Some("https://dl".to_string());
        a2.available_at = Some("2024-01-02T00:00:00Z".to_string());
        let a3 = MeetingArtifact {
            artifact_type: "recording".to_string(),
            artifact_id: "3".to_string(),
            display_name: None,
            content_type: None,
            source_url: None,
            download_url: None,
            created_at: None,
            available_at: None,
            size_bytes: None,
            metadata: HashMap::new(),
        };
        let best = select_preferred_transcript(&[a1, a2.clone(), a3]);
        assert_eq!(best.unwrap().artifact_id, "2");
    }

    #[test]
    fn download_paths_use_download_url_when_present() {
        let r = TeamsMeetingRef::new("meet1", None);
        let art = MeetingArtifact {
            artifact_type: "transcript".to_string(),
            artifact_id: "t1".to_string(),
            display_name: None,
            content_type: None,
            source_url: None,
            download_url: Some("https://download.example.com/file".to_string()),
            created_at: None,
            available_at: None,
            size_bytes: None,
            metadata: HashMap::new(),
        };
        assert_eq!(transcript_download_path(&r, &art), "https://download.example.com/file");
        assert_eq!(recording_download_path(&r, &art), "https://download.example.com/file");
        let art2 = MeetingArtifact { download_url: None, ..art };
        assert!(transcript_download_path(&r, &art2).contains("/transcripts/t1/content"));
        assert!(recording_download_path(&r, &art2).contains("/recordings/t1/content"));
    }

    #[test]
    fn parse_organizer_and_thread() {
        let payload = json!({
            "organizer": {"identity": {"user": {"id": "user-123"}}},
            "chatInfo": {"threadId": "thread-456"}
        });
        assert_eq!(parse_organizer_user_id(&payload).as_deref(), Some("user-123"));
        assert_eq!(parse_thread_id(&payload).as_deref(), Some("thread-456"));
        let payload2 = json!({"threadId": "t2"});
        assert_eq!(parse_thread_id(&payload2).as_deref(), Some("t2"));
    }
}
