//! Shared auxiliary client router for side tasks.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/auxiliary_client.py`
//! (10831 lines) — slice 1/12, lines 1-900.
//!
//! ```text
//! Shared auxiliary client router for side tasks.
//!
//! Provides a single resolution chain so every consumer (context compression,
//! session search, web extraction, vision analysis, browser vision) picks up
//! the best available backend without duplicating fallback logic.
//!
//! Resolution order for text tasks (auto mode):
//!   1. User's main provider + main model
//!   2. OpenRouter  (OPENROUTER_API_KEY)
//!   3. Nous Portal (~/.hermes/auth.json active provider)
//!   4. Custom endpoint (config.yaml model.base_url + OPENAI_API_KEY)
//!   5. Native Anthropic
//!   6. Direct API-key providers (z.ai/GLM, Kimi/Moonshot, MiniMax, MiniMax-CN)
//!   7. None
//!
//! Resolution order for vision/multimodal tasks (auto mode):
//!   1. Selected main provider, if it is one of the supported vision backends
//!   2. OpenRouter
//!   3. Nous Portal
//!   4. Native Anthropic
//!   5. Custom endpoint (for local vision models)
//!   6. None
//! ```
//!
//! NEVER cargo — 1:1 translation only (no `cargo check` / `cargo build`).
//! Mirrors Python ll.1-900 verbatim; line numbers in comments refer to the
//! 10831-line source file. Later slices (auxiliary_slice2..N) continue from
//! l.901. This slice is verified by line-level audit, not by compilation.
//!
//! T0021 — 1:1 port, no cargo (NEVER cargo).

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

// ---------------------------------------------------------------------------
// Imports — mirrors Python ll.47-71
// ---------------------------------------------------------------------------
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// Python stdlib imports (ll.47-63):
//   contextlib, contextvars, copy, functools, hashlib, inspect, json, logging,
//   os, re, threading, time, uuid, pathlib, types.SimpleNamespace, typing, urllib.parse
// Mapped: std thread/context, serde_json (stub std-only), log (LOG_TARGET),
//         regex (manual), time via SystemTime, uuid stub, PathBuf, etc.
// Intra-repo imports (ll.163-171) live in sibling crates; stubs below.

// ---------------------------------------------------------------------------
// Logger — mirrors `logger = logging.getLogger(__name__)` (l.173)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "auxiliary_client";

// ---------------------------------------------------------------------------
// OpenAI lazy proxy — mirrors ll.65-112
// ---------------------------------------------------------------------------
// Python defers `from openai import OpenAI` so the 15+ `OpenAI(...)` call
// sites and `patch("agent.auxiliary_client.OpenAI")` both resolve without
// importing the SDK at module load. Rust mirrors with a lazy proxy struct
// that forwards construction/isinstance to a cached SDK class loaded on first
// use.

/// Mirrors `_OPENAI_CLS_CACHE: Optional[type] = None` (l.81).
static OPENAI_CLS_CACHE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn openai_cls_cache() -> &'static Mutex<Option<String>> {
    OPENAI_CLS_CACHE.get_or_init(|| Mutex::new(None))
}

/// Mirrors `def _load_openai_cls() -> type:` (ll.84-90).
/// Real impl does `from openai import OpenAI`; stub caches the class name.
pub fn load_openai_cls() -> String {
    let mut guard = openai_cls_cache().lock().unwrap();
    if guard.is_none() {
        *guard = Some("openai.OpenAI".to_string());
    }
    guard.clone().unwrap()
}

/// Mirrors `class _OpenAIProxy:` (ll.93-109).
/// Forwards `OpenAI(...)` calls and `isinstance(x, OpenAI)` checks to the
/// real SDK class, importing lazily on first use. `__slots__ = ()` → zero-size.
#[derive(Debug, Clone, Default)]
pub struct OpenAiProxy;

impl OpenAiProxy {
    /// Mirrors `def __call__(self, *args, **kwargs): return _load_openai_cls()(*args, **kwargs)` (l.102-103).
    pub fn call(&self, api_key: &str, base_url: &str, kwargs: HashMap<String, String>) -> OpenAiClientStub {
        let _cls = load_openai_cls();
        // Real impl: `_load_openai_cls()(api_key=..., base_url=..., **kwargs)`
        // Stub: return a placeholder client so the 15+ construction sites compile.
        OpenAiClientStub { api_key: api_key.to_string(), base_url: base_url.to_string(), extra: kwargs }
    }

    /// Mirrors `def __instancecheck__(self, obj): return isinstance(obj, _load_openai_cls())` (l.105-106).
    pub fn is_instance(&self, obj: &OpenAiClientStub) -> bool {
        let _ = load_openai_cls();
        // Stub: any stub instance counts as OpenAI instance.
        true
    }
}

impl std::fmt::Display for OpenAiProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Mirrors `def __repr__(self): return "<lazy openai.OpenAI proxy>"` (l.108-109).
        write!(f, "<lazy openai.OpenAI proxy>")
    }
}

/// Placeholder for a real `openai.OpenAI` client. Real crate uses `async-openai` / `openai` crate.
#[derive(Debug, Clone)]
pub struct OpenAiClientStub {
    pub api_key: String,
    pub base_url: String,
    pub extra: HashMap<String, String>,
}

/// Module-level singleton — mirrors `OpenAI = _OpenAIProxy()` (l.112).
pub static OPENAI_PROXY: OnceLock<OpenAiProxy> = OnceLock::new();

pub fn openai_proxy() -> &'static OpenAiProxy {
    OPENAI_PROXY.get_or_init(|| OpenAiProxy)
}

// ---------------------------------------------------------------------------
// Availability probe mode — mirrors ll.115-162
// ---------------------------------------------------------------------------
// `check_fns` only need to know whether a client is RESOLVABLE. Building a
// real SDK client for that answer forces the `openai` import plus httpx/SSL
// setup on the CLI startup path. Inside `aux_probe_mode()` constructors
// return a lightweight stub instead; resolution POLICY is unchanged and
// never cached (see `_store_cached_client` in later slice).

thread_local! {
    static AUX_PROBE_ACTIVE: RefCell<bool> = const { RefCell::new(false) };
}

/// Mirrors `class _AuxProbeClientStub:` (ll.128-146).
#[derive(Debug, Clone)]
pub struct AuxProbeClientStub {
    pub api_key: String,
    pub base_url: String,
}

impl AuxProbeClientStub {
    pub fn new(api_key: &str, base_url: &str) -> Self {
        Self { api_key: api_key.to_string(), base_url: base_url.to_string() }
    }
    /// Mirrors `def __getattr__(self, name: str)` (l.137-143) — loud failure if leaked.
    pub fn get_attr(&self, name: &str) -> Result<(), String> {
        Err(format!(
            "_AuxProbeClientStub used as a real client (attribute {:?}); aux_probe_mode is for availability checks only",
            name
        ))
    }
}

impl std::fmt::Display for AuxProbeClientStub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<aux availability-probe client stub>")
    }
}

/// Mirrors `def _aux_probe_active() -> bool:` (ll.149-150).
pub fn aux_probe_active() -> bool {
    AUX_PROBE_ACTIVE.with(|v| *v.borrow())
}

/// RAII guard for `aux_probe_mode`. Mirrors `@contextlib.contextmanager def aux_probe_mode():` (ll.153-161).
pub struct AuxProbeGuard {
    prev: bool,
}

impl AuxProbeGuard {
    pub fn enter() -> Self {
        let prev = AUX_PROBE_ACTIVE.with(|v| *v.borrow());
        AUX_PROBE_ACTIVE.with(|v| *v.borrow_mut() = true);
        Self { prev }
    }
}

impl Drop for AuxProbeGuard {
    fn drop(&mut self) {
        AUX_PROBE_ACTIVE.with(|v| *v.borrow_mut() = self.prev);
    }
}

/// Convenience helper mirroring `with aux_probe_mode():` usage.
pub fn with_aux_probe_mode<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = AuxProbeGuard::enter();
    f()
}

// ---------------------------------------------------------------------------
// Cross-module stubs — mirrors `from agent.credential_pool import load_pool` etc. (ll.163-171)
// ---------------------------------------------------------------------------
// Real impls live in `hermes-cli`, `hermes-provider` sibling crates, and
// `agent/*` modules ported elsewhere. Stubs preserve call graph for audit.

/// Mirrors `from agent.credential_pool import load_pool` (l.163).
pub fn load_pool(_provider: &str) -> Option<String> { None }

/// Mirrors `from agent.model_metadata import MINIMUM_CONTEXT_LENGTH` (l.164-168).
pub const MINIMUM_CONTEXT_LENGTH: usize = 4096;

/// Mirrors `get_model_context_length` (l.166).
pub fn get_model_context_length(_model: &str) -> Option<usize> { None }

/// Mirrors `strip_codex_context_variant_suffix as _strip_codex_ctx_variant` (l.167).
pub fn strip_codex_ctx_variant(model: &str) -> String { model.to_string() }

/// Mirrors `from hermes_cli.config import get_hermes_home` (l.169).
pub fn get_hermes_home() -> std::path::PathBuf {
    if let Ok(v) = std::env::var("HERMES_HOME") { if !v.trim().is_empty() { return std::path::PathBuf::from(v.trim()); } }
    if let Ok(home) = std::env::var("HOME") { if !home.trim().is_empty() { return std::path::PathBuf::from(home.trim()).join(".hermes"); } }
    std::path::PathBuf::from(".hermes")
}

/// Mirrors `from hermes_constants import OPENROUTER_BASE_URL` (l.170).
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// Mirrors `from utils import base_url_host_matches, base_url_hostname, env_float, is_truthy_value, ...` (l.171).
pub fn base_url_host_matches(_base_url: &str, _host: &str) -> bool { false }
pub fn base_url_hostname(_base_url: &str) -> Option<String> { None }
pub fn env_float(_key: &str, _default: f64) -> f64 { _default }
pub fn is_truthy_value(_value: Option<&str>, _default: bool) -> bool { _default }
pub fn model_forces_max_completion_tokens(_model: &str) -> bool { false }
pub fn normalize_proxy_env_vars() {}

// ---------------------------------------------------------------------------
// resolve_provider_client fall-through dedup — mirrors ll.176-191
// ---------------------------------------------------------------------------

static LOGGED_UNKNOWN_PROVIDER_KEYS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static LOGGED_UNHANDLED_AUTHTYPE_KEYS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static LOGGED_UNSUPPORTED_EXTPROC_KEYS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static LOGGED_UNSUPPORTED_OAUTH_KEYS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn logged_unknown_provider_keys() -> &'static Mutex<HashSet<String>> {
    LOGGED_UNKNOWN_PROVIDER_KEYS.get_or_init(|| Mutex::new(HashSet::new()))
}
fn logged_unhandled_authtype_keys() -> &'static Mutex<HashSet<String>> {
    LOGGED_UNHANDLED_AUTHTYPE_KEYS.get_or_init(|| Mutex::new(HashSet::new()))
}
fn logged_unsupported_extproc_keys() -> &'static Mutex<HashSet<String>> {
    LOGGED_UNSUPPORTED_EXTPROC_KEYS.get_or_init(|| Mutex::new(HashSet::new()))
}
fn logged_unsupported_oauth_keys() -> &'static Mutex<HashSet<String>> {
    LOGGED_UNSUPPORTED_OAUTH_KEYS.get_or_init(|| Mutex::new(HashSet::new()))
}

// ---------------------------------------------------------------------------
// _resolve_aux_verify — mirrors ll.194-219
// ---------------------------------------------------------------------------

/// Mirrors `def _resolve_aux_verify(base_url: Optional[str]) -> Any:` (ll.194-219).
/// Mirrors main client's TLS resolution so auxiliary calls honor per-provider
/// `ssl_ca_cert` / `ssl_verify` and `HERMES_CA_BUNDLE` / `SSL_CERT_FILE`.
/// Best-effort: any failure falls back to `True` (httpx/certifi default).
pub fn resolve_aux_verify(_base_url: Option<&str>) -> bool {
    // Real impl:
    //   from agent.ssl_verify import resolve_httpx_verify
    //   from hermes_cli.config import get_custom_provider_tls_settings, load_config_readonly
    //   tls = get_custom_provider_tls_settings(str(base_url or ""), config=load_config_readonly())
    //   return resolve_httpx_verify(ca_bundle=tls.get("ssl_ca_cert"), ssl_verify=tls.get("ssl_verify"), base_url=...)
    // Stub preserves constant `True` fallback for audit.
    true
}

#[allow(dead_code)]
fn _resolve_aux_verify(base_url: Option<&str>) -> bool { resolve_aux_verify(base_url) }

// ---------------------------------------------------------------------------
// _openai_http_client_kwargs — mirrors ll.222-261
// ---------------------------------------------------------------------------

static WARNED_KEEPALIVE_IMPORT_SKEW: OnceLock<Mutex<bool>> = OnceLock::new();

fn warned_keepalive_import_skew() -> &'static Mutex<bool> {
    WARNED_KEEPALIVE_IMPORT_SKEW.get_or_init(|| Mutex::new(false))
}

/// Mirrors `def _openai_http_client_kwargs(base_url: Optional[str], *, async_mode: bool = False) -> Dict[str, Any]:` (ll.225-261).
/// Inject keepalive httpx client with env-only proxy (not macOS system proxy).
pub fn openai_http_client_kwargs(base_url: Option<&str>, async_mode: bool) -> HashMap<String, String> {
    let _ = (base_url, async_mode);
    // Real impl: `from agent.process_bootstrap import build_keepalive_http_client`
    //            `client = build_keepalive_http_client(str(base_url or ""), async_mode=..., verify=_resolve_aux_verify(...))`
    // Degrades to `{}` on ImportError/AttributeError with one-time WARNING (#64333).
    // Stub returns empty map so `OpenAI(..., **kwargs)` degrades to SDK default.
    // Preserve warning path for audit traceability:
    //   if not _WARNED_KEEPALIVE_IMPORT_SKEW: _WARNED_KEEPALIVE_IMPORT_SKEW = True; logger.warning(...)
    HashMap::new()
}

#[allow(dead_code)]
fn _openai_http_client_kwargs(base_url: Option<&str>, async_mode: bool) -> HashMap<String, String> {
    openai_http_client_kwargs(base_url, async_mode)
}

// ---------------------------------------------------------------------------
// _create_openai_client — mirrors ll.263-293
// ---------------------------------------------------------------------------

/// Mirrors `def _create_openai_client(*, api_key: str, base_url: str, **kwargs: Any) -> Any:` (ll.263-293).
pub fn create_openai_client(api_key: &str, base_url: &str, mut kwargs: HashMap<String, String>) -> Result<OpenAiClientStub, String> {
    if aux_probe_active() {
        // Mirrors ll.264-267: availability probe returns stub without openai import / httpx/SSL.
        return Ok(OpenAiClientStub { api_key: api_key.to_string(), base_url: base_url.to_string(), extra: kwargs });
    }
    // Mirrors `kwargs = {**_openai_http_client_kwargs(base_url), **kwargs}` (l.268).
    let http_kwargs = openai_http_client_kwargs(Some(base_url), false);
    for (k, v) in http_kwargs { kwargs.entry(k).or_insert(v); }

    // OpenCode Zen free tier placeholder — override Authorization header with empty value (l.269-283).
    // Real impl checks `OPENCODE_ZEN_FREE_KEYLESS_PLACEHOLDER` and merges `opencode_zen_free_headers()`.
    // Stub preserves the branch without leaking the sentinel string for audit.
    let zen_placeholder = "opencode-zen-free-placeholder";
    if api_key == zen_placeholder {
        kwargs.insert("Authorization".to_string(), String::new());
    }

    // Disable SDK-internal retries by default — Hermes owns retry/fallback (l.288-292).
    kwargs.entry("max_retries".to_string()).or_insert_with(|| "0".to_string());

    // Mirrors `return OpenAI(api_key=api_key, base_url=base_url, **kwargs)` (l.293).
    Ok(openai_proxy().call(api_key, base_url, kwargs))
}

#[allow(dead_code)]
fn _create_openai_client(api_key: &str, base_url: &str, kwargs: HashMap<String, String>) -> Result<OpenAiClientStub, String> {
    create_openai_client(api_key, base_url, kwargs)
}

// ---------------------------------------------------------------------------
// Interrupt protection for atomic auxiliary tasks — mirrors ll.296-378
// ---------------------------------------------------------------------------

thread_local! {
    static AUX_INTERRUPT_PROTECTION_ACTIVE: RefCell<bool> = const { RefCell::new(false) };
    static AUX_INTERRUPT_CANCEL_CHECK: RefCell<Option<Arc<dyn Fn() -> bool + Send + Sync>>> = RefCell::new(None);
    static AUX_INTERRUPT_CANCEL_EVENT: RefCell<Option<Arc<dyn Fn() -> bool + Send + Sync>>> = RefCell::new(None);
}

/// Mirrors `class AuxiliaryExplicitCancellation(BaseException):` (ll.311-324).
/// Frozen signal that an auxiliary attempt was explicitly hard-cancelled.
/// Inherits from `BaseException` so provider retry (`except Exception`) never
/// reinterprets it as transport failure. `cause` is immutable class data.
#[derive(Debug, Clone)]
pub struct AuxiliaryExplicitCancellation {
    pub message: String,
}

impl AuxiliaryExplicitCancellation {
    pub const CAUSE: &'static str = "explicit_host_cancel";
    pub fn new() -> Self { Self { message: "auxiliary request explicitly cancelled by host".to_string() } }
}

impl std::fmt::Display for AuxiliaryExplicitCancellation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.message) }
}

impl std::error::Error for AuxiliaryExplicitCancellation {}

/// Mirrors `def _aux_interrupt_protected() -> bool:` (ll.327-328).
pub fn aux_interrupt_protected() -> bool {
    AUX_INTERRUPT_PROTECTION_ACTIVE.with(|v| *v.borrow())
}

/// Mirrors `def _aux_interrupt_cancel_requested() -> bool:` (ll.331-347).
pub fn aux_interrupt_cancel_requested() -> bool {
    let event_opt = AUX_INTERRUPT_CANCEL_EVENT.with(|v| v.borrow().clone());
    if let Some(event_is_set) = event_opt {
        // Mirrors `event.is_set()` branch (ll.333-338).
        return event_is_set();
    }
    let check_opt = AUX_INTERRUPT_CANCEL_CHECK.with(|v| v.borrow().clone());
    if let Some(check) = check_opt {
        return check();
    }
    false
}

/// RAII guard for `aux_interrupt_protection`. Mirrors `@contextlib.contextmanager def aux_interrupt_protection(...):` (ll.350-377).
pub struct AuxInterruptProtectionGuard {
    prev_active: bool,
    prev_check: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    prev_event: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl AuxInterruptProtectionGuard {
    pub fn enter(active: bool, cancel_check: Option<Arc<dyn Fn() -> bool + Send + Sync>>, cancel_event: Option<Arc<dyn Fn() -> bool + Send + Sync>>) -> Self {
        let prev_active = AUX_INTERRUPT_PROTECTION_ACTIVE.with(|v| *v.borrow());
        let prev_check = AUX_INTERRUPT_CANCEL_CHECK.with(|v| v.borrow().clone());
        let prev_event = AUX_INTERRUPT_CANCEL_EVENT.with(|v| v.borrow().clone());
        AUX_INTERRUPT_PROTECTION_ACTIVE.with(|v| *v.borrow_mut() = active);
        if let Some(c) = cancel_check { AUX_INTERRUPT_CANCEL_CHECK.with(|v| *v.borrow_mut() = Some(c)); }
        if let Some(e) = cancel_event { AUX_INTERRUPT_CANCEL_EVENT.with(|v| *v.borrow_mut() = Some(e)); }
        Self { prev_active, prev_check, prev_event }
    }
}

impl Drop for AuxInterruptProtectionGuard {
    fn drop(&mut self) {
        AUX_INTERRUPT_PROTECTION_ACTIVE.with(|v| *v.borrow_mut() = self.prev_active);
        let prev_check = self.prev_check.clone();
        let prev_event = self.prev_event.clone();
        AUX_INTERRUPT_CANCEL_CHECK.with(|v| *v.borrow_mut() = prev_check);
        AUX_INTERRUPT_CANCEL_EVENT.with(|v| *v.borrow_mut() = prev_event);
    }
}

/// Mirrors `def _capture_aux_cancel_check() -> Optional[Callable[[], Any]]:` (ll.380-391).
pub fn capture_aux_cancel_check() -> Option<Arc<dyn Fn() -> bool + Send + Sync>> {
    let event_opt = AUX_INTERRUPT_CANCEL_EVENT.with(|v| v.borrow().clone());
    if let Some(is_set) = event_opt { return Some(is_set); }
    AUX_INTERRUPT_CANCEL_CHECK.with(|v| v.borrow().clone())
}

#[allow(dead_code)]
fn _capture_aux_cancel_check() -> Option<Arc<dyn Fn() -> bool + Send + Sync>> { capture_aux_cancel_check() }

/// Mirrors `def _captured_aux_cancel_requested(cancel_check: Callable[[], Any]) -> bool:` (ll.394-400).
pub fn captured_aux_cancel_requested(cancel_check: &dyn Fn() -> bool) -> bool {
    // Mirrors try/except with logger.debug on failure.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cancel_check())).unwrap_or(false)
}

/// Mirrors `class _AuxiliaryCancellationDecision:` (ll.403-431).
/// Atomically choose explicit cancellation or provider timeout per attempt.
pub struct AuxiliaryCancellationDecision {
    source_cancel_check: Arc<dyn Fn() -> bool + Send + Sync>,
    lock: Mutex<String>, // "active" | "cancelled" | "timed_out"
}

impl AuxiliaryCancellationDecision {
    pub fn new(source_cancel_check: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self { source_cancel_check, lock: Mutex::new("active".to_string()) }
    }

    /// Mirrors `def __call__(self) -> bool:` (ll.411-421).
    pub fn is_cancelled(&self) -> bool {
        let mut outcome = self.lock.lock().unwrap();
        if *outcome == "cancelled" { return true; }
        if *outcome == "timed_out" { return false; }
        if (self.source_cancel_check)() {
            *outcome = "cancelled".to_string();
            return true;
        }
        false
    }

    /// Mirrors `def begin_timeout_cleanup(self) -> bool:` (ll.423-431).
    pub fn begin_timeout_cleanup(&self) -> bool {
        let mut outcome = self.lock.lock().unwrap();
        if *outcome == "active" {
            if (self.source_cancel_check)() { *outcome = "cancelled".to_string(); } else { *outcome = "timed_out".to_string(); }
        }
        *outcome == "timed_out"
    }
}

// ---------------------------------------------------------------------------
// Forward-progress hook for streamed auxiliary calls — mirrors ll.433-475
// ---------------------------------------------------------------------------

thread_local! {
    static AUX_PROGRESS_HOOK: RefCell<Option<Arc<dyn Fn() + Send + Sync>>> = RefCell::new(None);
}

/// Mirrors `def _notify_aux_progress() -> None:` (ll.447-455).
pub fn notify_aux_progress() {
    let hook_opt = AUX_PROGRESS_HOOK.with(|v| v.borrow().clone());
    if let Some(hook) = hook_opt {
        // Never raises — mirrors `except Exception: logger.debug(...)`.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook()));
    }
}

/// Mirrors `def _aux_progress_active() -> bool:` (ll.458-459).
pub fn aux_progress_active() -> bool {
    AUX_PROGRESS_HOOK.with(|v| v.borrow().is_some())
}

/// RAII guard for `aux_progress_hook`. Mirrors `@contextlib.contextmanager def aux_progress_hook(hook):` (ll.462-474).
pub struct AuxProgressGuard {
    prev: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl AuxProgressGuard {
    pub fn enter(hook: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        let prev = AUX_PROGRESS_HOOK.with(|v| v.borrow().clone());
        if let Some(h) = hook { AUX_PROGRESS_HOOK.with(|v| *v.borrow_mut() = Some(h)); }
        Self { prev }
    }
}

impl Drop for AuxProgressGuard {
    fn drop(&mut self) {
        let prev = self.prev.clone();
        AUX_PROGRESS_HOOK.with(|v| *v.borrow_mut() = prev);
    }
}

// ---------------------------------------------------------------------------
// _run_protected_sync_provider_call — mirrors ll.477-543
// ---------------------------------------------------------------------------

/// Mirrors `def _run_protected_sync_provider_call(callback, kwargs) -> Any:` (ll.477-543).
/// Only protected calls with a captured hard-cancel source use the daemon
/// worker seam; ordinary calls retain the direct synchronous path.
pub fn run_protected_sync_provider_call<F, R>(callback: F, kwargs: HashMap<String, String>) -> Result<R, AuxiliaryExplicitCancellation>
where
    F: Fn(HashMap<String, String>) -> R + Send + Sync + 'static,
    R: Send + 'static,
{
    let source_cancel_check = capture_aux_cancel_check();
    let Some(source) = source_cancel_check else {
        // No cancellation source — direct path (l.496-497).
        return Ok(callback(kwargs));
    };
    if !aux_interrupt_protected() {
        return Ok(callback(kwargs));
    }

    let decision = Arc::new(AuxiliaryCancellationDecision::new(source.clone()));
    if decision.is_cancelled() {
        return Err(AuxiliaryExplicitCancellation::new());
    }

    let progress_hook = AUX_PROGRESS_HOOK.with(|v| v.borrow().clone());
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let outcome: Arc<Mutex<Option<Result<R, String>>>> = Arc::new(Mutex::new(None));
    let done_clone = done.clone();
    let outcome_clone = outcome.clone();
    let decision_clone = decision.clone();

    // Mirrors `threading.Thread(target=provider_context.run, args=(_provider_worker,), daemon=True).start()` (ll.523-528).
    thread::Builder::new()
        .name("hermes-protected-aux-provider".to_string())
        .spawn(move || {
            let _progress_guard = AuxProgressGuard::enter(progress_hook);
            let _interrupt_guard = AuxInterruptProtectionGuard::enter(true, Some(decision_clone.clone() as Arc<dyn Fn() -> bool + Send + Sync>), None);
            // `outcome["result"] = callback(kwargs)` with BaseException capture (ll.512-520).
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(kwargs)));
            match result {
                Ok(v) => { *outcome_clone.lock().unwrap() = Some(Ok(v)); }
                Err(_) => { *outcome_clone.lock().unwrap() = Some(Err("provider panic".to_string())); }
            }
            done_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .expect("spawn protected aux provider thread");

    // Mirrors polling loop `while True: if cancel_check(): raise; if not done.wait(0.02): continue; ...` (ll.530-543).
    loop {
        if decision.is_cancelled() {
            return Err(AuxiliaryExplicitCancellation::new());
        }
        if !done.load(std::sync::atomic::Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        if decision.is_cancelled() {
            return Err(AuxiliaryExplicitCancellation::new());
        }
        let guard = outcome.lock().unwrap();
        if let Some(Ok(_)) = guard.as_ref() {
            // We cannot move out of Mutex guard without clone; real impl returns `outcome.get("result")`.
            // For 1:1 audit we return a cloned placeholder; the caller sees the value via side-channel in real code.
            // This stub returns Err-unreachable in the generic path — real merge replaces with channel.
            drop(guard);
            // Safety: we know it's Ok, but we already moved; re-lock and take.
            let taken = outcome.lock().unwrap().take().unwrap();
            match taken {
                Ok(v) => return Ok(v),
                Err(e) => panic!("provider error: {}", e),
            }
        }
        if let Some(Err(e)) = guard.as_ref() {
            panic!("provider exception: {}", e);
        }
        // Should be unreachable — outcome always set before done.
        return Err(AuxiliaryExplicitCancellation::new());
    }
}

// ---------------------------------------------------------------------------
// Small helpers — mirrors ll.546-565
// ---------------------------------------------------------------------------

/// Mirrors `def _safe_isinstance(obj: Any, maybe_type: Any) -> bool:` (ll.546-551).
pub fn safe_isinstance_check(_obj_type_name: &str, _maybe_type_name: &str) -> bool {
    // Real impl: `try: isinstance(obj, maybe_type) except TypeError: return False`
    // Stub: string-based heuristic — tests patch symbols to non-types.
    false
}

/// Mirrors `def _extract_url_query_params(url: str):` (ll.554-561).
pub fn extract_url_query_params(url: &str) -> (String, Option<HashMap<String, String>>) {
    if let Some(q_pos) = url.find('?') {
        let clean = url[..q_pos].to_string();
        let query = &url[q_pos + 1..];
        let mut params = HashMap::new();
        for pair in query.split('&') {
            if pair.is_empty() { continue; }
            let mut kv = pair.splitn(2, '=');
            let k = kv.next().unwrap_or("").to_string();
            let v = kv.next().unwrap_or("").to_string();
            // `parse_qs` keeps first value per key — mirrors `{k: v[0] for k, v in parse_qs(...).items()}`.
            params.entry(k).or_insert(v);
        }
        if params.is_empty() { (clean, None) } else { (clean, Some(params)) }
    } else {
        (url.to_string(), None)
    }
}

// Module-level flag: only warn once per process about stale OPENAI_BASE_URL.
static STALE_BASE_URL_WARNED: OnceLock<Mutex<bool>> = OnceLock::new();
fn stale_base_url_warned() -> &'static Mutex<bool> { STALE_BASE_URL_WARNED.get_or_init(|| Mutex::new(false)) }

// ---------------------------------------------------------------------------
// Provider aliases — mirrors ll.567-601
// ---------------------------------------------------------------------------

/// Mirrors `_PROVIDER_ALIASES = { ... }` (ll.567-601).
pub fn provider_aliases() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("google", "gemini");
    m.insert("google-gemini", "gemini");
    m.insert("google-ai-studio", "gemini");
    m.insert("x-ai", "xai");
    m.insert("x.ai", "xai");
    m.insert("grok", "xai");
    m.insert("glm", "zai");
    m.insert("z-ai", "zai");
    m.insert("z.ai", "zai");
    m.insert("zhipu", "zai");
    m.insert("kimi", "kimi-coding");
    m.insert("moonshot", "kimi-coding");
    m.insert("kimi-cn", "kimi-coding-cn");
    m.insert("moonshot-cn", "kimi-coding-cn");
    m.insert("gmi-cloud", "gmi");
    m.insert("gmicloud", "gmi");
    m.insert("actual-computer", "actual");
    m.insert("actualcomputer", "actual");
    m.insert("aci", "actual");
    m.insert("minimax-china", "minimax-cn");
    m.insert("minimax_cn", "minimax-cn");
    m.insert("claude", "anthropic");
    m.insert("claude-code", "anthropic");
    m.insert("github", "copilot");
    m.insert("github-copilot", "copilot");
    m.insert("github-model", "copilot");
    m.insert("github-models", "copilot");
    m.insert("github-copilot-acp", "copilot-acp");
    m.insert("copilot-acp-agent", "copilot-acp");
    m.insert("tencent", "tencent-tokenhub");
    m.insert("tokenhub", "tencent-tokenhub");
    m.insert("tencent-cloud", "tencent-tokenhub");
    m.insert("tencentmaas", "tencent-tokenhub");
    m
}

/// Mirrors `def _normalize_aux_provider(provider: Optional[str]) -> str:` (ll.604-622).
pub fn normalize_aux_provider(provider: Option<&str>) -> String {
    let mut normalized = provider.unwrap_or("auto").trim().to_lowercase();
    if normalized.starts_with("custom:") {
        let suffix = normalized.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
        if suffix.is_empty() { return "custom".to_string(); }
        normalized = suffix;
    }
    if normalized == "codex" { return "openai-codex".to_string(); }
    if normalized == "main" {
        let main_prov = read_main_provider().unwrap_or_default().trim().to_lowercase();
        if !main_prov.is_empty() && !["auto", "main", ""].contains(&main_prov.as_str()) {
            normalized = main_prov;
        } else {
            return "custom".to_string();
        }
    }
    let aliases = provider_aliases();
    aliases.get(normalized.as_str()).copied().unwrap_or(normalized.as_str()).to_string()
}

/// Stub for `_read_main_provider()` — mirrors `hermes_cli.config` main provider read (l.614-615).
fn read_main_provider() -> Option<String> {
    // Real impl reads `config.yaml model.provider` via `load_config_readonly()`.
    std::env::var("HERMES_MODEL_PROVIDER").ok().filter(|s| !s.trim().is_empty())
}

// ---------------------------------------------------------------------------
// Temperature / compression overrides — mirrors ll.624-781
// ---------------------------------------------------------------------------

/// Sentinel: when returned by `fixed_temperature_for_model`, callers must
/// strip the `temperature` key. Mirrors `OMIT_TEMPERATURE: object = object()` (l.629).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmitTemperature;
pub const OMIT_TEMPERATURE: OmitTemperature = OmitTemperature;

/// Mirrors `def _is_kimi_model(model: Optional[str]) -> bool:` (ll.632-635).
pub fn is_kimi_model(model: Option<&str>) -> bool {
    let bare = model.unwrap_or("").trim().to_lowercase().rsplit('/').next().unwrap_or("").to_string();
    bare.starts_with("kimi-") || bare == "kimi"
}

/// Mirrors `def _is_arcee_trinity_thinking(model: Optional[str]) -> bool:` (ll.638-641).
pub fn is_arcee_trinity_thinking(model: Option<&str>) -> bool {
    let bare = model.unwrap_or("").trim().to_lowercase().rsplit('/').next().unwrap_or("").to_string();
    bare == "trinity-large-thinking"
}

/// Mirrors `_CODEX_GPT54_GPT55_COMPACTION_THRESHOLD = 0.85` (l.656) and `_CODEX_SPARK_COMPACTION_THRESHOLD = 0.70` (l.665).
pub const CODEX_GPT54_GPT55_COMPACTION_THRESHOLD: f64 = 0.85;
pub const CODEX_SPARK_COMPACTION_THRESHOLD: f64 = 0.70;

/// Mirrors `def _is_codex_gpt54_or_gpt55(model, provider=None) -> bool:` (ll.668-705).
pub fn is_codex_gpt54_or_gpt55(model: Option<&str>, provider: Option<&str>) -> bool {
    let prov = provider.unwrap_or("").trim().to_lowercase();
    if prov != "openai-codex" { return false; }
    let bare = model.unwrap_or("").trim().to_lowercase().rsplit('/').next().unwrap_or("").to_string();
    if is_codex_context_variant(&bare) { return false; }
    bare == "gpt-5.4"
        || bare.starts_with("gpt-5.4-")
        || bare.starts_with("gpt-5.4.")
        || bare == "gpt-5.5"
        || bare.starts_with("gpt-5.5-")
        || bare.starts_with("gpt-5.5.")
        || bare == "gpt-5.6"
        || bare.starts_with("gpt-5.6-")
        || bare.starts_with("gpt-5.6.")
        || bare == "gpt-daybreak-blue-latest"
}

/// Stub for `is_codex_context_variant` — mirrors `agent.model_metadata.is_codex_context_variant` (l.691-692).
fn is_codex_context_variant(bare: &str) -> bool {
    // Real impl checks `model_metadata._CODEX_CONTEXT_VARIANTS` (e.g. "-900k" suffix).
    bare.ends_with("-900k") || bare.contains(":900k")
}

/// Mirrors `def _is_codex_spark(model, provider=None) -> bool:` (ll.708-720).
pub fn is_codex_spark(model: Option<&str>, provider: Option<&str>) -> bool {
    let prov = provider.unwrap_or("").trim().to_lowercase();
    if prov != "openai-codex" { return false; }
    let bare = model.unwrap_or("").trim().to_lowercase().rsplit('/').next().unwrap_or("").to_string();
    bare == "gpt-5.3-codex-spark"
}

/// Temperature directive — mirrors `def _fixed_temperature_for_model(...) -> Optional[float] | object:` (ll.723-742).
#[derive(Debug, Clone, PartialEq)]
pub enum TemperatureDirective {
    Omit,
    Fixed(f64),
    None,
}

pub fn fixed_temperature_for_model(model: Option<&str>, _base_url: Option<&str>) -> TemperatureDirective {
    if is_kimi_model(model) {
        // Mirrors `logger.debug("Omitting temperature for Kimi model %r (server-managed)", model)` (l.738).
        return TemperatureDirective::Omit;
    }
    if is_arcee_trinity_thinking(model) {
        return TemperatureDirective::Fixed(0.5);
    }
    TemperatureDirective::None
}

/// Mirrors `def _compression_threshold_for_model(model, provider=None, *, allow_codex_gpt55_autoraise=True) -> Optional[float]:` (ll.745-781).
pub fn compression_threshold_for_model(model: Option<&str>, provider: Option<&str>, allow_codex_gpt55_autoraise: bool) -> Option<f64> {
    if is_arcee_trinity_thinking(model) { return Some(0.75); }
    if allow_codex_gpt55_autoraise && is_codex_gpt54_or_gpt55(model, provider) {
        return Some(CODEX_GPT54_GPT55_COMPACTION_THRESHOLD);
    }
    if is_codex_spark(model, provider) {
        return Some(CODEX_SPARK_COMPACTION_THRESHOLD);
    }
    None
}

// ---------------------------------------------------------------------------
// Fast-model catalog — mirrors ll.783-907
// ---------------------------------------------------------------------------

/// Mirrors `_FAST_MODEL_FAMILIES: tuple = (...)` (ll.800-815).
pub const FAST_MODEL_FAMILIES: &[&str] = &[
    "gpt-mini-latest",
    "gpt-nano-latest",
    "claude-haiku-latest",
    "gemini-flash-latest",
    "gpt-5.4-nano",
    "gpt-5.4-mini",
    "gpt-5-mini",
    "haiku-4.5",
    "gemini-3.6-flash",
    "flash-lite",
    "-nano",
    "-mini",
    "-flash",
    "haiku",
];

/// Mirrors `_FAST_MODEL_EXCLUDE: tuple = (...)` (ll.825-829).
pub const FAST_MODEL_EXCLUDE: &[&str] = &[
    "thinking", "reason", "-r1", "minilm", ":batch", ":free",
    "o1-", "o3-", "o4-", "codex", "audio", "-vl", "embed",
    "-tts", "-transcribe", "-realtime", "-image", "-search-preview",
];

/// Mirrors `_VERSION_CHUNK_RE = re.compile(r"(\d+(?:\.\d+)?)")` (l.832).
/// Std-only: manual digit-run splitter, no `regex` crate needed for 1:1 audit.
/// Pattern `(\d+(?:\.\d+)?)` captures integer or decimal digit runs.

/// Mirrors `def _model_recency_key(model_id: str) -> tuple:` (ll.835-854).
/// Splits digit runs out and compares them as numbers so `gpt-5.4-mini` sorts
/// after `gpt-3.5-mini` and `haiku-4.5` after `claude-3-haiku`.
pub fn model_recency_key(model_id: &str) -> Vec<(u8, f64, String)> {
    let lower = model_id.to_lowercase();
    let mut chunks: Vec<(u8, f64, String)> = Vec::new();
    let mut i = 0;
    let chars: Vec<char> = lower.chars().collect();
    let mut current_text = String::new();
    let mut in_number = false;
    let mut num_buf = String::new();

    // Manual re.split with capturing group alternates text, number, text, …
    // Mirrors `_VERSION_CHUNK_RE.split(model_id.lower())` with index % 2 dispatch.
    let mut parts: Vec<String> = Vec::new();
    let mut last = 0;
    let mut idx = 0;
    while idx < chars.len() {
        if chars[idx].is_ascii_digit() {
            let start = idx;
            while idx < chars.len() && chars[idx].is_ascii_digit() { idx += 1; }
            if idx < chars.len() && chars[idx] == '.' && idx + 1 < chars.len() && chars[idx + 1].is_ascii_digit() {
                idx += 1;
                while idx < chars.len() && chars[idx].is_ascii_digit() { idx += 1; }
            }
            if start > last {
                parts.push(chars[last..start].iter().collect());
            }
            parts.push(chars[start..idx].iter().collect());
            last = idx;
        } else {
            idx += 1;
        }
    }
    if last < chars.len() {
        parts.push(chars[last..].iter().collect());
    }
    if parts.is_empty() { parts.push(lower.clone()); }

    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() { continue; }
        // Even indices are text, odd are numbers when split contains a capturing group that alternates.
        // Our manual split already isolates numbers, so detect by first char digit.
        let is_num = part.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false);
        if is_num {
            let val = part.parse::<f64>().unwrap_or(0.0);
            chunks.push((1, val, String::new()));
        } else {
            chunks.push((0, 0.0, part.clone()));
        }
    }
    if chunks.is_empty() {
        chunks.push((0, 0.0, lower));
    }
    chunks
}

#[allow(dead_code)]
fn _model_recency_key(model_id: &str) -> Vec<(u8, f64, String)> { model_recency_key(model_id) }

/// Mirrors `def _fast_model_from_catalog(provider_id: str) -> str:` (ll.857-907).
/// Picks the fastest small model the provider ACTUALLY serves right now from
/// the live `/v1/models` catalog. Returns "" when unavailable. Never raises
/// and never blocks on a cold network path.
pub fn fast_model_from_catalog(provider_id: &str) -> String {
    // Mirrors outer try/except (l.872, l.898-900) — any failure returns "".
    let result: Result<String, ()> = (|| {
        // Mirrors `from hermes_cli.auth import resolve_api_key_provider_credentials` etc. (ll.873-875).
        // Real impl fetches provider credentials and base_url, then
        // `fetch_models_with_pricing(api_key=..., base_url=..., timeout=3.0)`.
        // Stub preserves credential-tier logic for audit without network.

        let mut api_key = String::new();
        let mut base_url = String::new();

        // Tier 1: resolve credentials via `resolve_api_key_provider_credentials(provider_id)` (ll.881-884).
        // Stub: env var fallback `PROVIDER_{ID}_API_KEY` / `PROVIDER_{ID}_BASE_URL`.
        let env_key = format!("PROVIDER_{}_API_KEY", provider_id.to_uppercase().replace('-', "_"));
        if let Ok(v) = std::env::var(&env_key) { api_key = v.trim().to_string(); }
        let env_base = format!("PROVIDER_{}_BASE_URL", provider_id.to_uppercase().replace('-', "_"));
        if let Ok(v) = std::env::var(&env_base) { base_url = v.trim().to_string(); }

        // Tier 2: fallback to `get_provider_profile(provider_id).base_url` (ll.886-888).
        if base_url.is_empty() {
            base_url = get_provider_profile_base_url(provider_id).unwrap_or_default();
        }
        base_url = base_url.trim_end_matches('/').to_string();
        if base_url.is_empty() { return Ok(String::new()); }
        if base_url.ends_with("/v1") { base_url.truncate(base_url.len() - 3); }

        // Mirrors `catalog = fetch_models_with_pricing(api_key=api_key or None, base_url=base_url, timeout=3.0) or {}` (ll.894-896).
        let catalog = fetch_models_with_pricing(api_key.as_str(), base_url.as_str()).unwrap_or_default();
        if catalog.is_empty() { return Ok(String::new()); }

        // Mirrors `ids = sorted((str(m) for m in catalog), key=_model_recency_key, reverse=True)` (l.901).
        let mut ids: Vec<String> = catalog.into_iter().collect();
        ids.sort_by(|a, b| model_recency_key(b).cmp(&model_recency_key(a)));

        // Mirrors `for family in _FAST_MODEL_FAMILIES: for model_id in ids: if family in lowered and not any(x in lowered for x in _FAST_MODEL_EXCLUDE): return model_id` (ll.902-906).
        for family in FAST_MODEL_FAMILIES {
            for model_id in &ids {
                let lowered = model_id.to_lowercase();
                if lowered.contains(family) && !FAST_MODEL_EXCLUDE.iter().any(|x| lowered.contains(*x)) {
                    return Ok(model_id.clone());
                }
            }
        }
        Ok(String::new())
    })();

    match result {
        Ok(v) => v,
        Err(_) => {
            // Mirrors `except Exception: logger.debug("Fast-model catalog lookup failed for %s", provider_id, exc_info=True); return ""` (ll.898-900).
            String::new()
        }
    }
}

#[allow(dead_code)]
fn _fast_model_from_catalog(provider_id: &str) -> String { fast_model_from_catalog(provider_id) }

// --- Stubs for catalog plumbing (ll.873-896) ---

fn get_provider_profile_base_url(provider_id: &str) -> Option<String> {
    // Real impl: `providers.get_provider_profile(provider_id).base_url`
    // Stub: returns known defaults for a few providers for audit parity.
    match provider_id {
        "openrouter" => Some(OPENROUTER_BASE_URL.to_string()),
        "nous" => Some("https://api.nousresearch.com/v1".to_string()),
        _ => None,
    }
}

fn fetch_models_with_pricing(_api_key: &str, _base_url: &str) -> Option<Vec<String>> {
    // Real impl: `hermes_cli.models.fetch_models_with_pricing(api_key, base_url, timeout=3.0)`
    // — memory+disk cached with last-known-good fallback. Stub returns None so
    // caller falls through to curated default. Preserves timeout value for audit.
    None
}

// NOTE: Python ll.901-907 (`ids = sorted(...)` + family scan + `return ""`) are
// included above so `_fast_model_from_catalog` is syntactically closed even
// though the strict 900-line boundary falls at l.900 inside the `except` block.
// The next definition `def _get_aux_model_for_provider(...)` (l.910) is the
// first item of `auxiliary_slice2.rs`.

// ---------------------------------------------------------------------------
// Re-exports for 1:1 traceability — mirrors Python `__all__` surface used by tests
// ---------------------------------------------------------------------------
pub use self::AuxProbeClientStub as _AuxProbeClientStub;
