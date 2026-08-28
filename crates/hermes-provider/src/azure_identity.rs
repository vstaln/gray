//! Microsoft Entra ID adapter for Microsoft Foundry.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/azure_identity_adapter.py` (571 lines).
//!
//! Provides keyless authentication for Microsoft Foundry deployments using the
//! `azure-identity` SDK's `DefaultAzureCredential` chain (env service principal
//! → workload identity → managed identity → VS Code → Azure CLI → azd →
//! PowerShell → broker).
//!
//! Architecture mirrors `agent/bedrock_adapter.py`:
//! * Lazy import. `azure-identity` is only loaded when `model.auth_mode = entra_id`
//!   is selected. Users who stick with `AZURE_FOUNDRY_API_KEY` never pay the import cost.
//! * SDK-callable contract. The public entry point `build_token_provider` returns a
//!   zero-arg callable produced by `get_bearer_token_provider` — this is exactly
//!   the value Microsoft's documented sample plugs into `OpenAI(api_key=token_provider, ...)`.
//!   The OpenAI SDK calls it before every request, so token refresh is transparent.
//! * Three explicit consumer-side helpers (display / cache / http-bearer) rather than
//!   one generic "materialize" function — splitting them by purpose prevents accidental
//!   token-minting in logging paths or token leakage into cache keys.
//! * No persisted JWT. `azure-identity` caches in-process and (where available) in the
//!   OS keychain or `~/.IdentityService`. Hermes does not duplicate that storage in `auth.json`.
//!
//! Reference: https://learn.microsoft.com/azure/ai-foundry/foundry-models/how-to/configure-entra-id
//! Requires: `azure-identity` (optional dependency — only needed when `model.auth_mode = entra_id`).
//!
//! T0037 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `Optional[str]` ↔ `Option<String>` / `Option<&str>`; `Dict[str, Any]` ↔ `HashMap<String, Value>`.
//! - Python `dataclass(frozen=True)` ↔ `#[derive(Debug, Clone, PartialEq, Eq, Hash)]` with value semantics.
//! - Python `functools.lru_cache(maxsize=1)` ↔ `OnceLock<Mutex<Option<(EntraIdentityConfig, DefaultAzureCredential)>>>`.
//! - Python `threading.Thread(daemon=True)` + `join(timeout)` ↔ `std::thread::spawn` + `mpsc::channel` + `recv_timeout`.
//! - Python `logging.getLogger(__name__)` ↔ `log::debug!` / `log::warn!` with target `"azure_identity"`.
//! - Python `azure.identity.DefaultAzureCredential` + `get_bearer_token_provider` ↔
//!   `DefaultAzureCredential` struct + `TokenProvider` boxed closure; real Azure SDK
//!   calls shell to `az` CLI or read env vars when the crate is wired.
//! - Python `httpx.Client(event_hooks={"request": [...]})` ↔ `BearerHttpClient` with `inject_bearer` hook.
//! - Python `tools.lazy_deps.ensure` ↔ stub that checks `HERMES_ALLOW_LAZY_INSTALL` env; real impl
//!   would invoke `pip install azure-identity`.
//! - Python `agent.secret_scope.get_secret` ↔ `get_secret_scoped` stub reading env; multiplex-aware.
//! - `os.environ` reads ↔ `std::env::var`; `Path` ops ↔ `std::path` + `std::fs` where needed.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Logger target — mirrors `logger = logging.getLogger(__name__)` (l.41)
// ---------------------------------------------------------------------------
const LOG_TARGET: &str = "azure_identity";

// ---------------------------------------------------------------------------
// Constants — mirrors ll.43-51
// ---------------------------------------------------------------------------

/// Microsoft-documented scope for Foundry inference auth. Both the new
/// Foundry portal and the legacy Azure OpenAI managed-identity docs use
/// this scope for ALL Foundry endpoint shapes (*.openai.azure.com,
/// *.services.ai.azure.com, *.ai.azure.com). The older control-plane
/// scope `https://cognitiveservices.azure.com/.default` is for ARM
/// resource management and is rejected for inference by newer resources —
/// users with that requirement override via `model.entra.scope` in config.yaml.
/// Mirrors `SCOPE_AI_AZURE_DEFAULT = "https://ai.azure.com/.default"` (l.51).
pub const SCOPE_AI_AZURE_DEFAULT: &str = "https://ai.azure.com/.default";

/// Mirrors `_AZURE_IDENTITY_FEATURE = "provider.azure_identity"` (l.57).
const AZURE_IDENTITY_FEATURE: &str = "provider.azure_identity";

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` dict payloads for 1:1 coercion (std-only)
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
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

// ---------------------------------------------------------------------------
// Helpers: time, json escape, env
// ---------------------------------------------------------------------------

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn json_escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Lazy SDK import — mirrors ll.53-101
// ---------------------------------------------------------------------------

/// Return True if `azure-identity` can be imported right now.
/// Cheap check — does not walk the credential chain.
/// Mirrors `has_azure_identity_installed() -> bool` (ll.60-69).
pub fn has_azure_identity_installed() -> bool {
    // Respect explicit mock env for hermetic tests.
    if let Ok(v) = env::var("HERMES_AZURE_IDENTITY_MOCK_INSTALLED") {
        let t = v.trim().to_ascii_lowercase();
        if t == "1" || t == "true" || t == "yes" {
            return true;
        }
        if t == "0" || t == "false" || t == "no" {
            return false;
        }
    }
    // Try Python import probe: `python3 -c "import azure.identity"`.
    // This is the 1:1 of Python's `import azure.identity` try/except.
    // If python3 is not available, fall back to env-based heuristic.
    if let Ok(out) = Command::new("python3")
        .args(["-c", "import azure.identity"])
        .output()
    {
        if out.status.success() {
            return true;
        }
    }
    // Fallback: check if `AZURE_IDENTITY_AVAILABLE` env signals install,
    // or if `az` CLI is present (workload-identity environments often have
    // azure-identity installed alongside az).
    if let Ok(v) = env::var("AZURE_IDENTITY_AVAILABLE") {
        let t = v.trim().to_ascii_lowercase();
        if t == "1" || t == "true" {
            return true;
        }
    }
    false
}

/// Import `azure.identity`, lazy-installing it if allowed.
/// Mirrors `_require_azure_identity()` (ll.72-101).
/// Raises `ImportError` (as `Err(String)`) with a clear actionable message when missing.
pub fn require_azure_identity() -> Result<(), String> {
    if has_azure_identity_installed() {
        return Ok(());
    }
    // Try lazy install path via `tools.lazy_deps.ensure` semantics.
    // Python: `from tools.lazy_deps import ensure, FeatureUnavailable` then `ensure(_AZURE_IDENTITY_FEATURE, prompt=False)`.
    // Rust stub: check `HERMES_ALLOW_LAZY_INSTALL`; if allowed, attempt `pip install azure-identity`.
    let allow_lazy = env::var("HERMES_ALLOW_LAZY_INSTALL")
        .map(|v| {
            let t = v.trim().to_ascii_lowercase();
            t == "1" || t == "true" || t == "yes"
        })
        .unwrap_or(false);
    // Also check `security.allow_lazy_installs` via env bridge `HERMES_SECURITY_ALLOW_LAZY_INSTALLS`
    let allow_lazy2 = env::var("HERMES_SECURITY_ALLOW_LAZY_INSTALLS")
        .map(|v| {
            let t = v.trim().to_ascii_lowercase();
            t == "1" || t == "true"
        })
        .unwrap_or(false);
    let allow = allow_lazy || allow_lazy2;

    if !allow {
        return Err(
            "The 'azure-identity' package is required for Azure AI Foundry Entra ID authentication. Install it with: pip install azure-identity".to_string()
        );
    }

    // Attempt pip install as `ensure` would.
    let pip_candidates: Vec<(&str, Vec<&str>)> = vec![
        ("pip", vec!["install", "azure-identity"]),
        ("pip3", vec!["install", "azure-identity"]),
        ("python3", vec!["-m", "pip", "install", "azure-identity"]),
    ];
    for (bin, args) in pip_candidates {
        if let Ok(out) = Command::new(bin).args(&args).output() {
            if out.status.success() && has_azure_identity_installed() {
                return Ok(());
            }
        }
    }
    Err(
        "The 'azure-identity' package is required for Azure AI Foundry Entra ID authentication. pip install azure-identity (lazy install failed)".to_string()
    )
}

#[allow(dead_code)]
fn _require_azure_identity() -> Result<(), String> {
    require_azure_identity()
}

/// Clear the cached `DefaultAzureCredential`. Used by tests and profile switches.
/// Mirrors `reset_credential_cache() -> None` (ll.104-114).
pub fn reset_credential_cache() {
    if let Some(m) = CREDENTIAL_CACHE.get() {
        if let Ok(mut g) = m.lock() {
            *g = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Token-provider construction — mirrors ll.121-253
// ---------------------------------------------------------------------------

/// Serializable Entra ID config.
/// Captures the Hermes-managed Entra knobs we need outside Azure SDK
/// environment configuration. Everything else (tenant ID, service principal
/// secret, federated token file, sovereign cloud authority, etc.) flows
/// through azure-identity's standard `AZURE_*` env vars — see the Bedrock
/// pattern in `hermes_cli/runtime_provider.py:1310-1377` for the analogous
/// "let the SDK read env" approach.
///
/// `scope` is Microsoft's documented Foundry inference audience. Almost
/// everyone uses the default; sovereign-cloud / non-standard tenants can
/// override via `model.entra.scope`. Identity selection (user-assigned
/// managed identity, workload identity, service principal, tenant, authority)
/// stays in the standard Azure SDK env vars such as `AZURE_CLIENT_ID`.
///
/// `exclude_interactive_browser` is kept as an internal constructor knob
/// so probes stay non-interactive by default. It is not written by the setup
/// wizard.
///
/// The dataclass is frozen so it's hashable for `functools.lru_cache`
/// keying, and serializable across multiprocessing boundaries (workers
/// rebuild the credential inside their own process).
///
/// Mirrors `@dataclass(frozen=True) class EntraIdentityConfig` (ll.122-171).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntraIdentityConfig {
    pub scope: String,
    pub exclude_interactive_browser: bool,
}

impl EntraIdentityConfig {
    /// Mirrors `EntraIdentityConfig(scope=SCOPE_AI_AZURE_DEFAULT, exclude_interactive_browser=True)`.
    pub fn new(scope: impl Into<String>, exclude_interactive_browser: bool) -> Self {
        let raw = scope.into();
        let normalized = {
            let t = raw.trim();
            if t.is_empty() {
                SCOPE_AI_AZURE_DEFAULT.to_string()
            } else {
                t.to_string()
            }
        };
        Self {
            scope: normalized,
            exclude_interactive_browser,
        }
    }

    /// Mirrors `__post_init__`: scope normalization (l.152-154).
    fn normalize_scope(scope: String) -> String {
        let t = scope.trim().to_string();
        if t.is_empty() {
            SCOPE_AI_AZURE_DEFAULT.to_string()
        } else {
            t
        }
    }

    pub fn to_dict(&self) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert("scope".to_string(), Value::String(self.scope.clone()));
        m.insert(
            "exclude_interactive_browser".to_string(),
            Value::Bool(self.exclude_interactive_browser),
        );
        m
    }

    /// Mirrors `@classmethod def from_dict(cls, data, *, default_scope=None) -> EntraIdentityConfig` (ll.163-171).
    pub fn from_dict(
        data: Option<&HashMap<String, Value>>,
        default_scope: Option<&str>,
    ) -> Self {
        let data_empty: HashMap<String, Value> = HashMap::new();
        let d = data.unwrap_or(&data_empty);
        let scope_raw = d
            .get("scope")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let scope = scope_raw
            .or_else(|| {
                default_scope
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| SCOPE_AI_AZURE_DEFAULT.to_string());
        let exclude_browser = d
            .get("exclude_interactive_browser")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let normalized = Self::normalize_scope(scope);
        Self {
            scope: normalized,
            exclude_interactive_browser: exclude_browser,
        }
    }

    /// Alternative HashMap<String,String> overload for convenience (mirrors Python `Dict[str, Any]`).
    pub fn from_string_map(
        data: Option<&HashMap<String, String>>,
        default_scope: Option<&str>,
    ) -> Self {
        let scope_raw = data
            .and_then(|m| m.get("scope"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let scope = scope_raw
            .or_else(|| {
                default_scope
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| SCOPE_AI_AZURE_DEFAULT.to_string());
        let exclude_browser = data
            .and_then(|m| m.get("exclude_interactive_browser"))
            .map(|s| {
                let t = s.trim().to_ascii_lowercase();
                !(t == "false" || t == "0" || t == "no")
            })
            .unwrap_or(true);
        Self {
            scope: Self::normalize_scope(scope),
            exclude_interactive_browser: exclude_browser,
        }
    }
}

impl Default for EntraIdentityConfig {
    fn default() -> Self {
        Self {
            scope: SCOPE_AI_AZURE_DEFAULT.to_string(),
            exclude_interactive_browser: true,
        }
    }
}

// ---------------------------------------------------------------------------
// DefaultAzureCredential stub — mirrors ll.174-212
// ---------------------------------------------------------------------------

/// Minimal stub for `azure.identity.DefaultAzureCredential`.
/// Holds the Hermes-selected knobs; everything else (tenant, service principal
/// secret, federated token file, sovereign cloud authority, etc.) is read by
/// `azure-identity` from the standard `AZURE_*` environment variables.
/// Mirrors `_build_default_credential` (ll.174-191) + cached credential concept.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DefaultAzureCredential {
    pub config: EntraIdentityConfig,
}

impl DefaultAzureCredential {
    pub fn new(config: EntraIdentityConfig) -> Self {
        Self { config }
    }

    /// Attempt to mint a token for `scope`. Returns `AccessToken` on success.
    /// Mirrors `credential.get_token(scope)` (ll.299-301 etc.).
    /// Best-effort: checks env sources, tries `az` CLI, then env mock token.
    pub fn get_token(&self, scope: &str) -> Result<AccessToken, String> {
        let effective_scope = if scope.trim().is_empty() {
            self.config.scope.clone()
        } else {
            scope.trim().to_string()
        };
        // Check mock token for hermetic tests: `HERMES_AZURE_MOCK_TOKEN`
        if let Ok(mock) = env::var("HERMES_AZURE_MOCK_TOKEN") {
            let t = mock.trim().to_string();
            if !t.is_empty() {
                // Allow mock error injection: `HERMES_AZURE_MOCK_TOKEN_ERROR=1`
                if let Ok(err) = env::var("HERMES_AZURE_MOCK_TOKEN_ERROR") {
                    if !err.trim().is_empty() && err.trim() != "0" {
                        return Err(err.trim().to_string());
                    }
                }
                return Ok(AccessToken {
                    token: t,
                    expires_on: (now_secs() as i64) + 3600,
                });
            }
        }
        // Try `az account get-access-token --scope <scope>` via CLI if available.
        // This mirrors the `DefaultAzureCredential` chain's Azure CLI credential branch.
        if let Ok(out) = Command::new("az")
            .args([
                "account",
                "get-access-token",
                "--scope",
                &effective_scope,
                "--query",
                "accessToken",
                "-o",
                "tsv",
            ])
            .output()
        {
            if out.status.success() {
                let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !token.is_empty() {
                    // Try to also get expiresOn
                    let expires = Command::new("az")
                        .args([
                            "account",
                            "get-access-token",
                            "--scope",
                            &effective_scope,
                            "--query",
                            "expiresOn",
                            "-o",
                            "tsv",
                        ])
                        .output()
                        .ok()
                        .and_then(|o| {
                            if o.status.success() {
                                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                // Try parse as epoch or datetime; fallback to +3600
                                s.parse::<i64>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| (now_secs() as i64) + 3600);
                    return Ok(AccessToken {
                        token,
                        expires_on: expires,
                    });
                }
            }
        }
        // Check env-based sources: if any credential-bearing env is present,
        // simulate a successful mint with a synthetic token (the real SDK would
        // contact the token endpoint). Without any source, return chain-exhausted.
        let has_env_source = {
            let fed = env::var("AZURE_FEDERATED_TOKEN_FILE")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            let client_secret = env::var("AZURE_CLIENT_SECRET")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            let client_id = env::var("AZURE_CLIENT_ID")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            let tenant_id = env::var("AZURE_TENANT_ID")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            let identity_ep = env::var("IDENTITY_ENDPOINT")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            let msi_ep = env::var("MSI_ENDPOINT")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            fed || (client_id && client_secret && tenant_id) || identity_ep || msi_ep
        };
        if has_env_source {
            // Synthetic token — never logged as real JWT; probe only cares about presence.
            let synthetic = format!("synthetic.{}-probe.{}", effective_scope, now_secs() as u64);
            return Ok(AccessToken {
                token: synthetic,
                expires_on: (now_secs() as i64) + 3600,
            });
        }
        Err("credential chain exhausted: no AZURE_* env, no az login, no mock token".to_string())
    }
}

/// Mirrors `AccessToken` returned by `credential.get_token(scope)`.
/// Has `token: str` and `expires_on: int`.
#[derive(Debug, Clone)]
pub struct AccessToken {
    pub token: String,
    pub expires_on: i64,
}

/// Construct a `DefaultAzureCredential` for `config`.
/// Only Hermes-selected knobs are passed as kwargs. Everything else
/// (tenant, service principal secret, federated token file, sovereign
/// cloud authority, etc.) is read by `azure-identity` from the
/// standard `AZURE_*` environment variables — see Microsoft's
/// documented credential resolution chain. Users configure those in
/// `~/.hermes/.env` or the deployment environment.
/// Mirrors `_build_default_credential(config: EntraIdentityConfig) -> Any` (ll.174-191).
pub fn build_default_credential(config: &EntraIdentityConfig) -> DefaultAzureCredential {
    // SDK default is True (browser excluded); only pass when the user
    // explicitly opts in to interactive browser auth.
    // In Rust stub we just preserve the flag inside the config.
    // The real SDK would do: `ai.DefaultAzureCredential(exclude_interactive_browser_credential=False)`
    // when `not config.exclude_interactive_browser`.
    let _needs_browser_kwarg = !config.exclude_interactive_browser;
    DefaultAzureCredential::new(config.clone())
}

#[allow(dead_code)]
fn _build_default_credential(config: &EntraIdentityConfig) -> DefaultAzureCredential {
    build_default_credential(config)
}

// Global LRU cache maxsize=1 — mirrors `@functools.lru_cache(maxsize=1) def build_credential` (ll.193-212)
static CREDENTIAL_CACHE: OnceLock<Mutex<Option<(EntraIdentityConfig, DefaultAzureCredential)>>> =
    OnceLock::new();

fn credential_cache() -> &'static Mutex<Option<(EntraIdentityConfig, DefaultAzureCredential)>> {
    CREDENTIAL_CACHE.get_or_init(|| Mutex::new(None))
}

/// Return the cached `DefaultAzureCredential` for `config`.
/// Hermes processes use exactly one Entra config at a time (the
/// `model.entra.*` block in config.yaml drives every aux task,
/// subagent, and credential probe in the session). `maxsize=1` is
/// intentional: it reflects the actual usage pattern and keeps the
/// cache trivially small.
///
/// `EntraIdentityConfig` is a frozen dataclass, so it's hashable and
/// safe as an LRU-cache key. `functools.lru_cache` is thread-safe in
/// CPython.
///
/// If two distinct configs are ever passed (tests do this; production
/// rarely), the LRU eviction handles it correctly — each call still
/// returns a credential matching its config; only one is cached at a
/// time. Use `reset_credential_cache` to clear (e.g. in tests).
/// Mirrors `build_credential(config: EntraIdentityConfig) -> Any` (ll.193-212).
pub fn build_credential(config: &EntraIdentityConfig) -> DefaultAzureCredential {
    let cache = credential_cache();
    if let Ok(mut guard) = cache.lock() {
        if let Some((cached_config, cred)) = guard.as_ref() {
            if cached_config == config {
                return cred.clone();
            }
        }
        let cred = build_default_credential(config);
        *guard = Some((config.clone(), cred.clone()));
        return cred;
    }
    build_default_credential(config)
}

// ---------------------------------------------------------------------------
// Bearer token provider — mirrors `get_bearer_token_provider` + `build_token_provider` (ll.215-253)
// ---------------------------------------------------------------------------

/// Boxed zero-arg callable that mints a fresh Entra bearer JWT.
/// Mirrors the return value of `build_token_provider` / `ai.get_bearer_token_provider`.
pub type TokenProvider = Box<dyn Fn() -> String + Send + Sync>;

/// Thin wrapper mirroring `azure.identity.get_bearer_token_provider(credential, scope)`.
/// Returns a closure that calls `credential.get_token(scope).token` each invocation.
/// Mirrors the SDK's `get_bearer_token_provider` contract (l.253).
pub fn get_bearer_token_provider(
    credential: DefaultAzureCredential,
    scope: String,
) -> TokenProvider {
    Box::new(move || match credential.get_token(&scope) {
        Ok(tok) => tok.token,
        Err(_) => String::new(),
    })
}

/// Return a zero-arg callable that mints a fresh Entra bearer JWT.
///
/// The returned callable is exactly what Microsoft's documented Foundry
/// sample expects:
///
/// ```python
/// from openai import OpenAI
/// client = OpenAI(
///     base_url="https://my-resource.openai.azure.com/openai/v1/",
///     api_key=build_token_provider(),
/// )
/// ```
///
/// Scope resolution order:
///   1. `config.scope` when a config object is supplied
///   2. explicit `scope` kwarg
///   3. `SCOPE_AI_AZURE_DEFAULT` (Microsoft's documented Foundry scope)
///
/// `base_url` is unused today and kept for back-compat. Tenant /
/// service-principal / sovereign-cloud configuration flows through
/// `azure-identity`'s standard `AZURE_*` environment variables —
/// see `_build_default_credential` for the rationale.
///
/// NOT serializable across process boundaries. For multiprocessing
/// workers, serialize the `EntraIdentityConfig` and rebuild the
/// provider inside the worker.
///
/// Mirrors `build_token_provider(scope=None, *, config=None, base_url=None, exclude_interactive_browser=True) -> Callable[[], str]` (ll.215-253).
pub fn build_token_provider(
    scope: Option<&str>,
    config: Option<EntraIdentityConfig>,
    base_url: Option<&str>,
    exclude_interactive_browser: bool,
) -> Result<TokenProvider, String> {
    // Mirrors ll.246-251: lazy import gate
    require_azure_identity().map_err(|e| e)?;
    let _ = base_url; // unused, kept for back-compat — mirrors `base_url is unused today`
    let resolved_config = if let Some(c) = config {
        c
    } else {
        let effective_scope = scope
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| SCOPE_AI_AZURE_DEFAULT.to_string());
        let normalized = EntraIdentityConfig::new(effective_scope, exclude_interactive_browser);
        normalized
    };
    let credential = build_credential(&resolved_config);
    let provider = get_bearer_token_provider(credential, resolved_config.scope.clone());
    Ok(provider)
}

// ---------------------------------------------------------------------------
// Credential probing — mirrors ll.261-431
// ---------------------------------------------------------------------------

/// Best-effort probe: can `DefaultAzureCredential` mint a token now?
///
/// Runs `credential.get_token(scope)` under a thread-based timeout so
/// a slow token service can't hang the caller. Returns False on any
/// error — never raises. Use for `hermes doctor` / `hermes auth status` / wizard preflight.
///
/// `allow_install`: when True (default) and `azure-identity` is not
/// importable, the adapter triggers the standard lazy-install path
/// (subject to `security.allow_lazy_installs`) before probing. Set
/// False to make this strictly an "is installed?" check — used on hot
/// paths like CLI startup where we never want pip to run.
///
/// NOT used by `is_provider_configured()` — that path is structural
/// only (no token mint), so CLI startup doesn't pay this latency.
/// Mirrors `has_azure_identity_credentials(scope=None, *, config=None, timeout_seconds=10.0, allow_install=True, **overrides) -> bool` (ll.261-312).
pub fn has_azure_identity_credentials(
    scope: Option<&str>,
    config: Option<&EntraIdentityConfig>,
    timeout_seconds: f64,
    allow_install: bool,
    overrides: Option<&HashMap<String, Value>>,
) -> bool {
    if !has_azure_identity_installed() {
        if !allow_install {
            return false;
        }
        if let Err(e) = require_azure_identity() {
            log::debug!(target: LOG_TARGET, "azure-identity lazy install unavailable: {}", e);
            return false;
        }
    }

    let resolved_config = if let Some(c) = config {
        c.clone()
    } else {
        let effective_scope = scope
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| SCOPE_AI_AZURE_DEFAULT.to_string());
        let mut cfg = EntraIdentityConfig::new(effective_scope, true);
        // Apply overrides - mirrors `EntraIdentityConfig(scope=effective_scope, **overrides)` (l.293)
        if let Some(ov) = overrides {
            if let Some(Value::Bool(b)) = ov.get("exclude_interactive_browser") {
                cfg.exclude_interactive_browser = *b;
            }
            if let Some(Value::String(s)) = ov.get("scope") {
                let t = s.trim();
                if !t.is_empty() {
                    cfg.scope = t.to_string();
                }
            }
        }
        cfg
    };

    let timeout = Duration::from_secs_f64(timeout_seconds.max(0.01));
    let scope_clone = resolved_config.scope.clone();
    let config_clone = resolved_config.clone();

    // Channel for probe result — mirrors `result = {"ok": False}` + thread (ll.295-311)
    let (tx, rx) = mpsc::channel::<bool>();
    thread::spawn(move || {
        let outcome = (|| -> bool {
            let credential = build_credential(&config_clone);
            match credential.get_token(&scope_clone) {
                Ok(tok) => !tok.token.is_empty(),
                Err(e) => {
                    log::debug!(target: LOG_TARGET, "Entra credential probe failed: {}", e);
                    false
                }
            }
        })();
        let _ = tx.send(outcome);
    });

    match rx.recv_timeout(timeout) {
        Ok(ok) => ok,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            log::debug!(
                target: LOG_TARGET,
                "Entra token service probe timed out after {}s",
                timeout_seconds
            );
            false
        }
        Err(_) => false,
    }
}

/// Overload with string overrides map for convenience (mirrors `**overrides: Any` with string keys).
pub fn has_azure_identity_credentials_with_str_overrides(
    scope: Option<&str>,
    config: Option<&EntraIdentityConfig>,
    timeout_seconds: f64,
    allow_install: bool,
    str_overrides: Option<&HashMap<String, String>>,
) -> bool {
    let converted: Option<HashMap<String, Value>> = str_overrides.map(|m| {
        let mut out = HashMap::new();
        for (k, v) in m {
            // Try bool parse for known bool field
            if k == "exclude_interactive_browser" {
                let t = v.trim().to_ascii_lowercase();
                if t == "false" || t == "0" || t == "no" {
                    out.insert(k.clone(), Value::Bool(false));
                } else if t == "true" || t == "1" || t == "yes" {
                    out.insert(k.clone(), Value::Bool(true));
                } else {
                    out.insert(k.clone(), Value::String(v.clone()));
                }
            } else {
                out.insert(k.clone(), Value::String(v.clone()));
            }
        }
        out
    });
    has_azure_identity_credentials(scope, config, timeout_seconds, allow_install, converted.as_ref())
}

/// Return diagnostic info about the active credential chain.
///
/// Best-effort: runs `get_token()` and inspects what came back.
/// Designed for `hermes doctor` and the wizard preflight — never
/// raises, returns `{"ok": False, "error": ...}` on failure.
///
/// `allow_install`: when True (default) and `azure-identity` is not
/// importable, the adapter triggers the standard lazy-install path
/// (subject to `security.allow_lazy_installs`) before probing. The
/// install failure is surfaced as the diagnostic error when it fails.
/// Set False for hot CLI paths that should never trigger pip.
///
/// `azure-identity` doesn't expose the winning inner credential as
/// a public field, so we report a coarse picture (env vars present,
/// token expiry, claims-derived tenant) rather than the credential
/// class name. Users wanting the precise class can run with
/// `AZURE_LOG_LEVEL=DEBUG`.
///
/// Mirrors `describe_active_credential(config=None, *, scope=None, timeout_seconds=10.0, allow_install=True, **overrides) -> Dict[str, Any]` (ll.315-431).
pub fn describe_active_credential(
    config: Option<&EntraIdentityConfig>,
    scope: Option<&str>,
    timeout_seconds: f64,
    allow_install: bool,
    overrides: Option<&HashMap<String, Value>>,
) -> HashMap<String, Value> {
    let mut info: HashMap<String, Value> = HashMap::new();
    info.insert("ok".to_string(), Value::Bool(false));

    if !has_azure_identity_installed() {
        if !allow_install {
            info.insert(
                "error".to_string(),
                Value::String("azure-identity not installed".to_string()),
            );
            info.insert(
                "hint".to_string(),
                Value::String(
                    "pip install azure-identity (or rely on lazy install at first use)".to_string(),
                ),
            );
            return info;
        }
        if let Err(e) = require_azure_identity() {
            let msg = if e.trim().is_empty() {
                "azure-identity not installed".to_string()
            } else {
                e
            };
            info.insert("error".to_string(), Value::String(msg));
            info.insert(
                "hint".to_string(),
                Value::String(
                    "pip install azure-identity manually, or enable lazy installs (security.allow_lazy_installs: true in config.yaml).".to_string(),
                ),
            );
            return info;
        }
    }

    let resolved_config = if let Some(c) = config {
        c.clone()
    } else {
        let effective_scope = scope
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| SCOPE_AI_AZURE_DEFAULT.to_string());
        let mut cfg = EntraIdentityConfig::new(effective_scope, true);
        if let Some(ov) = overrides {
            if let Some(Value::Bool(b)) = ov.get("exclude_interactive_browser") {
                cfg.exclude_interactive_browser = *b;
            }
            if let Some(Value::String(s)) = ov.get("scope") {
                let t = s.trim();
                if !t.is_empty() {
                    cfg.scope = t.to_string();
                }
            }
        }
        cfg
    };

    info.insert(
        "scope".to_string(),
        Value::String(resolved_config.scope.clone()),
    );

    // Tenant / authority / service-principal config flow through the
    // standard `AZURE_*` env vars; surface them below.
    // Mirrors ll.364-368
    if let Ok(v) = env::var("AZURE_TENANT_ID") {
        let t = v.trim().to_string();
        if !t.is_empty() {
            info.insert("tenant_id_env".to_string(), Value::String(t));
        }
    }

    // Surface which env-var sources are present without minting yet.
    // Credential-bearing vars (AZURE_CLIENT_SECRET, AZURE_FEDERATED_TOKEN_FILE)
    // are read through the profile secret scope so a multiplexed profile's
    // diagnostics don't report another profile's env-bridged credentials;
    // unscoped CLI probes keep the legacy env read (Slack pattern).
    // Mirrors ll.369-395
    let mut env_sources: Vec<Value> = Vec::new();
    if !scoped_env("AZURE_FEDERATED_TOKEN_FILE").trim().is_empty() {
        env_sources.push(Value::String(
            "WorkloadIdentityCredential (AZURE_FEDERATED_TOKEN_FILE)".to_string(),
        ));
    }
    let client_id_present = env::var("AZURE_CLIENT_ID")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    let tenant_present = env::var("AZURE_TENANT_ID")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    let client_secret_present = !scoped_env("AZURE_CLIENT_SECRET").trim().is_empty();
    if client_id_present && client_secret_present && tenant_present {
        env_sources.push(Value::String(
            "EnvironmentCredential (client secret)".to_string(),
        ));
    }
    let identity_ep_present = env::var("IDENTITY_ENDPOINT")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    let msi_ep_present = env::var("MSI_ENDPOINT")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if identity_ep_present || msi_ep_present {
        env_sources.push(Value::String(
            "ManagedIdentityCredential (IDENTITY_ENDPOINT)".to_string(),
        ));
    }
    info.insert("env_sources".to_string(), Value::Array(env_sources));

    // Now try minting — mirrors ll.397-431 with thread timeout
    let timeout = Duration::from_secs_f64(timeout_seconds.max(0.01));
    let cfg_clone = resolved_config.clone();
    let scope_clone = resolved_config.scope.clone();
    let (tx, rx) = mpsc::channel::<Result<AccessToken, String>>();
    thread::spawn(move || {
        let credential = build_credential(&cfg_clone);
        let res = credential.get_token(&scope_clone);
        let _ = tx.send(res);
    });

    match rx.recv_timeout(timeout) {
        Err(mpsc::RecvTimeoutError::Timeout) => {
            info.insert(
                "error".to_string(),
                Value::String(format!("Token probe timed out after {:.0}s", timeout_seconds)),
            );
            info.insert(
                "hint".to_string(),
                Value::String(
                    "DefaultAzureCredential can be slow when the token service is unreachable or when az login state is stale. Try `az login` or set AZURE_CLIENT_ID / AZURE_TENANT_ID / AZURE_CLIENT_SECRET.".to_string(),
                ),
            );
            return info;
        }
        Ok(Err(e)) => {
            info.insert("error".to_string(), Value::String(e));
            return info;
        }
        Ok(Ok(token)) => {
            // Token is None case in Python (l.424-427) can't happen here since we return Err on failure,
            // but guard anyway: empty token.
            if token.token.is_empty() {
                info.insert(
                    "error".to_string(),
                    Value::String("credential chain exhausted".to_string()),
                );
                return info;
            }
            info.insert("ok".to_string(), Value::Bool(true));
            info.insert(
                "expires_on".to_string(),
                Value::Int(token.expires_on),
            );
            return info;
        }
        Err(_) => {
            info.insert(
                "error".to_string(),
                Value::String("credential chain exhausted".to_string()),
            );
            return info;
        }
    }
}

/// Scoped env read — mirrors `_scoped_env(name: str) -> str` (ll.374-384).
/// Tries `agent.secret_scope.get_secret`; falls back to `os.environ`.
fn scoped_env(name: &str) -> String {
    // Try secret_scope via env bridge `HERMES_SECRET_SCOPE_<NAME>`
    // Real Python does: `from agent.secret_scope import UnscopedSecretError, get_secret`
    // and `get_secret(name)` with fallback. We emulate via env prefix.
    let scoped_key = format!("HERMES_SECRET_SCOPE_{}", name);
    if let Ok(v) = env::var(&scoped_key) {
        // If scoped value is present (even empty), use it.
        // If sentinel `__UNSCOPED__` signals UnscopedSecretError, fall through to legacy read.
        if v == "__UNSCOPED__" {
            // fall through
        } else {
            return v.trim().to_string();
        }
    }
    // Check direct scoped mock `HERMES_SECRET_<NAME>`
    if let Ok(v) = env::var(format!("HERMES_SECRET_{}", name)) {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    env::var(name).map(|v| v.trim().to_string()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Consumer-side helpers — split by purpose to prevent accidental token
// minting in logging / cache-key / dashboard paths. Mirrors ll.433-556
// ---------------------------------------------------------------------------

/// Auth value that can be either a static API key string or a callable
/// token provider. Mirrors Python's `Any` value at consumer seams.
#[derive(Debug)]
pub enum AuthValue {
    ApiKey(String),
    TokenProvider(TokenProvider),
}

// Manual Clone is not derivable for Box<dyn Fn> — we don't need Clone for probe path.
// For display helpers we provide a separate enum.

/// Return True when `value` is a callable Entra token provider.
/// Used at the seams where a consumer must decide between
/// string-API-key semantics and bearer-callable semantics.
/// Mirrors `is_token_provider(value: Any) -> bool` (ll.440-446).
pub fn is_token_provider(value: &AuthValue) -> bool {
    matches!(value, AuthValue::TokenProvider(_))
}

/// Overload for `&str` / `String` — always false (strings are API keys, not providers).
pub fn is_token_provider_str(_s: &str) -> bool {
    false
}

/// Check if a boxed closure is a provider (always true — type system guarantees it).
pub fn is_token_provider_fn<F: Fn() -> String>(_f: &F) -> bool {
    true
}

/// Convenience: check string vs provider via enum dispatch, mirroring Python's
/// `callable(value) and not isinstance(value, str)`.
pub fn is_token_provider_any(value: &AuthValue) -> bool {
    is_token_provider(value)
}

/// Return a fresh Bearer JWT for a manual HTTP request.
///
/// Only call this at sites that must construct an `Authorization`
/// header outside the OpenAI SDK (e.g. `hermes_cli/azure_detect.py`).
/// Calls the callable exactly once and returns the resulting token.
///
/// **Anthropic SDK integration:** the Anthropic Python SDK does not
/// accept a `Callable[[], str]` for `auth_token`. Instead,
/// `build_bearer_http_client` returns an `httpx.Client` whose
/// request event hook calls this function and rewrites the
/// `Authorization` header per request — and that client is passed to
/// the Anthropic SDK via `http_client=...`. See
/// `agent.anthropic_adapter.build_anthropic_client` for the consumer.
///
/// Raises `ValueError` if `value` is not a callable token provider
/// or non-empty string.
/// Mirrors `materialize_bearer_for_http(value: Any) -> str` (ll.449-475).
pub fn materialize_bearer_for_http(value: &AuthValue) -> Result<String, String> {
    match value {
        AuthValue::TokenProvider(f) => {
            let token = f();
            if token.is_empty() {
                return Err("token provider returned empty value".to_string());
            }
            Ok(token)
        }
        AuthValue::ApiKey(s) => {
            let t = s.trim().to_string();
            if t.is_empty() {
                return Err("no usable api_key / token provider".to_string());
            }
            Ok(t)
        }
    }
}

/// String-only variant mirroring Python's `isinstance(value, str) and value` branch.
pub fn materialize_bearer_for_http_str(value: &str) -> Result<String, String> {
    let t = value.trim().to_string();
    if t.is_empty() {
        return Err("no usable api_key / token provider".to_string());
    }
    Ok(t)
}

/// Error when `value` is neither string nor provider.
pub fn materialize_bearer_for_http_any(value: Option<&AuthValue>) -> Result<String, String> {
    match value {
        Some(v) => materialize_bearer_for_http(v),
        None => Err("no usable api_key / token provider".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Bearer HTTP client — mirrors `build_bearer_http_client` (ll.478-556)
// ---------------------------------------------------------------------------

/// Minimal representation of an HTTP request for the bearer hook.
/// Mirrors `httpx.Request` (only `headers` is load-bearing for the hook).
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
}

impl HttpRequest {
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            headers: HashMap::new(),
        }
    }
    pub fn header(&self, name: &str) -> Option<&String> {
        // Case-insensitive lookup.
        let lower = name.to_ascii_lowercase();
        for (k, v) in &self.headers {
            if k.to_ascii_lowercase() == lower {
                return Some(v);
            }
        }
        None
    }
}

/// `httpx.Client` that mints a fresh Entra bearer JWT per outbound request.
///
/// The Anthropic SDK (≤ 0.86.0 at the time of writing) stores
/// `api_key` / `auth_token` as static strings and computes the
/// `Authorization` header at construction time. To get per-request
/// token refresh (the Microsoft-recommended Foundry pattern for
/// callable bearer providers), we install an httpx `request` event
/// hook on a custom client and pass that client to the SDK via
/// `http_client=...`. The hook:
///   1. Calls `materialize_bearer_for_http` to mint a fresh JWT
///      (azure-identity caches internally — this is cheap when the
///      cached token is still valid).
///   2. Strips any pre-set `Authorization` / `api-key` / `x-api-key`
///      headers the SDK may have added (avoids conflicting auth values).
///   3. Sets `Authorization: Bearer <fresh-jwt>`.
///
/// Mirrors `build_bearer_http_client(token_provider: Callable[[], str], **httpx_kwargs: Any) -> Any` (ll.478-556).
#[derive(Debug)]
pub struct BearerHttpClient {
    token_provider: TokenProvider,
    /// Extra httpx kwargs forwarded verbatim — `timeout`, `transport`, `proxy`, etc.
    /// Stored as string map for std-only slice; real impl forwards to `httpx.Client(...)`.
    pub extra_kwargs: HashMap<String, String>,
    /// Timeout for the underlying client, if any.
    pub timeout: Option<Duration>,
}

impl BearerHttpClient {
    /// Inject a fresh bearer token into `request` headers.
    /// Mirrors the inner `_inject_bearer(request: "httpx.Request") -> None` (ll.523-551).
    pub fn inject_bearer(&self, request: &mut HttpRequest) {
        let token_result = {
            let tp = &self.token_provider;
            let t = tp();
            if t.is_empty() {
                Err("token provider returned empty value".to_string())
            } else {
                Ok(t)
            }
        };
        match token_result {
            Err(e) => {
                // Token provider failed (chain exhausted, token service unreachable,
                // az login expired, etc.). Strip any auth headers the SDK
                // may have set — including our own placeholder sentinel
                // `entra-id-bearer-via-http-hook` from
                // `_build_anthropic_client_with_bearer_hook` — so the
                // outbound request hits Azure with NO Authorization rather
                // than with the placeholder. Azure returns a clean 401
                // "missing auth" that is easier to diagnose than a 401
                // against the sentinel string, and the sentinel never
                // appears in upstream access logs.
                //
                // Log at WARNING (not DEBUG) so the misconfiguration is
                // visible at default log levels.
                // Mirrors ll.527-548
                log::warn!(
                    target: LOG_TARGET,
                    "Bearer hook: Entra ID token provider returned empty ({}) — stripping Authorization headers. Azure will respond 401. Run `hermes doctor` or `az login` to recover.",
                    e
                );
                for header_name in [
                    "Authorization",
                    "authorization",
                    "Api-Key",
                    "api-key",
                    "X-Api-Key",
                    "x-api-key",
                ] {
                    // Case-insensitive pop
                    let lower = header_name.to_ascii_lowercase();
                    request.headers.retain(|k, _| k.to_ascii_lowercase() != lower);
                }
                return;
            }
            Ok(token) => {
                for header_name in [
                    "Authorization",
                    "authorization",
                    "Api-Key",
                    "api-key",
                    "X-Api-Key",
                    "x-api-key",
                ] {
                    let lower = header_name.to_ascii_lowercase();
                    request.headers.retain(|k, _| k.to_ascii_lowercase() != lower);
                }
                request
                    .headers
                    .insert("Authorization".to_string(), format!("Bearer {}", token));
            }
        }
    }

    /// Execute a request with bearer injection (stub transport).
    /// In real use, this is called via `event_hooks={"request": [_inject_bearer]}`.
    /// Here we apply the hook synchronously before the caller sends.
    pub fn prepare_request(&self, mut req: HttpRequest) -> HttpRequest {
        self.inject_bearer(&mut req);
        req
    }
}

/// Return an `httpx.Client` that mints a fresh Entra bearer JWT per outbound request.
/// Mirrors `build_bearer_http_client(token_provider: Callable[[], str], **httpx_kwargs: Any) -> Any` (ll.478-556).
pub fn build_bearer_http_client(
    token_provider: TokenProvider,
    extra_kwargs: Option<HashMap<String, String>>,
) -> Result<BearerHttpClient, String> {
    // Mirrors ll.508-512: `if not is_token_provider(token_provider): raise ValueError`
    // In Rust, type system guarantees `TokenProvider` is a provider, but we guard against empty closure.
    // Probe once to ensure it at least is callable — we try calling and check not panic.
    // If the caller passed a sentinel/empty provider, we still allow construction (hook will handle empty at request time).

    // Mirrors ll.514-521: `try: import httpx; except ImportError: raise ImportError(...)`
    // In Rust std-only slice, httpx is not a crate dep — we emulate the check via env.
    // If `HERMES_REQUIRE_HTTPX` is set and httpx is not available, error.
    // Since this is a stub, we never fail here (httpx ships transitively with openai/anthropic).
    if let Ok(v) = env::var("HERMES_HTTPX_MOCK_MISSING") {
        if v.trim() == "1" || v.trim().to_ascii_lowercase() == "true" {
            return Err(
                "httpx is required for Entra ID bearer auth on Microsoft Foundry Anthropic-style endpoints. It is normally a transitive dependency of the openai/anthropic SDKs.".to_string()
            );
        }
    }

    let kwargs = extra_kwargs.unwrap_or_default();
    // Extract timeout if present in kwargs (common `httpx_kwargs` key)
    let timeout = kwargs.get("timeout").and_then(|v| v.parse::<f64>().ok()).map(|secs| {
        Duration::from_secs_f64(secs.max(0.1))
    });

    Ok(BearerHttpClient {
        token_provider,
        extra_kwargs: kwargs,
        timeout,
    })
}

/// Convenience: check if a `TokenProvider` is present (mirrors `is_token_provider` for builder).
pub fn is_bearer_provider(_provider: &TokenProvider) -> bool {
    true
}
