//! Provider-agnostic billing/credit recovery links.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/agent/billing_links.py` (124 lines).
//!
//! Maps a billing-classified failure onto a recovery link + label. *Detection*
//! is not done here — that is `agent.error_classifier` (`FailoverReason.billing`),
//! the single source of truth for "credit wall vs. rate limit / auth / transport".
//! The resulting `BillingBlock` rides the turn result and the gateway
//! `message.complete` event so every surface (CLI, TUI, desktop) renders one
//! structured signal instead of re-parsing error text.
//!
//! T0051 — 1:1 port, no cargo (NEVER cargo).
//!
//! Notes on fidelity vs. Rust idioms:
//! - Python `dataclass` `BillingBlock` ↔ `#[derive(Debug, Clone, PartialEq)]` struct
//!   with `to_dict()` returning `HashMap<String, Value>` (std-only `serde_json::Value` stand-in).
//! - Python `_Provider(label, url, slugs, hosts)` frozen dataclass ↔ `Provider` with `&'static str` slices.
//! - Python `_PROVIDERS: tuple[_Provider, ...]` ↔ `&[Provider]` const slice (same 14 entries, byte-identical URLs/hosts).
//! - Python `_BY_SLUG: dict[str,_Provider] = {slug: p for ...}` ↔ linear scan `provider_by_slug` (same semantics, no HashMap cost).
//! - Python `from utils import base_url_host_matches` ↔ local `base_url_host_matches` + `base_url_hostname`
//!   faithful to `utils.py:906-924` (lowercase, trailing-dot strip, suffix check, bare-host `//` handling).
//! - Python `(provider or "").strip().lower()` ↔ `provider.trim().to_ascii_lowercase()`.
//! - Python `str(base_url or "")` ↔ `base_url: &str` (empty when None; callers pass `""` for None).
//! - Python `slug.replace("_"," ").replace("-"," ").strip().title() or "your provider"` ↔ `slug_to_title()` helper.
//! - Python `from hermes_cli.nous_account import nous_portal_billing_url` try/except ↔
///   `crate::nous_account::nous_portal_billing_url(None)` with fallback to `"https://portal.nousresearch.com/billing"`.
//! - Python `Optional[str]` billing_url ↔ `Option<String>`.
//! - Crate stays `std`-only — no `serde`, `tokio`, or external deps.

#![allow(dead_code, unused_imports, unused_variables, clippy::all)]

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Minimal Value — mirrors `Any` dict payloads for 1:1 `asdict` / `to_dict` (std-only)
// ---------------------------------------------------------------------------

/// Std-only stand-in for `Any` / `serde_json::Value` used by `BillingBlock::to_dict`.
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
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

// ---------------------------------------------------------------------------
// utils helpers — mirrors `utils.py:851-924`
// ---------------------------------------------------------------------------

/// Mirrors `def base_url_hostname(base_url: str) -> str:` (utils.py:851-865).
pub fn base_url_hostname(base_url: &str) -> String {
    let raw = base_url.trim();
    if raw.is_empty() {
        return String::new();
    }
    // Python: `parsed = urlparse(raw if "://" in raw else f"//{raw}")`
    // For `//host`, urlparse treats `host` as netloc. We mimic by stripping scheme/userinfo.
    let without_scheme = if let Some(idx) = raw.find("://") {
        &raw[idx + 3..]
    } else if raw.starts_with("//") {
        &raw[2..]
    } else {
        raw
    };
    // Take up to '/' or '?' or '#', then strip port and userinfo.
    let host_and_rest = without_scheme
        .split('/')
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .split('#')
        .next()
        .unwrap_or("");
    // Strip userinfo `user:pass@`
    let host_port = host_and_rest.rsplit('@').next().unwrap_or(host_and_rest);
    // Strip port `:port` — but be careful with IPv6 `[::1]:port` (not needed for provider hosts)
    let hostname = if host_port.starts_with('[') {
        // IPv6 literal: `[host]` or `[host]:port`
        if let Some(end) = host_port.find(']') {
            &host_port[1..end]
        } else {
            host_port
        }
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    hostname.to_ascii_lowercase().trim_end_matches('.').to_string()
}

/// Mirrors `def base_url_host_matches(base_url: str, domain: str) -> bool:` (utils.py:906-924).
pub fn base_url_host_matches(base_url: &str, domain: &str) -> bool {
    let hostname = base_url_hostname(base_url);
    if hostname.is_empty() {
        return false;
    }
    let domain = domain.trim().to_ascii_lowercase();
    let domain = domain.trim_end_matches('.');
    if domain.is_empty() {
        return false;
    }
    hostname == domain || hostname.ends_with(&format!(".{}", domain))
}

#[allow(dead_code)]
fn _base_url_host_matches(base_url: &str, domain: &str) -> bool {
    base_url_host_matches(base_url, domain)
}

// ---------------------------------------------------------------------------
// BillingBlock — mirrors `agent/billing_links.py:19-37`
// ---------------------------------------------------------------------------

/// Structured billing-wall descriptor shared across every surface.
///
/// `is_nous` is the routing bit: Nous has a first-class in-app billing surface
/// (desktop Settings → Billing, TUI/CLI `/topup`), so surfaces prefer that over
/// `billing_url`; third-party providers have no in-app flow, so `billing_url`
/// is the deep link the user actually needs.
///
/// Mirrors `@dataclass class BillingBlock:` (ll.19-37).
#[derive(Debug, Clone, PartialEq)]
pub struct BillingBlock {
    pub provider: String,
    pub provider_label: String,
    pub model: String,
    pub billing_url: Option<String>,
    pub is_nous: bool,
    pub message: String,
}

impl BillingBlock {
    pub fn new(
        provider: impl Into<String>,
        provider_label: impl Into<String>,
        model: impl Into<String>,
        billing_url: Option<String>,
        is_nous: bool,
        message: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            provider_label: provider_label.into(),
            model: model.into(),
            billing_url,
            is_nous,
            message: message.into(),
        }
    }

    /// Mirrors `def to_dict(self) -> dict: return asdict(self)` (ll.36-37).
    pub fn to_dict(&self) -> HashMap<String, Value> {
        let mut m = HashMap::new();
        m.insert("provider".to_string(), Value::String(self.provider.clone()));
        m.insert(
            "provider_label".to_string(),
            Value::String(self.provider_label.clone()),
        );
        m.insert("model".to_string(), Value::String(self.model.clone()));
        m.insert(
            "billing_url".to_string(),
            match &self.billing_url {
                Some(u) => Value::String(u.clone()),
                None => Value::Null,
            },
        );
        m.insert("is_nous".to_string(), Value::Bool(self.is_nous));
        m.insert("message".to_string(), Value::String(self.message.clone()));
        m
    }
}

// ---------------------------------------------------------------------------
// _Provider — mirrors `agent/billing_links.py:40-45`
// ---------------------------------------------------------------------------

/// Mirrors `@dataclass(frozen=True) class _Provider:` (ll.40-45).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub label: &'static str,
    pub url: &'static str,
    pub slugs: &'static [&'static str],
    pub hosts: &'static [&'static str],
}

// Single source of truth: internal slug(s) + base_url host(s) → billing page.
// Curated "add credits / manage billing" landing pages, not marketing homes.
// Hosts back the OpenAI-compatible fallback where the slug is a generic bucket
// (e.g. "openai_compatible") but base_url reveals the real upstream. An unknown
// provider degrades to a readable label with no invented URL.
// Mirrors `_PROVIDERS: tuple[_Provider, ...] = (...)` (ll.48-68)
pub const PROVIDERS: &[Provider] = &[
    Provider {
        label: "OpenAI",
        url: "https://platform.openai.com/settings/organization/billing",
        slugs: &["openai"],
        hosts: &["api.openai.com"],
    },
    Provider {
        label: "Anthropic",
        url: "https://console.anthropic.com/settings/billing",
        slugs: &["anthropic"],
        hosts: &["api.anthropic.com"],
    },
    Provider {
        label: "OpenRouter",
        url: "https://openrouter.ai/settings/credits",
        slugs: &["openrouter"],
        hosts: &["openrouter.ai"],
    },
    Provider {
        label: "xAI",
        url: "https://console.x.ai/team/default/billing",
        slugs: &["xai", "xai-oauth"],
        hosts: &["api.x.ai"],
    },
    Provider {
        label: "DeepSeek",
        url: "https://platform.deepseek.com/top_up",
        slugs: &["deepseek"],
        hosts: &["api.deepseek.com"],
    },
    Provider {
        label: "Groq",
        url: "https://console.groq.com/settings/billing",
        slugs: &["groq"],
        hosts: &["api.groq.com"],
    },
    Provider {
        label: "Mistral",
        url: "https://console.mistral.ai/billing",
        slugs: &["mistral"],
        hosts: &["api.mistral.ai"],
    },
    Provider {
        label: "Together AI",
        url: "https://api.together.ai/settings/billing",
        slugs: &["together"],
        hosts: &["api.together.ai", "api.together.xyz"],
    },
    Provider {
        label: "Fireworks AI",
        url: "https://fireworks.ai/account/billing",
        slugs: &["fireworks"],
        hosts: &["fireworks.ai"],
    },
    Provider {
        label: "Perplexity",
        url: "https://www.perplexity.ai/settings/api",
        slugs: &["perplexity"],
        hosts: &["perplexity.ai"],
    },
    Provider {
        label: "Google AI",
        url: "https://aistudio.google.com/app/billing",
        slugs: &["google", "gemini"],
        hosts: &["generativelanguage.googleapis.com"],
    },
    Provider {
        label: "Cohere",
        url: "https://dashboard.cohere.com/billing",
        slugs: &["cohere"],
        hosts: &[],
    },
    Provider {
        label: "Moonshot AI",
        url: "https://platform.moonshot.ai/console/pay",
        slugs: &["moonshot"],
        hosts: &[],
    },
    Provider {
        label: "NVIDIA",
        url: "https://build.nvidia.com/settings/billing",
        slugs: &["nvidia"],
        hosts: &[],
    },
];

/// Mirrors `_BY_SLUG: dict[str, _Provider] = {slug: p for p in _PROVIDERS for slug in p.slugs}` (l.70).
pub fn provider_by_slug(slug: &str) -> Option<&'static Provider> {
    let key = slug.trim().to_ascii_lowercase();
    for p in PROVIDERS {
        for s in p.slugs {
            if *s == key {
                return Some(p);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// is_nous_inference_route — mirrors `agent/billing_links.py:73-77`
// ---------------------------------------------------------------------------

/// True when the failing route is the Nous-managed inference gateway.
///
/// Mirrors `def is_nous_inference_route(provider: str, base_url: str) -> bool:` (ll.73-77).
pub fn is_nous_inference_route(provider: &str, base_url: &str) -> bool {
    // Mirrors `if (provider or "").strip().lower() == "nous": return True` (l.75)
    if provider.trim().to_ascii_lowercase() == "nous" {
        return true;
    }
    // Mirrors `return base_url_host_matches(str(base_url or ""), "inference-api.nousresearch.com")` (l.77)
    base_url_host_matches(base_url, "inference-api.nousresearch.com")
}

#[allow(dead_code)]
fn _is_nous_inference_route(provider: &str, base_url: &str) -> bool {
    is_nous_inference_route(provider, base_url)
}

// ---------------------------------------------------------------------------
// _nous_billing_url — mirrors `agent/billing_links.py:80-87`
// ---------------------------------------------------------------------------

/// Best-effort Nous portal billing URL (text-surface fallback; Nous prefers the in-app flow).
///
/// Mirrors `def _nous_billing_url() -> Optional[str]:` (ll.80-87).
pub fn nous_billing_url() -> Option<String> {
    // Mirrors `try: from hermes_cli.nous_account import nous_portal_billing_url; return nous_portal_billing_url(None) except Exception: return "https://portal.nousresearch.com/billing"` (ll.82-87)
    let fallback = "https://portal.nousresearch.com/billing".to_string();
    // In this crate `crate::nous_account` is the 1:1 port of `hermes_cli.nous_account`.
    // Use it when compiled as part of `hermes-provider`; otherwise fall back.
    // We call via a helper that isolates the dependency so the file stays compilable standalone.
    try_nous_portal_billing_url().unwrap_or(fallback)
}

fn try_nous_portal_billing_url() -> Option<String> {
    // The sibling module `crate::nous_account::nous_portal_billing_url` mirrors
    // `hermes_cli.nous_account.nous_portal_billing_url` exactly (nous_account.rs:286).
    // It never raises; we still wrap to preserve the `except Exception` shape.
    let url = crate::nous_account::nous_portal_billing_url(None);
    let t = url.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

#[allow(dead_code)]
fn _nous_billing_url() -> Option<String> {
    nous_billing_url()
}

// ---------------------------------------------------------------------------
// _resolve_provider_link — mirrors `agent/billing_links.py:90-101`
// ---------------------------------------------------------------------------

/// Resolve `(label, url)`: exact slug → base_url host → readable-label fallback.
///
/// Mirrors `def _resolve_provider_link(slug: str, base_url: str) -> tuple[str, Optional[str]]:` (ll.90-101).
pub fn resolve_provider_link(slug: &str, base_url: &str) -> (String, Option<String>) {
    // Mirrors `hit = _BY_SLUG.get(slug); if hit: return hit.label, hit.url` (ll.92-94)
    if let Some(hit) = provider_by_slug(slug) {
        return (hit.label.to_string(), Some(hit.url.to_string()));
    }
    // Mirrors `base = str(base_url or ""); for p in _PROVIDERS: if any(base_url_host_matches(base, host) for host in p.hosts): return p.label, p.url` (ll.96-99)
    let base = base_url;
    for p in PROVIDERS {
        for host in p.hosts {
            if base_url_host_matches(base, host) {
                return (p.label.to_string(), Some(p.url.to_string()));
            }
        }
    }
    // Mirrors `return slug.replace("_", " ").replace("-", " ").strip().title() or "your provider", None` (l.101)
    let title = slug_to_title(slug);
    let label = if title.is_empty() {
        "your provider".to_string()
    } else {
        title
    };
    (label, None)
}

#[allow(dead_code)]
fn _resolve_provider_link(slug: &str, base_url: &str) -> (String, Option<String>) {
    resolve_provider_link(slug, base_url)
}

fn slug_to_title(slug: &str) -> String {
    // Mirrors `slug.replace("_", " ").replace("-", " ").strip().title()` (l.101)
    let replaced = slug.replace('_', " ").replace('-', " ");
    let trimmed = replaced.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Python `str.title()` capitalizes each word: first char upper, rest lower.
    // Emulate via whitespace split.
    trimmed
        .split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut out = first.to_uppercase().to_string();
                    out.push_str(&chars.as_str().to_ascii_lowercase());
                    out
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// build_billing_block — mirrors `agent/billing_links.py:104-124`
// ---------------------------------------------------------------------------

/// Build the billing descriptor for a billing-classified failure.
///
/// `message` is the guidance already assembled by the agent loop
/// (`agent.conversation_loop._billing_or_entitlement_message`), carried
/// through unchanged so every surface shows identical copy.
///
/// Mirrors `def build_billing_block(*, provider: str, base_url: str, model: str, message: str = "") -> BillingBlock:` (ll.104-124).
pub fn build_billing_block(provider: &str, base_url: &str, model: &str, message: &str) -> BillingBlock {
    // Mirrors `slug = (provider or "").strip().lower()` (l.117)
    let slug = provider.trim().to_ascii_lowercase();
    // Mirrors `model = (model or "").strip()` (l.118)
    let model = model.trim().to_string();

    // Mirrors `if is_nous_inference_route(slug, base_url): return BillingBlock(slug or "nous", "Nous Portal", model, _nous_billing_url(), True, message or "")` (ll.120-121)
    if is_nous_inference_route(&slug, base_url) {
        let prov = if slug.is_empty() { "nous".to_string() } else { slug.clone() };
        let billing_url = nous_billing_url();
        let msg = message.to_string();
        return BillingBlock::new(prov, "Nous Portal", model, billing_url, true, msg);
    }

    // Mirrors `label, url = _resolve_provider_link(slug, base_url)` (l.123)
    let (label, url) = resolve_provider_link(&slug, base_url);
    // Mirrors `return BillingBlock(slug, label, model, url, False, message or "")` (l.124)
    BillingBlock::new(slug, label, model, url, false, message.to_string())
}

#[allow(dead_code)]
fn _build_billing_block(provider: &str, base_url: &str, model: &str, message: &str) -> BillingBlock {
    build_billing_block(provider, base_url, model, message)
}
