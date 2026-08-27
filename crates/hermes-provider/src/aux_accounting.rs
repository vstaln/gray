//! Ambient session-accounting context for auxiliary LLM calls.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/aux_accounting.py` (138 lines).
//!
//! Auxiliary calls (vision, compression, title generation, web_extract,
//! session_search, ...) funnel through `agent.auxiliary_client` which has no
//! session handle — so their token usage was historically discarded, leaving
//! dashboard analytics blind to aux model spend (issue #23270).
//!
//! Instead of threading `session_db`/`session_id` parameters through every
//! aux call site, the agent loop publishes them here (mirroring the Nous Portal
//! conversation context in `agent.portal_tags`) and the auxiliary client
//! records usage at its single response-validation chokepoint.
//!
//! ContextVar semantics give us the right isolation for free:
//!
//! * concurrent agents in one process (gateway sessions, delegate subagents)
//!   never see each other's accounting context;
//! * worker threads spawned via `tools.thread_context.propagate_context_to_thread`
//!   (MoA fan-out, background review) inherit the parent turn's context;
//! * asyncio tasks inherit the context of the code that created them.
//!
//! MoA reference/aggregator slots are explicitly EXCLUDED from recording:
//! `agent/conversation_loop.py` already folds MoA advisor usage and cost into
//! the main loop's `update_token_counts` delta, so recording them here would
//! double-count (see `EXCLUDED_TASKS`).
//!
//! T0050 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs Rust idioms:
//! - Python `ContextVar[Optional[tuple]]` (`_accounting`) ↔ `thread_local!` + `RefCell<Option<AccountingContext>>`.
//!   Python's `ContextVar` is task-local (asyncio + thread propagation); Rust's `thread_local!`
//!   is thread-local. For the `propagate_context_to_thread` fan-out, callers must
//!   capture `get_accounting_context()` in the parent and re-publish with
//!   `set_accounting_context` inside the child closure — the isolation
//!   contract is the same, the inheritance must be explicit (std has no implicit
//!   thread-context propagation).
//! - Python `frozenset({"moa_reference","moa_aggregator"})` ↔ `&[&str]` const + `contains` scan.
//! - Python `session_db: Any` with `record_auxiliary_usage` ↔ `Arc<dyn SessionDb>` trait.
//! - Python `response: Any` with `getattr(response,"usage",None)` / `getattr(response,"model","")`
//!   ↔ `AuxResponse { model: Option<String>, usage: Option<Value> }` + `Value::Object` (std-only `Any` stand-in).
//! - Python `from agent.usage_pricing import estimate_usage_cost, normalize_usage` ↔
//!   local `normalize_usage` / `estimate_usage_cost` stubs mirroring `agent/usage_pricing.py` shapes;
//!   real pricing would live in a sibling `usage_pricing` crate — this slice preserves the method
//!   signatures and the best-effort `None` cost path.
//! - Python `logger.debug` ↔ `eprintln!` elided (best-effort is silent; failures never break an aux call).
//! - Python `try/except Exception` swallowing ↔ `std::panic::catch_unwind` + `Option` guards; record path is best-effort.
//! - Crate stays `std`-only — no `serde`, `tokio`, or external deps.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Logger target — mirrors `logger = logging.getLogger(__name__)` (l.33)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "aux_accounting";

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` / `Dict[str, Any]` (std-only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    Int(i64),
    String(String),
    Array(Vec<Value>),
    Object(HashMap<String, Value>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Number(n) if n.is_finite() => Some(*n as i64),
            _ => None,
        }
    }
    pub fn as_object(&self) -> Option<&HashMap<String, Value>> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

// ---------------------------------------------------------------------------
// CanonicalUsage — mirrors `agent.usage_pricing.CanonicalUsage` (usage_pricing.py ll.73-109)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CanonicalUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub request_count: i64,
}

impl CanonicalUsage {
    pub fn is_empty(&self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_tokens == 0
            && self.cache_write_tokens == 0
            && self.reasoning_tokens == 0
    }
}

// ---------------------------------------------------------------------------
// CostResult — mirrors `agent.usage_pricing.CostResult` (usage_pricing.py ll.131-141)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CostResult {
    /// `None` → unknown pricing; `Some(f64)` → estimated USD.
    pub amount_usd: Option<f64>,
    pub status: String,
    pub source: String,
    pub label: String,
}

impl CostResult {
    pub fn unknown() -> Self {
        Self {
            amount_usd: None,
            status: "unknown".to_string(),
            source: "none".to_string(),
            label: "n/a".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// SessionDb trait — mirrors `hermes_state.SessionDB.record_auxiliary_usage` (hermes_state.py ll.8811-8875)
// ---------------------------------------------------------------------------

/// Minimal trait for the ambient DB handle stored in the accounting context.
///
/// Mirrors `hermes_state.SessionDB` (`hermes_state.py:8811`) — the sole method
/// the aux path calls is `record_auxiliary_usage`. The real `SessionDB` would
/// implement this by inserting into `session_model_usage` with `task` dimension.
pub trait SessionDb: Send + Sync {
    fn record_auxiliary_usage(
        &self,
        session_id: &str,
        task: &str,
        model: &str,
        billing_provider: Option<&str>,
        billing_base_url: Option<&str>,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_write_tokens: i64,
        reasoning_tokens: i64,
        estimated_cost_usd: Option<f64>,
    );
}

// ---------------------------------------------------------------------------
// AuxResponse — mirrors `response: Any` with `response.model` / `response.usage`
// ---------------------------------------------------------------------------

/// Minimal aux response shape the recorder reads.
///
/// Mirrors `getattr(response,"model","")` (l.113) and `getattr(response,"usage",None)` (l.99).
/// `usage` may be a dict/object shape or `None` (no usage recorded).
#[derive(Debug, Clone, Default)]
pub struct AuxResponse {
    pub model: Option<String>,
    pub usage: Option<Value>,
}

impl AuxResponse {
    pub fn new(model: impl Into<String>, usage: Option<Value>) -> Self {
        let m = model.into();
        let model_opt = if m.trim().is_empty() { None } else { Some(m) };
        Self { model: model_opt, usage }
    }
    pub fn with_model(model: impl Into<String>) -> Self {
        Self::new(model, None)
    }
}

// ---------------------------------------------------------------------------
// Accounting context — mirrors `ContextVar[Optional[tuple]]` (ll.36-38)
// ---------------------------------------------------------------------------

/// Ambient accounting handle for the active agent turn.
///
/// Mirrors `_accounting: ContextVar[Optional[tuple]] = ContextVar("aux_accounting_context", default=None)` (ll.36-38).
#[derive(Clone)]
pub struct AccountingContext {
    pub db: Arc<dyn SessionDb>,
    pub session_id: String,
}

// Manual Debug: `db` has no Debug; show session_id and db pointer.
impl std::fmt::Debug for AccountingContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountingContext")
            .field("session_id", &self.session_id)
            .field("db", &format_args!("Arc<dyn SessionDb@{:p}>", Arc::as_ptr(&self.db)))
            .finish()
    }
}

/// Token returned by `set_accounting_context` for `reset_accounting_context`.
///
/// Mirrors `ContextVar.set()` token (ll.54-55). Holds the previous context so
/// `reset` can restore it. `None` means “no previous context”.
pub type AccountingToken = Option<AccountingContext>;

/// Aux tasks already accounted by the main loop — recording would double-count.
///
/// Mirrors `_EXCLUDED_TASKS = frozenset({"moa_reference","moa_aggregator"})` (l.43).
pub const EXCLUDED_TASKS: &[&str] = &["moa_reference", "moa_aggregator"];

/// Mirror of `_EXCLUDED_TASKS` as a `HashSet` helper (mirrors Python `in` check l.93).
pub fn excluded_tasks_set() -> HashSet<&'static str> {
    EXCLUDED_TASKS.iter().copied().collect()
}

fn is_excluded_task(task: &str) -> bool {
    EXCLUDED_TASKS.contains(&task)
}

thread_local! {
    /// Mirrors `_accounting: ContextVar[Optional[tuple]]` (ll.36-38) with `default=None`.
    static ACCOUNTING: RefCell<Option<AccountingContext>> = RefCell::new(None);
}

// ---------------------------------------------------------------------------
// set / reset / get — mirrors ll.46-68
// ---------------------------------------------------------------------------

/// Publish the active session's accounting handles for aux usage recording.
///
/// Called by the agent loop at turn entry. Returns the `ContextVar` token so
/// callers can `reset_accounting_context(token)` on turn exit. Publishing
/// `None` handles (no DB / no session id) clears the context.
///
/// Mirrors `def set_accounting_context(session_db: Any, session_id: Optional[str]):` (ll.46-55).
pub fn set_accounting_context(
    session_db: Option<Arc<dyn SessionDb>>,
    session_id: Option<&str>,
) -> AccountingToken {
    // Mirrors `if session_db is None or not session_id: return _accounting.set(None)` (ll.53-54)
    let new_ctx = match (session_db, session_id) {
        (Some(db), Some(sid)) if !sid.trim().is_empty() => Some(AccountingContext {
            db,
            session_id: sid.trim().to_string(),
        }),
        _ => None,
    };
    ACCOUNTING.with(|cell| {
        let prev = cell.borrow().clone();
        *cell.borrow_mut() = new_ctx;
        prev
    })
}

/// Restore the previous accounting context (pair with `set_...`).
///
/// Mirrors `def reset_accounting_context(token) -> None:` (ll.58-63).
pub fn reset_accounting_context(token: AccountingToken) {
    // Mirrors `try: _accounting.reset(token) except Exception: _accounting.set(None)` (ll.60-63)
    let res = catch_unwind(AssertUnwindSafe(|| {
        ACCOUNTING.with(|cell| {
            *cell.borrow_mut() = token.clone();
        });
    }));
    if res.is_err() {
        ACCOUNTING.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

/// Return `(session_db, session_id)` for the active turn, or `None`.
///
/// Mirrors `def get_accounting_context() -> Optional[tuple]:` (ll.66-68).
pub fn get_accounting_context() -> Option<AccountingContext> {
    ACCOUNTING.with(|cell| cell.borrow().clone())
}

// ---------------------------------------------------------------------------
// Helpers: normalize_usage / estimate_usage_cost — mirrors `agent.usage_pricing`
// ---------------------------------------------------------------------------

fn get_int(map: &HashMap<String, Value>, key: &str) -> Option<i64> {
    match map.get(key)? {
        Value::Int(i) => Some(*i),
        Value::Number(n) if n.is_finite() => Some(*n as i64),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        Value::Bool(b) => Some(if *b { 1 } else { 0 }),
        _ => None,
    }
}

fn get_int_clamped(map: &HashMap<String, Value>, key: &str) -> i64 {
    get_int(map, key).unwrap_or(0).max(0)
}

/// Normalize raw usage into canonical token buckets.
///
/// Mirrors `agent.usage_pricing.normalize_usage` (usage_pricing.py:1276-1428)
/// for the shapes the aux client actually sees (vision, compression, etc.).
/// Best-effort; never panics. Handles:
/// - Anthropic: `input_tokens`/`output_tokens`/`cache_read_input_tokens`/`cache_creation_input_tokens`
/// - OpenAI Chat Completions: `prompt_tokens`/`completion_tokens` + `prompt_tokens_details.cached_tokens`
/// - DeepSeek / Kimi / MiniMax variants: top-level `prompt_cache_hit_tokens` / `cached_tokens`
/// - Nested details: `input_tokens_details.cached_tokens`, `output_tokens_details.reasoning_tokens`, etc.
pub fn normalize_usage(raw_usage: Option<&Value>, provider: Option<&str>) -> CanonicalUsage {
    let Some(Value::Object(map)) = raw_usage else {
        return CanonicalUsage::default();
    };
    let provider_name = provider.unwrap_or("").trim().to_ascii_lowercase();

    // Anthropic fast path (mirrors `if mode == "anthropic_messages" or provider_name == "anthropic"` l.1299)
    if provider_name == "anthropic" {
        return CanonicalUsage {
            input_tokens: get_int_clamped(map, "input_tokens"),
            output_tokens: get_int_clamped(map, "output_tokens"),
            cache_read_tokens: get_int_clamped(map, "cache_read_input_tokens"),
            cache_write_tokens: get_int_clamped(map, "cache_creation_input_tokens"),
            reasoning_tokens: 0,
            request_count: 1,
        };
    }

    // Generic / OpenAI-compatible path (mirrors ll.1325-1384)
    let prompt_total = get_int(map, "prompt_tokens")
        .or_else(|| get_int(map, "input_tokens"))
        .unwrap_or(0)
        .max(0);
    let output_tokens = get_int(map, "completion_tokens")
        .or_else(|| get_int(map, "output_tokens"))
        .unwrap_or(0)
        .max(0);

    // cache_read: try nested details first, then top-level fallbacks (mirrors ll.1342-1367)
    let mut cache_read_tokens: i64 = 0;
    if let Some(Value::Object(details)) = map.get("prompt_tokens_details") {
        cache_read_tokens = get_int_clamped(details, "cached_tokens");
    }
    if cache_read_tokens == 0 {
        cache_read_tokens = get_int_clamped(map, "cache_read_input_tokens");
    }
    if cache_read_tokens == 0 {
        cache_read_tokens = get_int_clamped(map, "prompt_cache_hit_tokens");
    }
    if cache_read_tokens == 0 {
        cache_read_tokens = get_int_clamped(map, "cached_tokens");
    }
    if cache_read_tokens == 0 {
        if let Some(Value::Object(details)) = map.get("input_tokens_details") {
            cache_read_tokens = get_int_clamped(details, "cached_tokens");
        }
    }

    let mut cache_write_tokens: i64 = 0;
    if let Some(Value::Object(details)) = map.get("prompt_tokens_details") {
        cache_write_tokens = get_int(details, "cache_write_tokens")
            .or_else(|| get_int(details, "cache_creation_input_tokens"))
            .unwrap_or(0)
            .max(0);
    }
    if cache_write_tokens == 0 {
        cache_write_tokens = get_int_clamped(map, "cache_creation_input_tokens");
    }
    if cache_write_tokens == 0 {
        cache_write_tokens = get_int_clamped(map, "cache_write_tokens");
    }
    if cache_write_tokens == 0 {
        if let Some(Value::Object(details)) = map.get("input_tokens_details") {
            cache_write_tokens = get_int(details, "cache_write_tokens")
                .or_else(|| get_int(details, "cache_creation_tokens"))
                .unwrap_or(0)
                .max(0);
        }
    }

    // input_tokens derived by subtracting cache tokens (mirrors l.1384)
    let has_prompt = map.contains_key("prompt_tokens");
    let has_input = map.contains_key("input_tokens");
    let mut input_tokens = if has_prompt && !has_input {
        (prompt_total - cache_read_tokens - cache_write_tokens).max(0)
    } else if has_input && !has_prompt {
        // codex shape already handled above as anthropic? For cross-compat, keep input as-is minus caches if nested present
        let input_total = get_int_clamped(map, "input_tokens");
        if cache_read_tokens > 0 || cache_write_tokens > 0 {
            // only subtract if the raw total likely includes caches (codex/openai contract)
            // mirrors the `input_tokens = max(0, input_total - cache_read - cache_write)` (l.1324/l.1384)
            // but only when details were present
            let has_details = map.contains_key("input_tokens_details") || map.contains_key("prompt_tokens_details");
            if has_details {
                (input_total - cache_read_tokens - cache_write_tokens).max(0)
            } else {
                input_total
            }
        } else {
            input_total
        }
    } else {
        // fallback: prefer explicit input_tokens if both present, else prompt-derived
        get_int(map, "input_tokens").unwrap_or(prompt_total - cache_read_tokens - cache_write_tokens).max(0)
    };
    input_tokens = input_tokens.max(0);

    // reasoning_tokens (mirrors ll.1386-1402)
    let mut reasoning_tokens: i64 = 0;
    if let Some(Value::Object(details)) = map.get("output_tokens_details") {
        reasoning_tokens = get_int_clamped(details, "reasoning_tokens");
    }
    if reasoning_tokens == 0 {
        if let Some(Value::Object(details)) = map.get("completion_tokens_details") {
            reasoning_tokens = get_int_clamped(details, "reasoning_tokens");
        }
    }
    // top-level fallback (some providers emit reasoning_tokens at Usage root)
    if reasoning_tokens == 0 {
        reasoning_tokens = get_int_clamped(map, "reasoning_tokens");
    }

    CanonicalUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        reasoning_tokens,
        request_count: 1,
    }
}

/// Estimate USD cost for a usage bucket.
///
/// Mirrors `agent.usage_pricing.estimate_usage_cost` (usage_pricing.py:1431-1509).
/// This slice is std-only with no pricing table — it returns `unknown` (amount `None`)
/// so callers store `estimated_cost_usd = None` and analytics still gets token counts.
/// A future `usage_pricing` crate would replace this with the real snapshot + models-API lookup.
pub fn estimate_usage_cost(
    _model: &str,
    _usage: &CanonicalUsage,
    _provider: Option<&str>,
    _base_url: Option<&str>,
) -> CostResult {
    // Subscription-included fast path would be checked via `resolve_billing_route`
    // in the real impl; here we preserve the best-effort contract: any failure
    // is swallowed and cost is simply not stored (mirrors aux_accounting.py:115-122).
    CostResult::unknown()
}

// Back-compat aliases mirroring private names (line-level audit)
#[allow(dead_code)]
fn _normalize_usage(raw: Option<&Value>, provider: Option<&str>) -> CanonicalUsage {
    normalize_usage(raw, provider)
}
#[allow(dead_code)]
fn _estimate_usage_cost(m: &str, u: &CanonicalUsage, p: Option<&str>, b: Option<&str>) -> CostResult {
    estimate_usage_cost(m, u, p, b)
}

// ---------------------------------------------------------------------------
// record_aux_usage — mirrors ll.71-138
// ---------------------------------------------------------------------------

/// Record an auxiliary response's token usage against the ambient session.
///
/// Called from the auxiliary client's response-validation chokepoint. Strictly
/// best-effort: any failure is swallowed (accounting must never break an aux
/// call). No-ops when:
///
/// * no accounting context is published (call is outside any agent turn),
/// * the task is main-loop-accounted (MoA slots — see `EXCLUDED_TASKS`),
/// * the response carries no usage object.
///
/// The model is read from `response.model` (accurate even after the aux
/// client's provider-fallback chains); *provider*/*base_url* reflect the
/// originally-resolved route and are best-effort.
///
/// Mirrors `def record_aux_usage(response: Any, task: Optional[str], *, provider: Optional[str]=None, base_url: Optional[str]=None) -> None:` (ll.71-138).
pub fn record_aux_usage(
    response: &AuxResponse,
    task: Option<&str>,
    provider: Option<&str>,
    base_url: Option<&str>,
) {
    // Outer best-effort guard — mirrors `try: ... except Exception: logger.debug(..., exc_info=True)` (l.92 / l.137)
    let _ = catch_unwind(AssertUnwindSafe(|| {
        record_aux_usage_inner(response, task, provider, base_url);
    }));
    // If the closure panicked, we swallow it — accounting must never break an aux call.
}

fn record_aux_usage_inner(
    response: &AuxResponse,
    task: Option<&str>,
    provider: Option<&str>,
    base_url: Option<&str>,
) {
    // Mirrors `if not task or task in _EXCLUDED_TASKS: return` (ll.93-94)
    let task_str = match task {
        Some(t) if !t.trim().is_empty() => t.trim(),
        _ => return,
    };
    if is_excluded_task(task_str) {
        return;
    }

    // Mirrors `ctx = _accounting.get(); if ctx is None: return; session_db, session_id = ctx` (ll.95-98)
    let ctx = match get_accounting_context() {
        Some(c) => c,
        None => return,
    };
    let session_id = ctx.session_id.clone();
    // Clone Arc to keep it alive outside thread_local borrow
    let db: Arc<dyn SessionDb> = Arc::clone(&ctx.db);

    // Mirrors `raw_usage = getattr(response, "usage", None); if raw_usage is None: return` (ll.99-101)
    let raw_usage = match response.usage.as_ref() {
        Some(u) => u,
        None => return,
    };

    // Mirrors `from agent.usage_pricing import estimate_usage_cost, normalize_usage` + `usage = normalize_usage(raw_usage, provider=provider)` (ll.103-105)
    // Keep the import inside the try so failures are swallowed (best-effort).
    let usage = {
        let r = catch_unwind(AssertUnwindSafe(|| normalize_usage(Some(raw_usage), provider)));
        match r {
            Ok(u) => u,
            Err(_) => return,
        }
    };

    // Mirrors `if not (usage.input_tokens or ...): return` (ll.106-111)
    if usage.is_empty() {
        return;
    }

    // Mirrors `model = str(getattr(response, "model", "") or "") or "unknown"` (l.113)
    let model = response
        .model
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    // Mirrors `estimated_cost = None; try: cost = estimate_usage_cost(...); if cost.amount_usd is not None: estimated_cost = float(cost.amount_usd) except Exception: logger.debug(...)` (ll.114-122)
    let estimated_cost_usd: Option<f64> = {
        let r = catch_unwind(AssertUnwindSafe(|| {
            estimate_usage_cost(&model, &usage, provider, base_url)
        }));
        match r {
            Ok(cost) => cost.amount_usd,
            Err(_) => None,
        }
    };

    // Mirrors `session_db.record_auxiliary_usage(session_id, task, model=model, billing_provider=provider, billing_base_url=base_url, ...)` (ll.124-136)
    let _ = catch_unwind(AssertUnwindSafe(|| {
        db.record_auxiliary_usage(
            &session_id,
            task_str,
            &model,
            provider,
            base_url,
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_tokens,
            usage.cache_write_tokens,
            usage.reasoning_tokens,
            estimated_cost_usd,
        );
    }));
    // Swallow any panic from the DB write — mirrors outer `except Exception: logger.debug(...)` (l.137)
}

// Python-compatible alias (snake_case already matches)
#[allow(dead_code)]
fn _record_aux_usage(r: &AuxResponse, t: Option<&str>, p: Option<&str>, b: Option<&str>) {
    record_aux_usage(r, t, p, b)
}

// ---------------------------------------------------------------------------
// Helpers for tests / interop
// ---------------------------------------------------------------------------

/// Convenience: build a `Value::Object` usage for tests from token counts (mirrors SimpleNamespace usage in tests).
pub fn usage_value_from_counts(prompt: i64, completion: i64) -> Value {
    let mut m = HashMap::new();
    m.insert("prompt_tokens".to_string(), Value::Int(prompt));
    m.insert("completion_tokens".to_string(), Value::Int(completion));
    Value::Object(m)
}

pub fn usage_value_full(
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
) -> Value {
    let mut m = HashMap::new();
    m.insert("input_tokens".to_string(), Value::Int(input));
    m.insert("output_tokens".to_string(), Value::Int(output));
    m.insert("cache_read_input_tokens".to_string(), Value::Int(cache_read));
    m.insert("cache_creation_input_tokens".to_string(), Value::Int(cache_write));
    if reasoning != 0 {
        m.insert("reasoning_tokens".to_string(), Value::Int(reasoning));
    }
    Value::Object(m)
}

// ---------------------------------------------------------------------------
// Tests — mirrors `tests/hermes_state/test_aux_usage_accounting.py` (abridged)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct MockDb {
        records: Mutex<Vec<Record>>,
    }

    #[derive(Debug, Clone, PartialEq)]
    struct Record {
        session_id: String,
        task: String,
        model: String,
        billing_provider: Option<String>,
        billing_base_url: Option<String>,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_write_tokens: i64,
        reasoning_tokens: i64,
        estimated_cost_usd: Option<f64>,
    }

    impl SessionDb for MockDb {
        fn record_auxiliary_usage(
            &self,
            session_id: &str,
            task: &str,
            model: &str,
            billing_provider: Option<&str>,
            billing_base_url: Option<&str>,
            input_tokens: i64,
            output_tokens: i64,
            cache_read_tokens: i64,
            cache_write_tokens: i64,
            reasoning_tokens: i64,
            estimated_cost_usd: Option<f64>,
        ) {
            self.records.lock().unwrap().push(Record {
                session_id: session_id.to_string(),
                task: task.to_string(),
                model: model.to_string(),
                billing_provider: billing_provider.map(|s| s.to_string()),
                billing_base_url: billing_base_url.map(|s| s.to_string()),
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                reasoning_tokens,
                estimated_cost_usd,
            });
        }
    }

    fn mk_response(model: &str, prompt: i64, completion: i64) -> AuxResponse {
        AuxResponse {
            model: Some(model.to_string()),
            usage: Some(usage_value_from_counts(prompt, completion)),
        }
    }

    #[test]
    fn set_and_get_context_roundtrip() {
        let db: Arc<dyn SessionDb> = Arc::new(MockDb::default());
        // Ensure clean start
        let _ = set_accounting_context(None, None);
        assert!(get_accounting_context().is_none());

        let tok = set_accounting_context(Some(Arc::clone(&db)), Some("s1"));
        assert!(tok.is_none()); // previous was None
        let ctx = get_accounting_context().expect("should be set");
        assert_eq!(ctx.session_id, "s1");

        // Overwrite
        let tok2 = set_accounting_context(Some(Arc::clone(&db)), Some("s2"));
        assert_eq!(tok2.unwrap().session_id, "s1");
        assert_eq!(get_accounting_context().unwrap().session_id, "s2");

        // Reset to tok2 (which held s1)
        reset_accounting_context(tok2);
        assert_eq!(get_accounting_context().unwrap().session_id, "s1");

        // Clearing via None
        let _ = set_accounting_context(None, None);
        assert!(get_accounting_context().is_none());
        // Reset to previous (should restore s1)
        reset_accounting_context(tok);
        assert!(get_accounting_context().is_none()); // tok was None → None

        let _ = set_accounting_context(None, None);
    }

    #[test]
    fn set_clears_on_empty_session_id() {
        let db: Arc<dyn SessionDb> = Arc::new(MockDb::default());
        let _ = set_accounting_context(None, None);
        let tok = set_accounting_context(Some(Arc::clone(&db)), Some(""));
        assert!(get_accounting_context().is_none());
        reset_accounting_context(tok);
        let tok2 = set_accounting_context(Some(Arc::clone(&db)), None);
        assert!(get_accounting_context().is_none());
        reset_accounting_context(tok2);
        let _ = set_accounting_context(None, None);
    }

    #[test]
    fn record_writes_through_context() {
        let db = Arc::new(MockDb::default());
        let db_trait: Arc<dyn SessionDb> = db.clone();
        let _ = set_accounting_context(None, None);
        let tok = set_accounting_context(Some(db_trait), Some("s1"));
        let resp = mk_response("aux-m", 100, 20);
        record_aux_usage(&resp, Some("vision"), Some("gemini"), None);
        reset_accounting_context(tok);
        let _ = set_accounting_context(None, None);

        let recs = db.records.lock().unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].task, "vision");
        assert_eq!(recs[0].model, "aux-m");
        assert_eq!(recs[0].input_tokens, 100);
        assert_eq!(recs[0].output_tokens, 20);
        assert_eq!(recs[0].billing_provider.as_deref(), Some("gemini"));
    }

    #[test]
    fn moa_tasks_excluded() {
        let db = Arc::new(MockDb::default());
        let db_trait: Arc<dyn SessionDb> = db.clone();
        let _ = set_accounting_context(None, None);
        let tok = set_accounting_context(Some(db_trait), Some("s1"));
        record_aux_usage(&mk_response("m", 10, 5), Some("moa_reference"), None, None);
        record_aux_usage(&mk_response("m", 10, 5), Some("moa_aggregator"), None, None);
        reset_accounting_context(tok);
        let _ = set_accounting_context(None, None);
        assert!(db.records.lock().unwrap().is_empty());
    }

    #[test]
    fn no_context_is_noop() {
        let _ = set_accounting_context(None, None);
        // Should not panic even with no context
        record_aux_usage(&mk_response("m", 10, 5), Some("vision"), None, None);
        // also excluded task without context
        record_aux_usage(&mk_response("m", 10, 5), Some("moa_reference"), None, None);
    }

    #[test]
    fn no_usage_is_noop() {
        let db = Arc::new(MockDb::default());
        let db_trait: Arc<dyn SessionDb> = db.clone();
        let _ = set_accounting_context(None, None);
        let tok = set_accounting_context(Some(db_trait), Some("s1"));
        let resp = AuxResponse { model: Some("m".to_string()), usage: None };
        record_aux_usage(&resp, Some("vision"), None, None);
        reset_accounting_context(tok);
        let _ = set_accounting_context(None, None);
        assert!(db.records.lock().unwrap().is_empty());
    }

    #[test]
    fn zero_tokens_is_noop() {
        let db = Arc::new(MockDb::default());
        let db_trait: Arc<dyn SessionDb> = db.clone();
        let _ = set_accounting_context(None, None);
        let tok = set_accounting_context(Some(db_trait), Some("s1"));
        let mut m = HashMap::new();
        m.insert("prompt_tokens".to_string(), Value::Int(0));
        m.insert("completion_tokens".to_string(), Value::Int(0));
        let resp = AuxResponse { model: Some("m".to_string()), usage: Some(Value::Object(m)) };
        record_aux_usage(&resp, Some("vision"), None, None);
        reset_accounting_context(tok);
        let _ = set_accounting_context(None, None);
        assert!(db.records.lock().unwrap().is_empty());
    }

    #[test]
    fn unknown_model_when_empty() {
        let db = Arc::new(MockDb::default());
        let db_trait: Arc<dyn SessionDb> = db.clone();
        let _ = set_accounting_context(None, None);
        let tok = set_accounting_context(Some(db_trait), Some("s1"));
        let resp = AuxResponse { model: Some("".to_string()), usage: Some(usage_value_from_counts(10, 2)) };
        record_aux_usage(&resp, Some("vision"), None, None);
        reset_accounting_context(tok);
        let _ = set_accounting_context(None, None);
        let recs = db.records.lock().unwrap();
        assert_eq!(recs[0].model, "unknown");
    }

    #[test]
    fn normalize_handles_anthropic_shape() {
        let v = usage_value_full(500, 50, 10, 5, 0);
        let u = normalize_usage(Some(&v), Some("anthropic"));
        assert_eq!(u.input_tokens, 500);
        assert_eq!(u.cache_read_tokens, 10);
        assert_eq!(u.cache_write_tokens, 5);
    }

    #[test]
    fn excluded_tasks_constant() {
        assert!(EXCLUDED_TASKS.contains(&"moa_reference"));
        assert!(EXCLUDED_TASKS.contains(&"moa_aggregator"));
        assert_eq!(EXCLUDED_TASKS.len(), 2);
        assert!(!EXCLUDED_TASKS.contains(&"vision"));
    }
}
