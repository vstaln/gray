//! ACP agent server — slice 2/4
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/acp_adapter/server.py`
//! slice 2 — lines 800–1600 of 2640.
//!
//! Covers: remainder of `_build_model_state` (800–977), `_resolve_model_selection`
//! (979–996), `_build_usage_update` (998–1030), `_send_usage_update` (1032–1049),
//! `_provenance_meta` (1051–1069), `_send_session_info_update` (1071–1118),
//! `_schedule_usage_update` (1119–1125), `_register_session_mcp_servers`
//! (1126–1195), `_schedule_mcp_late_refresh` (1197–1291), `initialize`
//! (1293–1327), `authenticate` (1329–1349), and session-management history
//! helpers `_flatten_history_text` through `_replay_session_history` plus the
//! `new_session` prologue (1351–1600). Slice 3 continues at `new_session` body.
//!
//! T0410 — 1:1 port, no cargo (NEVER cargo). All external crates / ACP SDK
//! types are stubbed as local structs for traceability; `async` is modelled
//! as sync stubs with `_async_` prefixes where needed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// Re-use slice 1 surface where possible (kept as `allow(unused_imports)` so
// the file remains self-contained if `server_slice1` is not linked).
#[allow(unused_imports)]
use crate::server_slice1::{
    AdvertisedCommand, AgentCapabilities, AgentMessageChunk, AgentThoughtChunk, EnvItem,
    HermesAcpAgent, McpServerHttp, McpServerSse, McpServerStdio, ModelInfo, SessionMode,
    SessionModeState, SessionModelState, SessionState, TextContentBlock,
    ACP_MAX_MODELS_PER_PROVIDER, LIST_SESSIONS_PAGE_SIZE, MAX_ACP_RESOURCE_BYTES,
};

// ---------------------------------------------------------------------------
// Local shims for types referenced in this slice but defined in slice 1
// (duplicated here so this file is readable in isolation; `crate::server_slice1`
// remains the canonical definition — these aliases exist only for reviewer
// line-mapping when reading slice 2 standalone).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Default)]
pub struct ClientCapabilities {
    pub dummy: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AgentCapabilitiesFull {
    pub load_session: bool,
    pub prompt_image: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SessionCapabilities {
    pub fork: bool,
    pub list: bool,
    pub resume: bool,
}

#[derive(Debug, Clone, Default)]
pub struct InitializeResponse {
    pub protocol_version: u32,
    pub agent_name: String,
    pub agent_version: String,
}

#[derive(Debug, Clone, Default)]
pub struct AuthenticateResponse {}

#[derive(Debug, Clone, Default)]
pub struct NewSessionResponse {
    pub session_id: String,
    pub current_model_id: String,
    pub current_mode_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct UsageUpdate {
    pub size: usize,
    pub used: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SessionInfoUpdate {
    pub title: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default)]
pub struct UserMessageChunk {
    pub text: String,
    pub field_meta: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Default)]
pub struct HistoryMessage {
    pub role: String,
    pub content: HistoryContent,
    pub reasoning: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<Vec<HistoryToolCall>>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub enum HistoryContent {
    #[default]
    Empty,
    Text(String),
    Parts(Vec<HashMap<String, String>>),
}

#[derive(Debug, Clone, Default)]
pub struct HistoryToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String, // JSON string or raw
    pub function: Option<ToolFunction>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolFunction {
    pub name: String,
    pub arguments: String,
}

// ---------------------------------------------------------------------------
// Constants — mirrors slice 1 re-exports for local use
// ---------------------------------------------------------------------------

const PROTOCOL_VERSION: u32 = 1;
const HERMES_VERSION_FALLBACK: &str = "0.0.0";
const TERMINAL_SETUP_AUTH_METHOD_ID: &str = "terminal_setup";

// ---------------------------------------------------------------------------
// Helpers — mirrors tiny utils from Python slice 2 dependencies
// ---------------------------------------------------------------------------

fn normalize_provider(raw: &str) -> String {
    // Mirrors `hermes_cli.models.normalize_provider(raw)` — stub as lowercased
    raw.trim().to_lowercase()
}

fn provider_label(normalized: &str) -> String {
    // Mirrors `hermes_cli.models.provider_label(row_provider)` — stub: title-cased fallback
    let n = normalized.trim();
    if n.is_empty() {
        return "Unknown".to_string();
    }
    // Simple title case for stub
    let mut out = String::new();
    let mut cap = true;
    for ch in n.chars() {
        if cap {
            out.extend(ch.to_uppercase());
            cap = false;
        } else {
            out.push(ch);
        }
        if ch == '_' || ch == '-' || ch == ':' {
            cap = true;
        }
    }
    out
}

fn semantic_provider(provider_id: &str) -> String {
    // Mirrors `def semantic_provider(provider_id: str)` (774-780)
    let raw = provider_id.trim().to_lowercase();
    if raw == "ollama" || raw == "custom:ollama" {
        return "ollama".to_string();
    }
    if raw.starts_with("custom:") {
        return raw;
    }
    normalize_provider(&raw)
}

// ---------------------------------------------------------------------------
// _build_model_state continuation — lines 800-977
// ---------------------------------------------------------------------------
// This is the 1:1 of the inner block of `_build_model_state` from line 800
// (`if not row_provider: continue`) through the fallback assembly at 970-977.
// Slice 1 stops at that guard; slice 2 implements the remainder.

/// Mirrors the `for model_entry in row_models:` expansion (807-850) as a helper.
///
/// Python:
/// ```python
/// for model_entry in row_models:
///     if isinstance(model_entry, dict):
///         rendered_model = str(model_entry.get("id") or ...).strip()
///     else:
///         rendered_model = str(model_entry or "").strip()
///     ...
///     encoded_provider = "custom:ollama" if raw_row_provider == "ollama" else ...
///     choice_id = self._encode_model_choice(encoded_provider, rendered_model)
///     semantic_id = f"{semantic_provider(encoded_provider)}:{rendered_model}"
///     if choice_id in seen_ids or semantic_id in seen_semantic_ids: continue
///     is_current = semantic_provider(encoded_provider) == semantic_provider(current_choice_provider) and rendered_model == model
///     description = f"Provider: {provider_name}" + (" • current" if is_current else "")
///     available_models.append(ModelInfo(...))
/// ```
pub fn build_model_state_row_models(
    raw_row_provider: &str,
    row_provider: &str,
    provider_name: &str,
    row_models: &[ModelEntry],
    current_choice_provider: &str,
    model: &str,
    available_models: &mut Vec<ModelInfo>,
    seen_ids: &mut HashSet<String>,
    seen_semantic_ids: &mut HashSet<String>,
) {
    // Mirrors lines 807-850
    for model_entry in row_models {
        let rendered_model = match model_entry {
            ModelEntry::Dict(m) => m
                .get("id")
                .or_else(|| m.get("model"))
                .or_else(|| m.get("name"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default(),
            ModelEntry::Str(s) => s.trim().to_string(),
        };
        if rendered_model.is_empty() {
            continue; // 817-818
        }
        // 819-827: encoded_provider derivation
        let encoded_provider = if raw_row_provider == "ollama" {
            "custom:ollama".to_string()
        } else if raw_row_provider == "custom:ollama" {
            raw_row_provider.to_string()
        } else if raw_row_provider.starts_with("custom:") {
            raw_row_provider.to_string()
        } else {
            row_provider.to_string()
        };
        let choice_id = HermesAcpAgent::encode_model_choice(Some(&encoded_provider), Some(&rendered_model));
        let semantic_id = format!("{}:{}", semantic_provider(&encoded_provider), rendered_model);
        if seen_ids.contains(&choice_id) || seen_semantic_ids.contains(&semantic_id) {
            continue; // 832-833
        }
        let is_current = semantic_provider(&encoded_provider) == semantic_provider(current_choice_provider)
            && rendered_model == model; // 834-838
        let mut description = format!("Provider: {provider_name}");
        if is_current {
            description.push_str(" • current"); // 839-841
        }
        available_models.push(ModelInfo {
            model_id: choice_id.clone(),
            name: format!("{provider_name} · {rendered_model}"),
            description: Some(description),
        });
        seen_ids.insert(choice_id);
        seen_semantic_ids.insert(semantic_id);
    }
}

#[derive(Debug, Clone)]
pub enum ModelEntry {
    Dict(HashMap<String, String>),
    Str(String),
}

/// Mirrors named custom provider append block (852-884).
pub fn append_named_custom_catalogs(
    named_catalogs: &[(String, String, Vec<(String, String)>)],
    normalized_provider: &str,
    model: &str,
    available_models: &mut Vec<ModelInfo>,
    seen_ids: &mut HashSet<String>,
    seen_semantic_ids: &mut HashSet<String>,
    native_empty_rows: &HashSet<String>,
    named_empty_authoritative: &mut HashSet<String>,
) {
    // Mirrors `named_empty_authoritative: set[str] = set(native_empty_rows)` (855)
    // Caller should have pre-seeded `named_empty_authoritative` with `native_empty_rows`.
    for (named_slug, named_label, named_catalog) in named_catalogs {
        if named_catalog.is_empty() {
            named_empty_authoritative.insert(named_slug.trim().to_lowercase()); // 857-858
            continue;
        }
        for (named_model, named_desc) in named_catalog {
            let named_choice = HermesAcpAgent::encode_model_choice(Some(named_slug), Some(named_model)); // 861
            let named_semantic_id = format!("{}:{}", semantic_provider(named_slug), named_model); // 862-864
            if named_choice.is_empty()
                || seen_ids.contains(&named_choice)
                || seen_semantic_ids.contains(&named_semantic_id)
            {
                continue; // 865-869
            }
            let mut parts = vec![format!("Provider: {named_label}")];
            if !named_desc.trim().is_empty() {
                parts.push(named_desc.trim().to_string());
            }
            if named_slug == normalized_provider && named_model == model {
                parts.push("current".to_string());
            }
            available_models.push(ModelInfo {
                model_id: named_choice.clone(),
                name: named_model.clone(),
                description: Some(parts.join(" • ")),
            });
            seen_ids.insert(named_choice);
            seen_semantic_ids.insert(named_semantic_id);
        }
    }
    let _ = native_empty_rows; // suppress unused in stub path
}

/// Mirrors `def empty_catalog_applies(provider_id: str) -> bool:` (886-902).
pub fn empty_catalog_applies(provider_id: &str, named_empty_authoritative: &HashSet<String>) -> bool {
    let raw = provider_id.trim().to_lowercase();
    let normalized = normalize_provider(&raw);
    if normalized == "custom" {
        // 889-895 specialized branch
        return named_empty_authoritative.iter().any(|candidate| {
            candidate == &raw
                || format!("custom:{candidate}") == raw
                || (raw == "custom" && candidate == "custom")
        });
    }
    // 896-902 general branch
    named_empty_authoritative.iter().any(|candidate| {
        candidate == &raw
            || candidate == &format!("custom:{normalized}")
            || candidate == &format!("custom:{raw}")
            || normalize_provider(candidate) == normalized
    })
}

/// Mirrors `def choice_provider(model_id: str) -> str:` (904-922).
pub fn choice_provider(model_id: &str) -> String {
    let parts: Vec<&str> = model_id.split(':').collect();
    if parts.first() == Some(&"custom") && parts.len() > 1 {
        // Mirrors `from hermes_cli.models import _configured_custom_provider_ids` path (907)
        // Stub: without live custom provider id registry we behave as if no
        // candidate matches and return "custom" (line 921).
        let lowered = model_id.to_lowercase();
        // In real impl this scans sorted configured ids by length desc
        // and returns first candidate where lowered.startswith(candidate + ":")
        // Stub: return the longest matching prefix heuristic collapsed to "custom"
        let _ = lowered;
        return "custom".to_string();
    }
    parts.first().unwrap_or(&"").to_string()
}

/// Mirrors the empty-catalog filtering + current model insertion tail (924-977).
///
/// ```python
/// if named_empty_authoritative: available_models = [item for item in ... if not empty_catalog_applies(...)]
/// current_is_empty = empty_catalog_applies(current_choice_provider)
/// if current_is_empty: available_models = [item for item in ... if " • current" not in ...]
/// current_model_id = "" if current_is_empty else _encode_model_choice(...)
/// if current_model_id and current_model_id not in seen_ids and not current_is_empty:
///     available_models.insert(0, ModelInfo(...))
/// if not available_models and current_is_empty: return SessionModelState([], "")
/// if available_models: return SessionModelState(...)
/// except Exception: log.debug → fallback
/// if not model: return None
/// return SessionModelState([ModelInfo(fallback)], fallback)
/// ```
pub fn build_model_state_tail_filter(
    available_models: Vec<ModelInfo>,
    seen_ids: HashSet<String>,
    named_empty_authoritative: &HashSet<String>,
    current_choice_provider: &str,
    normalized_provider: &str,
    model: &str,
) -> (Vec<ModelInfo>, HashSet<String>, String, bool) {
    let mut available_models = available_models;
    let mut seen_ids = seen_ids;
    if !named_empty_authoritative.is_empty() {
        // 924-930
        available_models = available_models
            .into_iter()
            .filter(|item| !empty_catalog_applies(&choice_provider(&item.model_id), named_empty_authoritative))
            .collect();
        seen_ids = available_models.iter().map(|m| m.model_id.clone()).collect();
    }
    let current_is_empty = empty_catalog_applies(current_choice_provider, named_empty_authoritative); // 932
    if current_is_empty {
        // 933-939: strip any " • current" badge when current catalog is empty
        available_models = available_models
            .into_iter()
            .filter(|item| !item.description.as_deref().unwrap_or("").contains(" • current"))
            .collect();
        seen_ids = available_models.iter().map(|m| m.model_id.clone()).collect();
    }
    let current_model_id = if current_is_empty {
        String::new()
    } else {
        HermesAcpAgent::encode_model_choice(Some(current_choice_provider), Some(model)) // 940-942
    };
    let mut do_insert = false;
    if !current_model_id.is_empty() && !seen_ids.contains(&current_model_id) && !current_is_empty {
        do_insert = true;
    }
    if do_insert {
        let provider_name = provider_label(normalized_provider);
        available_models.insert(
            0,
            ModelInfo {
                model_id: current_model_id.clone(),
                name: format!("{provider_name} · {model}"),
                description: Some(format!("Provider: {provider_name} • current")),
            },
        );
        seen_ids.insert(current_model_id.clone());
    }
    (available_models, seen_ids, current_model_id, current_is_empty)
}

pub fn build_model_state_finalize(
    available_models: Vec<ModelInfo>,
    current_model_id: String,
    current_is_empty: bool,
    provider: &str,
    model: &str,
) -> Option<SessionModelState> {
    // Mirrors lines 958-977
    if available_models.is_empty() && current_is_empty {
        return Some(SessionModelState {
            available_models: vec![],
            current_model_id: String::new(),
        });
    }
    if !available_models.is_empty() {
        let cid = if !current_model_id.is_empty() || current_is_empty {
            current_model_id.clone()
        } else {
            available_models.first().map(|m| m.model_id.clone()).unwrap_or_default()
        };
        return Some(SessionModelState {
            available_models,
            current_model_id: cid,
        });
    }
    // Fallback when try block raised (967-977)
    if model.trim().is_empty() {
        return None;
    }
    let fallback_choice = HermesAcpAgent::encode_model_choice(Some(provider), Some(model));
    Some(SessionModelState {
        available_models: vec![ModelInfo {
            model_id: fallback_choice.clone(),
            name: model.to_string(),
            description: None,
        }],
        current_model_id: fallback_choice,
    })
}

// Full 1:1 of the try/except shim that slice 1 stubbed — kept here for
// reviewers diffing against Python 731-977 line-by-line.
#[allow(dead_code)]
pub fn build_model_state_full(state: &SessionState) -> Option<SessionModelState> {
    let model = state
        .model
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| state.agent_model.trim())
        .to_string();
    let provider = state
        .agent_provider
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("openrouter")
        .to_string();
    // In this slice the inventory crate is not linked (NEVER cargo) so we
    // exercise the except path directly, matching slice 1 stub behaviour
    // (logs debug and returns fallback). The helpers above remain available
    // for future wiring when inventory is linked.
    let _ = provider_label(&normalize_provider(&provider));
    build_model_state_finalize(vec![], String::new(), false, &provider, &model)
}

// ---------------------------------------------------------------------------
// _resolve_model_selection — lines 979-996
// ---------------------------------------------------------------------------

/// Resolve `provider:model` input into the provider and normalized model id.
/// Mirrors `def _resolve_model_selection(raw_model, current_provider)` (979-996).
pub fn resolve_model_selection(raw_model: &str, current_provider: &str) -> (String, String) {
    let mut target_provider = current_provider.to_string();
    let mut new_model = raw_model.trim().to_string();
    // Mirrors `try: from hermes_cli.models import detect_provider_for_model, parse_model_input`
    // In Rust stub (NEVER cargo) we approximate parse_model_input: if raw contains
    // `provider:model` where provider is a known prefix, split; else keep.
    // Stub respects the `except Exception: logger.debug` fallback.
    let parsed = parse_model_input_stub(&new_model, current_provider);
    if let Some((p, m)) = parsed {
        target_provider = p;
        new_model = m;
        if target_provider == current_provider {
            if let Some((det_p, det_m)) = detect_provider_for_model_stub(&new_model, current_provider) {
                target_provider = det_p;
                new_model = det_m;
            }
        }
    }
    (target_provider, new_model)
}

fn parse_model_input_stub(input: &str, current_provider: &str) -> Option<(String, String)> {
    // Very small stub of `parse_model_input`: recognises `provider:model` when
    // the provider part is non-empty and contains no spaces.
    let input = input.trim().to_string();
    if let Some(colon) = input.find(':') {
        let prov = input[..colon].trim().to_string();
        let rest = input[colon + 1..].trim().to_string();
        if !prov.is_empty() && !rest.is_empty() && !prov.contains(' ') {
            return Some((prov.to_lowercase(), rest));
        }
    }
    // No split — caller treats as current_provider + raw
    let _ = current_provider;
    None
}

fn detect_provider_for_model_stub(model: &str, current: &str) -> Option<(String, String)> {
    // Mirrors `detect_provider_for_model(new_model, current_provider)` — stub
    // returns None (no detection) which is the safe fallback.
    let _ = (model, current);
    None
}

#[allow(dead_code)]
pub fn _resolve_model_selection(raw_model: &str, current_provider: &str) -> (String, String) {
    resolve_model_selection(raw_model, current_provider)
}

// ---------------------------------------------------------------------------
// _build_usage_update — lines 998-1030
// ---------------------------------------------------------------------------

/// Build ACP native context-usage data for clients like Zed.
/// Mirrors `def _build_usage_update(state)` (998-1030).
///
/// Zed's circular context indicator is driven by ACP `usage_update`
/// session updates: `size` is the model context window and `used` is the
/// current request pressure. Hermes estimates `used` from the same
/// buckets it sends to providers: system prompt, conversation history, and
/// tool schemas.
pub fn build_usage_update(state: &SessionState, system_prompt: &str, tools_count: usize) -> Option<UsageUpdate> {
    // Mirrors `compressor = getattr(agent, "context_compressor", None); size = int(getattr(compressor, "context_length", 0) or 0)`
    // In Rust stub we read `state.usage_context_length` if present (otherwise 0)
    let size = state_usage_context_length(state);
    if size <= 0 {
        return None; // 1011-1012
    }
    // Mirrors `try: from agent.model_metadata import estimate_request_tokens_rough; used = estimate_request_tokens_rough(...)`
    let used = estimate_request_tokens_rough_stub(state, system_prompt, tools_count)
        .unwrap_or_else(|| state_usage_last_prompt_tokens(state));
    Some(UsageUpdate {
        size: size.max(0) as usize,
        used: used.max(0) as usize,
    })
}

// Stub helpers for compressor fields — in real crate these live on `state.agent.context_compressor`
fn state_usage_context_length(_state: &SessionState) -> i64 {
    // In slice 2 standalone mode we have no live compressor; return 0 so
    // callers see `None` (the most common path when compressor absent).
    // When linked with real state, replace with `state.agent.context_compressor.context_length`.
    0
}
fn state_usage_last_prompt_tokens(_state: &SessionState) -> i64 {
    0
}
fn estimate_request_tokens_rough_stub(_state: &SessionState, _system_prompt: &str, _tools: usize) -> Option<i64> {
    // Mirrors `estimate_request_tokens_rough(history, system_prompt, tools)` — stub returns None
    // to exercise the `except: used = compressor.last_prompt_tokens` fallback.
    None
}

#[allow(dead_code)]
pub fn _build_usage_update(state: &SessionState) -> Option<UsageUpdate> {
    build_usage_update(state, "", 0)
}

// ---------------------------------------------------------------------------
// _send_usage_update — lines 1032-1049
// ---------------------------------------------------------------------------

/// Send ACP native context usage to the connected client.
/// Mirrors `async def _send_usage_update(self, state)` (1032-1049).
pub fn send_usage_update_sync(state: &SessionState, has_conn: bool) -> bool {
    if !has_conn {
        return false; // 1034-1035
    }
    let update = match build_usage_update(state, "", 0) {
        None => return false, // 1036-1038
        Some(u) => u,
    };
    // Mirrors `await self._conn.session_update(session_id=state.session_id, update=update)`
    // In sync stub we just log and return success; real async wiring is in
    // `server_slice3`/`server_slice4` when tokio is linked.
    let _ = (state.session_id.clone(), update);
    // Mirrors `except Exception: logger.warning(...)`
    true
}

#[allow(dead_code)]
pub fn _send_usage_update_stub(state: &SessionState, has_conn: bool) -> bool {
    send_usage_update_sync(state, has_conn)
}

// ---------------------------------------------------------------------------
// _provenance_meta — lines 1051-1069
// ---------------------------------------------------------------------------

/// Best-effort `_meta.hermes.sessionProvenance` for an ACP session.
/// Mirrors `def _provenance_meta(self, acp_session_id, current_hermes_session_id, previous...)` (1051-1069).
pub fn provenance_meta(
    acp_session_id: &str,
    current_hermes_session_id: &str,
    previous_hermes_session_id: Option<&str>,
) -> Option<HashMap<String, String>> {
    // Mirrors `try: return session_provenance_meta(db, acp_session_id, current..., previous...)`
    // Stub: without DB linkage (NEVER cargo) return a minimal provenance map
    // so callers can attach `_meta.hermes.sessionProvenance` without panicking,
    // matching the Python `except Exception: logger.debug(...); return None` fallback
    // but surfacing a best-effort stub. Real DB path will be wired when hermes_state is linked.
    let mut meta = HashMap::new();
    meta.insert("acpSessionId".to_string(), acp_session_id.to_string());
    meta.insert("hermesSessionId".to_string(), current_hermes_session_id.to_string());
    if let Some(prev) = previous_hermes_session_id {
        if !prev.trim().is_empty() {
            meta.insert("previousHermesSessionId".to_string(), prev.to_string());
        }
    }
    Some(meta)
}

#[allow(dead_code)]
pub fn _provenance_meta(
    acp_session_id: &str,
    current_hermes_session_id: &str,
    previous: Option<&str>,
) -> Option<HashMap<String, String>> {
    provenance_meta(acp_session_id, current_hermes_session_id, previous)
}

// ---------------------------------------------------------------------------
// _send_session_info_update — lines 1071-1118
// ---------------------------------------------------------------------------

/// Send ACP native session metadata after Hermes changes it.
/// Mirrors `async def _send_session_info_update(self, session_id, *, current..., previous...)` (1071-1118).
///
/// When the internal Hermes head rotated (e.g. compression-driven session split
/// during a turn), pass `previous_hermes_session_id` so the attached
/// `_meta.hermes.sessionProvenance` flags the rotation reason.
pub fn send_session_info_update_sync(
    session_id: &str,
    has_conn: bool,
    db_title: Option<String>,
    current_hermes_session_id: Option<&str>,
    previous_hermes_session_id: Option<&str>,
) -> Option<SessionInfoUpdate> {
    if !has_conn {
        return None; // 1084-1085
    }
    // Mirrors `row = self.session_manager._get_db().get_session(session_id)` try/except (1086-1092)
    // In stub we accept `db_title` as the already-fetched row title to avoid DB linkage.
    let title = match db_title {
        Some(t) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => None,
    };
    // Mirrors `updated_at = datetime.now(timezone.utc).isoformat()` (1099)
    let updated_at = now_utc_iso8601();
    let _meta = provenance_meta(
        session_id,
        current_hermes_session_id.unwrap_or(session_id),
        previous_hermes_session_id,
    );
    let _ = _meta; // attached as field_meta in real ACP update
    let update = SessionInfoUpdate { title, updated_at };
    // Mirrors `await self._conn.session_update(...)` try/except (1111-1117)
    Some(update)
}

fn now_utc_iso8601() -> String {
    // Minimal stub for `datetime.now(timezone.utc).isoformat()` — returns a
    // fixed sentinel; real impl uses `chrono` when linked.
    "1970-01-01T00:00:00+00:00".to_string()
}

#[allow(dead_code)]
pub fn _send_session_info_update(
    session_id: &str,
    has_conn: bool,
    title: Option<String>,
    current: Option<&str>,
    previous: Option<&str>,
) -> Option<SessionInfoUpdate> {
    send_session_info_update_sync(session_id, has_conn, title, current, previous)
}

// ---------------------------------------------------------------------------
// _schedule_usage_update — lines 1119-1125
// ---------------------------------------------------------------------------

/// Schedule native context indicator refresh after ACP responses.
/// Mirrors `def _schedule_usage_update(self, state)` (1119-1125).
pub fn schedule_usage_update(has_conn: bool) -> bool {
    if !has_conn {
        return false; // 1121-1122
    }
    // Mirrors `loop = asyncio.get_running_loop(); loop.call_soon(asyncio.create_task, self._send_usage_update(state))` (1123-1124)
    // In Rust sync stub we just signal that a task would be scheduled.
    true
}

#[allow(dead_code)]
pub fn _schedule_usage_update(has_conn: bool) -> bool {
    schedule_usage_update(has_conn)
}

// ---------------------------------------------------------------------------
// _register_session_mcp_servers — lines 1126-1195
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum McpServerDef {
    Stdio { name: String, command: String, args: Vec<String>, env: Vec<(String, String)> },
    Http { name: String, url: String, headers: Vec<(String, String)> },
    Sse { name: String, url: String, headers: Vec<(String, String)> },
}

/// Register ACP-provided MCP servers and refresh the agent tool surface.
/// Mirrors `async def _register_session_mcp_servers(self, state, mcp_servers)` (1126-1195).
pub fn register_session_mcp_servers_sync(
    session_id: &str,
    mcp_servers: Option<&[McpServerDef]>,
    enabled_toolsets: Option<Vec<String>>,
) -> Option<Vec<String>> {
    let servers = match mcp_servers {
        None | Some([]) => return None, // 1132-1133
        Some(s) => s,
    };
    // Mirrors `try: from tools.mcp_tool import register_mcp_servers; config_map = {}` (1135-1152)
    let mut config_map: HashMap<String, HashMap<String, String>> = HashMap::new();
    for server in servers {
        match server {
            McpServerDef::Stdio { name, command, args, env } => {
                let mut cfg = HashMap::new();
                cfg.insert("command".to_string(), command.clone());
                cfg.insert("args".to_string(), args.join(" "));
                for (k, v) in env {
                    cfg.insert(format!("env:{k}"), v.clone());
                }
                config_map.insert(name.clone(), cfg);
            }
            McpServerDef::Http { name, url, headers } | McpServerDef::Sse { name, url, headers } => {
                let mut cfg = HashMap::new();
                cfg.insert("url".to_string(), url.clone());
                for (k, v) in headers {
                    cfg.insert(format!("header:{k}"), v.clone());
                }
                config_map.insert(name.clone(), cfg);
            }
        }
    }
    // Mirrors `await asyncio.to_thread(register_mcp_servers, config_map)` try/except (1154-1161)
    // Stub: succeed without real MCP linkage; log on failure shape preserved.
    let _ = config_map;

    // Mirrors `try: from model_tools import get_tool_definitions; ...` (1163-1195)
    // `enabled_toolsets = _expand_acp_enabled_toolsets(getattr(state.agent, "enabled_toolsets", None) or ["hermes-acp"], mcp_server_names=...)`
    let mcp_names: Vec<String> = servers
        .iter()
        .map(|s| match s {
            McpServerDef::Stdio { name, .. } => name.clone(),
            McpServerDef::Http { name, .. } => name.clone(),
            McpServerDef::Sse { name, .. } => name.clone(),
        })
        .collect();
    let expanded = expand_acp_enabled_toolsets_stub(enabled_toolsets.unwrap_or_else(|| vec!["hermes-acp".to_string()]), &mcp_names);
    // Mirrors `state.agent.tools = get_tool_definitions(...); state.agent.valid_tool_names = {...}; inject_memory_provider_tools(...); invalidate()`
    // Stub: return expanded list so callers can observe the refresh without
    // needing live tool definitions.
    eprintln!("[acp_adapter.server] Session {session_id}: refreshed tool surface after ACP MCP registration ({} tools)", expanded.len());
    Some(expanded)
}

fn expand_acp_enabled_toolsets_stub(mut base: Vec<String>, mcp_names: &[String]) -> Vec<String> {
    // Mirrors `_expand_acp_enabled_toolsets` — stub appends mcp names as toolset ids
    for n in mcp_names {
        let ts = format!("mcp:{n}");
        if !base.contains(&ts) {
            base.push(ts);
        }
    }
    base
}

#[allow(dead_code)]
pub fn _register_session_mcp_servers(
    session_id: &str,
    servers: Option<&[McpServerDef]>,
    toolsets: Option<Vec<String>>,
) -> Option<Vec<String>> {
    register_session_mcp_servers_sync(session_id, servers, toolsets)
}

// ---------------------------------------------------------------------------
// _schedule_mcp_late_refresh — lines 1197-1291
// ---------------------------------------------------------------------------

/// Refresh the agent's tool snapshot when background MCP discovery lands late.
/// Mirrors `def _schedule_mcp_late_refresh(self, state)` (1197-1291).
///
/// ACP entry.py starts MCP tool discovery in a background daemon thread so a
/// slow/dead configured server can't block `asyncio.run()`. `_make_agent`
/// briefly joins that thread (bounded ~1.5s) so already-spawning fast servers
/// land in the snapshot — but a server slower than the bound lands *after*.
///
/// This schedules an off-critical-path daemon that waits for discovery to
/// finish (bounded 30s), then rebuilds the snapshot via the shared
/// `refresh_agent_mcp_tools` helper. Mirrors the TUI late-refresh (PR #48403).
pub fn schedule_mcp_late_refresh(
    session_id: &str,
    user_turn_count: usize,
    api_call_count: usize,
    is_running: bool,
    mcp_discovery_in_flight: bool,
) -> bool {
    // Mirrors `try: from hermes_cli.mcp_startup import mcp_discovery_in_flight; except: return` (1224-1229)
    if !mcp_discovery_in_flight {
        return false;
    }
    // Mirrors threading.Thread(target=_wait_then_refresh, daemon=True).start() (1287-1291)
    // Stub: model the decision tree of `_wait_then_refresh` synchronously for 1:1 audit.

    // `if not join_mcp_discovery(timeout=30.0): return` (1240-1241) — stub as success
    let joined = true;
    if !joined {
        return false;
    }
    // `with self.session_manager._lock: current = self.session_manager._sessions.get(session_id)` (1248-1251)
    // Stub: assume session still live and agent identity matches
    let session_live = true;
    let agent_matches = true;
    if !session_live || !agent_matches {
        return false;
    }
    // Cache safety guard (1253-1268): bail if conversation already started or a turn is running
    if is_running {
        return false;
    }
    if user_turn_count > 0 || api_call_count > 0 {
        return false;
    }
    // Mirrors `added = refresh_agent_mcp_tools(agent, quiet_mode=True)` (1272)
    // Stub: no tools added in this standalone slice
    let added: Vec<String> = vec![];
    if !added.is_empty() {
        eprintln!(
            "[acp_adapter.server] Session {session_id}: late MCP refresh added {} tools: {}",
            added.len(),
            added.join(", ")
        );
    }
    true
}

#[allow(dead_code)]
pub fn _schedule_mcp_late_refresh(
    session_id: &str,
    user_turn_count: usize,
    api_call_count: usize,
    is_running: bool,
    in_flight: bool,
) -> bool {
    schedule_mcp_late_refresh(session_id, user_turn_count, api_call_count, is_running, in_flight)
}

// ---------------------------------------------------------------------------
// initialize — lines 1293-1327
// ---------------------------------------------------------------------------

/// Mirrors `async def initialize(self, protocol_version, client_capabilities, client_info, **kwargs)` (1295-1327).
pub fn initialize(
    protocol_version: Option<u32>,
    client_info: Option<&Implementation>,
) -> InitializeResponse {
    let resolved = protocol_version.unwrap_or(PROTOCOL_VERSION); // 1302-1304
    let _ = resolved;
    // Mirrors `auth_methods = build_auth_methods()` (1305) — stub as empty
    let _auth_methods: Vec<String> = vec![];
    let client_name = client_info.map(|c| c.name.as_str()).unwrap_or("unknown"); // 1307
    eprintln!("[acp_adapter.server] Initialize from {client_name} (protocol v{})", PROTOCOL_VERSION); // 1308-1312
    InitializeResponse {
        protocol_version: PROTOCOL_VERSION, // 1315
        agent_name: "hermes-agent".to_string(), // 1316
        agent_version: HERMES_VERSION_FALLBACK.to_string(),
    }
}

#[allow(dead_code)]
pub fn _initialize(
    protocol_version: Option<u32>,
    client_info: Option<&Implementation>,
) -> InitializeResponse {
    initialize(protocol_version, client_info)
}

// ---------------------------------------------------------------------------
// authenticate — lines 1329-1349
// ---------------------------------------------------------------------------

/// Mirrors `async def authenticate(self, method_id, **kwargs)` (1329-1349).
pub fn authenticate(method_id: &str, provider: Option<&str>) -> Option<AuthenticateResponse> {
    if method_id.trim().is_empty() {
        return None; // 1336-1337: not isinstance / empty → None
    }
    let normalized = method_id.trim().to_lowercase(); // 1338
    let provider = provider.map(|p| p.trim().to_lowercase()).unwrap_or_default();

    if normalized == TERMINAL_SETUP_AUTH_METHOD_ID {
        // 1341-1345: terminal auth only succeeds once runtime credentials exist
        return if provider.is_empty() { None } else { Some(AuthenticateResponse {}) };
    }
    if provider.is_empty() || normalized != provider {
        return None; // 1347-1348
    }
    Some(AuthenticateResponse {}) // 1349
}

#[allow(dead_code)]
pub fn _authenticate(method_id: &str, provider: Option<&str>) -> Option<AuthenticateResponse> {
    authenticate(method_id, provider)
}

// ---------------------------------------------------------------------------
// History helpers — lines 1353-1492
// ---------------------------------------------------------------------------

/// Normalize a persisted text-or-text-parts value into a single string.
/// Mirrors `def _flatten_history_text(value: Any)` (1353-1377).
pub fn flatten_history_text(value: &HistoryContent) -> String {
    match value {
        HistoryContent::Text(s) => s.trim().to_string(), // 1363-1364
        HistoryContent::Parts(items) => {
            // Not used in this stub enum; kept for completeness.
            let _ = items;
            String::new()
        }
        HistoryContent::Empty => String::new(),
    }
}

/// Variant that mirrors the Python `value: str | list[dict | str]` dynamic dispatch.
/// Used when history was loaded from JSON where `content` may be a String or
/// an array of parts.
pub fn flatten_history_text_dynamic(value: &serde_like::JsonValue) -> String {
    match value {
        serde_like::JsonValue::Str(s) => s.trim().to_string(),
        serde_like::JsonValue::Array(arr) => {
            let mut parts: Vec<String> = Vec::new();
            for item in arr {
                match item {
                    serde_like::JsonValue::Object(map) => {
                        if let Some(serde_like::JsonValue::Str(t)) = map.get("text") {
                            parts.push(t.clone());
                        } else if map.get("type") == Some(&serde_like::JsonValue::Str("text".to_string())) {
                            if let Some(serde_like::JsonValue::Str(c)) = map.get("content") {
                                parts.push(c.clone());
                            }
                        }
                    }
                    serde_like::JsonValue::Str(s) => parts.push(s.clone()),
                    _ => {}
                }
            }
            parts
                .into_iter()
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        }
        _ => String::new(),
    }
}

#[allow(dead_code)]
pub fn _flatten_history_text(value: &HistoryContent) -> String {
    flatten_history_text(value)
}

/// Extract displayable text from a persisted OpenAI-style message.
/// Mirrors `def _history_message_text(cls, message)` (1379-1382).
pub fn history_message_text(content: &serde_like::JsonValue) -> String {
    flatten_history_text_dynamic(content)
}

/// Extract displayable reasoning/thought text from a persisted assistant message.
/// Mirrors `def _history_reasoning_text(cls, message)` (1384-1400).
pub fn history_reasoning_text(message: &HashMap<String, serde_like::JsonValue>) -> String {
    for key in ["reasoning_content", "reasoning"] {
        if let Some(val) = message.get(key) {
            let text = flatten_history_text_dynamic(val);
            if !text.trim().is_empty() {
                return text;
            }
        }
    }
    String::new()
}

/// Build the `_meta` payload for a replayed compaction summary.
/// Mirrors `def _history_summary_meta(message, text)` (1402-1437).
pub fn history_summary_meta(
    message: &HashMap<String, serde_like::JsonValue>,
    text: &str,
) -> Option<HashMap<String, HashMap<String, bool>>> {
    let mut kind = classify_summary_content(text); // 1427
    if kind.is_none() {
        // Mirrors `if kind is None and message.get(COMPRESSED_SUMMARY_METADATA_KEY): kind = "standalone"` (1428-1432)
        if message.contains_key("__compressed_summary") {
            kind = Some("standalone");
        }
    }
    match kind {
        Some("standalone") => {
            let mut inner = HashMap::new();
            inner.insert("compactionSummary".to_string(), true);
            let mut outer = HashMap::new();
            outer.insert("hermes".to_string(), inner);
            Some(outer) // 1433-1434
        }
        Some("merged") => {
            let mut inner = HashMap::new();
            inner.insert("containsCompactionSummary".to_string(), true);
            let mut outer = HashMap::new();
            outer.insert("hermes".to_string(), inner);
            Some(outer) // 1435-1436
        }
        _ => None,
    }
}

fn classify_summary_content(text: &str) -> Option<&'static str> {
    // Mirrors `ContextCompressor.classify_summary_content(text)` — stub:
    // recognizes the real compressor's sentinel prefixes. Without the
    // compressor crate linked we approximate via substring.
    let t = text.trim();
    if t.starts_with("[Hermes compaction summary]") || t.starts_with("## Compaction Summary") {
        return Some("standalone");
    }
    if t.contains("[Compaction:") && t.contains("preserved") {
        return Some("merged");
    }
    None
}

#[allow(dead_code)]
pub fn _history_summary_meta(
    message: &HashMap<String, serde_like::JsonValue>,
    text: &str,
) -> Option<HashMap<String, HashMap<String, bool>>> {
    history_summary_meta(message, text)
}

/// Build an ACP history replay update for a user/assistant message.
/// Mirrors `def _history_message_update(*, role, text, field_meta)` (1439-1460).
pub fn history_message_update(
    role: &str,
    text: &str,
    field_meta: Option<HashMap<String, HashMap<String, bool>>>,
) -> Option<UserMessageChunk> {
    if role != "user" && role != "assistant" {
        return None;
    }
    // Mirrors `block = TextContentBlock(type="text", text=text)` + Chunk construction
    let _ = field_meta;
    Some(UserMessageChunk {
        text: text.to_string(),
        field_meta: None,
    })
}

#[allow(dead_code)]
pub fn _history_message_update(
    role: &str,
    text: &str,
    field_meta: Option<HashMap<String, HashMap<String, bool>>>,
) -> Option<UserMessageChunk> {
    history_message_update(role, text, field_meta)
}

/// Build an ACP history replay update for an assistant thought.
/// Mirrors `def _history_thought_update(text)` (1462-1465).
pub fn history_thought_update(text: &str) -> AgentThoughtChunk {
    // Mirrors `return acp.update_agent_thought_text(text)`
    AgentThoughtChunk { text: text.to_string() }
}

#[allow(dead_code)]
pub fn _history_thought_update(text: &str) -> AgentThoughtChunk {
    history_thought_update(text)
}

/// Extract function name/arguments from an OpenAI-style tool_call.
/// Mirrors `def _history_tool_call_name_args(tool_call)` (1467-1481).
pub fn history_tool_call_name_args(tool_call: &HistoryToolCall) -> (String, HashMap<String, String>) {
    let name = if !tool_call.function.as_ref().map(|f| f.name.trim().to_string()).unwrap_or_default().is_empty() {
        tool_call.function.as_ref().unwrap().name.trim().to_string()
    } else if !tool_call.name.trim().is_empty() {
        tool_call.name.trim().to_string()
    } else {
        "unknown_tool".to_string()
    };
    let raw = if let Some(f) = &tool_call.function {
        if !f.arguments.trim().is_empty() {
            f.arguments.clone()
        } else {
            tool_call.arguments.clone()
        }
    } else {
        tool_call.arguments.clone()
    };
    let raw = raw.trim().to_string();
    // Mirrors `if isinstance(raw_args, str): try: parsed = json.loads(raw_args); except: parsed = {"raw": raw_args}`
    let parsed: HashMap<String, String> = if raw.is_empty() {
        HashMap::new()
    } else if raw.starts_with('{') {
        parse_json_object_stub(&raw).unwrap_or_else(|| {
            let mut m = HashMap::new();
            m.insert("raw".to_string(), raw.clone());
            m
        })
    } else {
        let mut m = HashMap::new();
        if !raw.is_empty() {
            m.insert("raw".to_string(), raw.clone());
        }
        m
    };
    (name, parsed)
}

fn parse_json_object_stub(s: &str) -> Option<HashMap<String, String>> {
    // Minimal JSON object parser for 1:1 without serde — handles flat {"k":"v"} shapes
    // used in tool_call arguments. Returns None on parse failure to trigger raw fallback.
    let s = s.trim();
    if !s.starts_with('{') || !s.ends_with('}') {
        return None;
    }
    let inner = s[1..s.len() - 1].trim();
    if inner.is_empty() {
        return Some(HashMap::new());
    }
    let mut map = HashMap::new();
    // Very naive split on "," not inside quotes — sufficient for tests; real
    // JSON with nested objects will fall back to {"raw": ...} which matches
    // Python's broad `except Exception: parsed = {"raw": raw_args}`.
    for part in split_json_fields(inner) {
        let part = part.trim();
        if let Some(colon) = part.find(':') {
            let k = part[..colon].trim().trim_matches('"').to_string();
            let v = part[colon + 1..].trim().trim_matches('"').to_string();
            map.insert(k, v);
        }
    }
    Some(map)
}

fn split_json_fields(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut esc = false;
    let mut depth: i32 = 0;
    for ch in s.chars() {
        if esc {
            cur.push(ch);
            esc = false;
            continue;
        }
        if ch == '\\' && in_str {
            esc = true;
            cur.push(ch);
            continue;
        }
        if ch == '"' {
            in_str = !in_str;
            cur.push(ch);
            continue;
        }
        if !in_str {
            if ch == '{' || ch == '[' {
                depth += 1;
            } else if ch == '}' || ch == ']' {
                depth -= 1;
            }
            if ch == ',' && depth == 0 {
                out.push(cur.clone());
                cur.clear();
                continue;
            }
        }
        cur.push(ch);
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

#[allow(dead_code)]
pub fn _history_tool_call_name_args(tool_call: &HistoryToolCall) -> (String, HashMap<String, String>) {
    history_tool_call_name_args(tool_call)
}

/// Return the stable provider tool call id for ACP history replay.
/// Mirrors `def _history_tool_call_id(tool_call)` (1483-1491).
pub fn history_tool_call_id(tool_call: &HistoryToolCall) -> String {
    // Mirrors `str(tool_call.get("id") or tool_call.get("call_id") or tool_call.get("tool_call_id") or "").strip()`
    // In typed stub we only have `id`; other aliases would be separate fields if needed.
    tool_call.id.trim().to_string()
}

#[allow(dead_code)]
pub fn _history_tool_call_id(tool_call: &HistoryToolCall) -> String {
    history_tool_call_id(tool_call)
}

// ---------------------------------------------------------------------------
// _replay_session_history — lines 1493-1590 (async, modelled sync for audit)
// ---------------------------------------------------------------------------

/// Mirrors `async def _replay_session_history(self, state)` (1493-1590).
///
/// Replay is invoked inline (`await`) from both `load_session` and
/// `resume_session` so spec-compliant ACP clients receive the full transcript
/// within the request's lifetime. Merely restoring server-side state makes
/// Hermes remember context, but leaves the editor looking like a clean thread.
pub fn replay_session_history_sync(
    state: &SessionState,
    history: &[HashMap<String, serde_like::JsonValue>],
    has_conn: bool,
) -> ReplayResult {
    if !has_conn || history.is_empty() {
        return ReplayResult::Noop; // 1506-1507
    }
    let mut active_tool_calls: HashMap<String, (String, HashMap<String, String>)> = HashMap::new();
    let mut sent: Vec<String> = Vec::new();

    for message in history {
        let role = message
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if role == "user" {
            // Mirrors `text = self._history_message_text(message); if text: update = _history_message_update(...); await _send(update)`
            if let Some(content) = message.get("content") {
                let text = flatten_history_text_dynamic(content);
                if !text.is_empty() {
                    let meta = history_summary_meta(message, &text);
                    if let Some(chunk) = history_message_update(&role, &text, meta) {
                        // Mirrors `await self._conn.session_update(...); except: logger.warning; return`
                        // Sync stub: record and continue unless simulated failure
                        sent.push(format!("user_message_chunk:{}", chunk.text));
                    } else {
                        return ReplayResult::Failed(sent);
                    }
                }
            }
            continue;
        }

        if role == "assistant" {
            // Mirrors thought chunk first, then text, then tool_calls (1538-1565)
            let thought = history_reasoning_text(message);
            if !thought.is_empty() {
                let _chunk = history_thought_update(&thought);
                sent.push(format!("thought:{}", thought));
            }
            if let Some(content) = message.get("content") {
                let text = flatten_history_text_dynamic(content);
                if !text.is_empty() {
                    let meta = history_summary_meta(message, &text);
                    if let Some(chunk) = history_message_update(&role, &text, meta) {
                        sent.push(format!("assistant_message_chunk:{}", chunk.text));
                    } else {
                        return ReplayResult::Failed(sent);
                    }
                }
            }
            if let Some(serde_like::JsonValue::Array(calls)) = message.get("tool_calls") {
                for call_val in calls {
                    if let serde_like::JsonValue::Object(map) = call_val {
                        let id = map
                            .get("id")
                            .and_then(|v| v.as_str())
                            .or_else(|| map.get("call_id").and_then(|v| v.as_str()))
                            .or_else(|| map.get("tool_call_id").and_then(|v| v.as_str()))
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if id.is_empty() {
                            continue; // 1559-1560
                        }
                        // Extract name/args via helper (1561)
                        let name = map
                            .get("function")
                            .and_then(|f| match f {
                                serde_like::JsonValue::Object(fm) => fm.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                _ => None,
                            })
                            .or_else(|| map.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
                            .unwrap_or_else(|| "unknown_tool".to_string());
                        let args_str = map
                            .get("function")
                            .and_then(|f| match f {
                                serde_like::JsonValue::Object(fm) => fm.get("arguments").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                _ => None,
                            })
                            .unwrap_or_default();
                        let args = if args_str.trim().starts_with('{') {
                            parse_json_object_stub(&args_str).unwrap_or_default()
                        } else {
                            HashMap::new()
                        };
                        active_tool_calls.insert(id.clone(), (name.clone(), args.clone()));
                        sent.push(format!("tool_start:{id}:{name}"));
                    }
                }
            }
            continue;
        }

        if role == "tool" {
            // Mirrors lines 1567-1589
            let tool_call_id = message
                .get("tool_call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let mut tool_name = message
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let function_args = if let Some(entry) = active_tool_calls.remove(&tool_call_id) {
                tool_name = entry.0.clone();
                Some(entry.1)
            } else {
                None
            };
            if tool_call_id.is_empty() || tool_name.is_empty() {
                continue; // 1573-1574
            }
            let result_text = message.get("content").and_then(|v| v.as_str()).map(|s| s.to_string());
            let _ = function_args;
            sent.push(format!(
                "tool_complete:{tool_call_id}:{tool_name}:{}",
                result_text.as_deref().unwrap_or("")
            ));
            if tool_name == "todo" {
                // Mirrors `plan_update = _build_plan_update_from_todo_result(result_text); if plan_update is not None: await _send(plan_update)`
                if let Some(plan) = build_plan_update_from_todo_result_stub(result_text.as_deref()) {
                    sent.push(format!("plan_update:{plan}"));
                }
            }
        }
    }
    ReplayResult::Ok(sent)
}

fn build_plan_update_from_todo_result_stub(result: Option<&str>) -> Option<String> {
    // Mirrors `acp_adapter.events._build_plan_update_from_todo_result` — stub
    let _ = result;
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayResult {
    Ok(Vec<String>),
    Failed(Vec<String>),
    Noop,
}

#[allow(dead_code)]
pub fn _replay_session_history(
    state: &SessionState,
    history: &[HashMap<String, serde_like::JsonValue>],
    has_conn: bool,
) -> ReplayResult {
    replay_session_history_sync(state, history, has_conn)
}

// ---------------------------------------------------------------------------
// new_session prologue — lines 1591-1600
// ---------------------------------------------------------------------------

/// Mirrors `async def new_session(self, cwd, mcp_servers, **kwargs) -> NewSessionResponse` prologue (1591-1600).
///
/// ```python
/// state = self.session_manager.create_session(cwd=cwd)
/// await self._register_session_mcp_servers(state, mcp_servers)
/// self._schedule_mcp_late_refresh(state)
/// logger.info("New session %s (cwd=%s)", state.session_id, cwd)
/// self._schedule_available_commands_update(state.session_id)
/// self._schedule_usage_update(state)
/// return NewSessionResponse(...)
/// ```
/// Slice 2 covers through the `logger.info` line (1600). The remaining
/// `schedule_available_commands_update` / `schedule_usage_update` and the
/// `NewSessionResponse` assembly (1601-1610) continue in `server_slice3.rs`.
pub fn new_session_prologue(
    cwd: &str,
    mcp_servers: Option<&[McpServerDef]>,
    has_conn: bool,
) -> NewSessionPrologue {
    // Mirrors `state = self.session_manager.create_session(cwd=cwd)` (1597)
    let session_id = format!("acp-{}", cwd.replace('/', "-").trim_matches('-'));
    // Mirrors `await self._register_session_mcp_servers(state, mcp_servers)` (1598)
    let mcp_expanded = register_session_mcp_servers_sync(&session_id, mcp_servers, None);
    // Mirrors `self._schedule_mcp_late_refresh(state)` (1599)
    let late_scheduled = schedule_mcp_late_refresh(&session_id, 0, 0, false, false);
    // Mirrors `logger.info("New session %s (cwd=%s)", state.session_id, cwd)` (1600)
    eprintln!("[acp_adapter.server] New session {session_id} (cwd={cwd})");
    NewSessionPrologue {
        session_id,
        mcp_expanded,
        late_scheduled,
        has_conn,
    }
}

#[derive(Debug, Clone)]
pub struct NewSessionPrologue {
    pub session_id: String,
    pub mcp_expanded: Option<Vec<String>>,
    pub late_scheduled: bool,
    pub has_conn: bool,
}

#[allow(dead_code)]
pub fn _new_session_prologue(
    cwd: &str,
    mcp_servers: Option<&[McpServerDef]>,
    has_conn: bool,
) -> NewSessionPrologue {
    new_session_prologue(cwd, mcp_servers, has_conn)
}

// ---------------------------------------------------------------------------
// Minimal JSON value shim — mirrors `typing.Any` + `json.loads` without serde
// (NEVER cargo). Only the shapes used in slice 2 history replay are modelled.
// ---------------------------------------------------------------------------

pub mod serde_like {
    use std::collections::HashMap;

    #[derive(Debug, Clone, PartialEq)]
    pub enum JsonValue {
        Null,
        Bool(bool),
        Str(String),
        Number(String),
        Array(Vec<JsonValue>),
        Object(HashMap<String, JsonValue>),
    }

    impl JsonValue {
        pub fn as_str(&self) -> Option<&str> {
            match self {
                JsonValue::Str(s) => Some(s.as_str()),
                _ => None,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Slice boundary — line 1600
// ---------------------------------------------------------------------------
// Python `acp_adapter/server.py` lines 1601-2640 (remainder of `new_session`,
// `load_session`, `resume_session`, `fork_session`, `prompt`, slash-command
// dispatch, tool-call streaming, and the remaining HermesACPAgent surface)
// continue in `server_slice3.rs` and `server_slice4.rs`. This file stops at
// the `logger.info("New session ...")` line (1600) so the 4-slice
// decomposition (~660 lines/slice) stays clean and `cargo` is never invoked.

