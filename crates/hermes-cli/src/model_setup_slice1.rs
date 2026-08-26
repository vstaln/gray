//! hermes-cli model_setup — slice 1/4
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/hermes_cli/model_setup_flows.py`
//! slice 1/4 — lines 1–900 of 3 281 (first 900 LOC).
//! Covers: module docstring (provider wizard extract from main.py, 18
//! `_model_flow_*` branches + lazy `hermes_cli.main` imports), bootstrap
//! imports (`argparse`, `os`, `subprocess`, `urllib.parse`,
//! `line_input`, `clear_model_endpoint_credentials`,
//! `custom_provider_slug`), `BEDROCK_GEO_PREFIXES` +
//! `bedrock_region_geo_prefix` / `bedrock_model_routable_from_region`,
//! `_existing_api_key_for_model_flow`,
//! `_prune_replaced_custom_model_config_credentials`,
//! `_prompt_auth_credentials_choice`,
//! `_model_flow_openrouter` (OpenRouter API-key + live model picker),
//! `_print_moa_preset`, `_model_flow_ai_gateway` (Vercel AI Gateway),
//! `_model_flow_moa` (Mixture-of-Agents preset picker),
//! `_model_flow_nous` (Nous Portal curated + portal recommendations +
//! free/paid tier partition), `_model_flow_openai_codex` (Codex OAuth
//! reuse/reauth/cancel + model picker), `_model_flow_xai_oauth`
//! (SuperGrok OAuth), `_model_flow_qwen_oauth` (Qwen CLI OAuth reuse),
//! `_model_flow_minimax_oauth` (MiniMax OAuth) through
//! `_model_flow_custom` header (first ~5 lines, line 895–900).
//! Continued in `model_setup_slice2.rs` (from `_model_flow_custom`
//! body, line 901).
//!
//! T0694 — 1:1 port, no cargo (NEVER cargo).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Module docstring — mirrors lines 1-19
// ---------------------------------------------------------------------------
// Python: """Per-provider model-selection wizard flows for ``hermes setup`` / ``hermes model``.
// Extracted from ``hermes_cli/main.py`` as part of the god-file decomposition
// campaign (``~/.hermes/plans/god-file-decomposition.md``, Phase 2 — splitting
// main.py handler/flow bodies out of the module). These 18 ``_model_flow_*``
// functions are the interactive provider-setup branches dispatched by
// ``select_provider_and_model`` (which stays in main.py).
//
// Behavior-neutral: each function is lifted verbatim. ``select_provider_and_model``
// in main.py re-imports them (``from hermes_cli.model_setup_flows import *``-style
// explicit import) so existing call sites — and test monkeypatches that target
// ``hermes_cli.main._model_flow_*`` — keep resolving against main.py's namespace.
//
// main.py-internal helpers the flows call (``_prompt_api_key``, ``_save_custom_provider``,
// the reasoning-effort/stepfun/qwen helpers, ``_run_anthropic_oauth_flow``, …) are
// imported lazily inside the flows (``from hermes_cli.main import ...`` resolves at
// call time, when main.py is fully loaded) so this module never imports
// ``hermes_cli.main`` at import time -> no import cycle.
// """

/// Module doc — mirrors `hermes_cli/model_setup_flows.py` top docstring (lines 1-19).
pub const MODULE_DOC: &str =
    "Per-provider model-selection wizard flows for `hermes setup` / `hermes model` — 18 _model_flow_* branches";

// ---------------------------------------------------------------------------
// Imports — mirrors lines 21-31
// ---------------------------------------------------------------------------
// Python: from __future__ import annotations
//         from hermes_cli.cli_output import line_input
//         import argparse, os, subprocess, urllib.parse
//         from hermes_cli.config import clear_model_endpoint_credentials
//         from hermes_cli.providers import custom_provider_slug
//
// Rust: std only (NEVER cargo). External hermes_cli / agent crates are stubbed
// for 1:1 traceability; real wiring in later slices or via injected callbacks.

/// Mirrors `from hermes_cli.cli_output import line_input` — stub.
pub fn line_input_stub(prompt: &str) -> String {
    let _ = prompt;
    String::new()
}

/// Mirrors `from hermes_cli.config import clear_model_endpoint_credentials` — stub.
///
/// Drops stale endpoint credentials from `model` dict. `clear_api_mode=false`
/// keeps `api_mode` (mirrors Python `clear_api_mode=False` default for most
/// flows; MoA passes `True`).
pub fn clear_model_endpoint_credentials_stub(
    model: &mut HashMap<String, String>,
    clear_api_mode: bool,
) {
    // Python: deletes `api_key` (+ maybe `api_mode`) from the model dict when
    // switching providers so a stale key/mode doesn't poison the next provider.
    // Stub: remove `api_key` always; `api_mode` only when clear_api_mode=true
    // or when the caller explicitly wants it (MoA, bedrock, vertex branches).
    model.remove("api_key");
    if clear_api_mode {
        model.remove("api_mode");
    }
    // Also clears `base_url` in some flows via `clear_base_url=True` kwarg
    // (bedrock-mantle branch) — handled by explicit `pop` in those flows.
}

/// Mirrors `from hermes_cli.providers import custom_provider_slug` — stub.
pub fn custom_provider_slug_stub(name: &str, _provider_key: &str) -> String {
    // Python: slugifies custom provider name -> `custom:<slug>`.
    // Stub: lowercases and hyphens.
    let slug = name.trim().to_lowercase().replace(' ', "-");
    if slug.is_empty() {
        "custom".to_string()
    } else {
        format!("custom:{}", slug)
    }
}

// argparse / os / subprocess / urllib.parse — mirrors lines 24-27
// Rust: `std::env` covers `os`, `std::process::Command` covers `subprocess`,
// `url` crate would cover `urllib.parse` but NEVER cargo so we hand-parse.

pub fn urllib_parse_urlparse_stub(url: &str) -> (String, String) {
    // Minimal stub for `urllib.parse.urlparse` used in _model_flow_custom
    // (hostname + port extraction for custom key env). Keep 1:1 call site shape.
    let _ = url;
    (String::new(), String::new())
}

// ---------------------------------------------------------------------------
// BEDROCK_GEO_PREFIXES — mirrors lines 33-41
// ---------------------------------------------------------------------------
// Python: BEDROCK_GEO_PREFIXES = (
//     "us.", "eu.", "ap.", "apac.", "jp.", "ca.", "sa.", "me.", "af.",
// )

/// AWS cross-region inference profile prefixes. Any geo-prefixed profile only
/// routes from endpoints in its own geography, so the Bedrock picker must not
/// offer (e.g.) us.* profiles to an eu-central-2 endpoint — selecting one
/// produces a config AWS rejects regardless of credentials (#28156).
/// global.* routes from everywhere. Full set per the AWS cross-region
/// inference docs. Mirrors `BEDROCK_GEO_PREFIXES` (lines 39-41).
pub const BEDROCK_GEO_PREFIXES: &[&str] = &[
    "us.", "eu.", "ap.", "apac.", "jp.", "ca.", "sa.", "me.", "af.",
];

// ---------------------------------------------------------------------------
// bedrock_region_geo_prefix — mirrors lines 44-58
// ---------------------------------------------------------------------------

/// Map an AWS region name to its inference-profile geo prefix ('' = unknown).
/// Mirrors `bedrock_region_geo_prefix(region_name: str) -> str` (lines 44-58).
pub fn bedrock_region_geo_prefix(region_name: &str) -> String {
    let r = region_name.to_lowercase();
    // Mirrors `for geo, region_prefixes in ((\"us.\", (\"us-\", \"us_gov\")), ...)`
    let table: &[(&str, &[&str])] = &[
        ("us.", &["us-", "us_gov"]),
        ("eu.", &["eu-"]),
        ("ap.", &["ap-"]),
        ("ca.", &["ca-"]),
        ("sa.", &["sa-"]),
        ("me.", &["me-"]),
        ("af.", &["af-"]),
    ];
    for (geo, prefixes) in table {
        for p in *prefixes {
            if r.starts_with(p) {
                return geo.to_string();
            }
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// bedrock_model_routable_from_region — mirrors lines 61-78
// ---------------------------------------------------------------------------

/// True when *model_id* can be invoked from *region_name*'s endpoint.
/// Bare foundation-model ids and ``global.*`` profiles route from anywhere.
/// Geo-prefixed inference profiles (``us.*``, ``eu.*``, ...) only route from
/// endpoints in their own geography. Unknown region shapes hide nothing.
/// Mirrors `bedrock_model_routable_from_region(model_id: str, region_name: str) -> bool`
/// (lines 61-78).
pub fn bedrock_model_routable_from_region(model_id: &str, region_name: &str) -> bool {
    let mid = model_id.to_lowercase();
    let matched_geo = BEDROCK_GEO_PREFIXES
        .iter()
        .find(|p| mid.starts_with(**p))
        .copied();
    if matched_geo.is_none() || mid.starts_with("global.") {
        return true;
    }
    let matched_geo = matched_geo.unwrap();
    let geo = bedrock_region_geo_prefix(region_name);
    if geo.is_empty() {
        return true;
    }
    if geo == "ap." {
        // Asia-Pacific regions can carry ap./apac./jp. profile spellings.
        return matches!(matched_geo, "ap." | "apac." | "jp.");
    }
    matched_geo == geo
}

// ---------------------------------------------------------------------------
// _existing_api_key_for_model_flow — mirrors lines 81-85
// ---------------------------------------------------------------------------

/// Resolve an existing wizard credential without changing its storage.
/// Mirrors `_existing_api_key_for_model_flow(provider_id: str, pconfig) -> tuple[str, str]`
/// (lines 81-85).
pub fn existing_api_key_for_model_flow(
    provider_id: &str,
    pconfig: &ProviderConfigStub,
) -> (String, String) {
    // Python: from hermes_cli.auth import _resolve_api_key_provider_secret
    //         return _resolve_api_key_provider_secret(provider_id, pconfig)
    resolve_api_key_provider_secret_stub(provider_id, pconfig)
}

// Minimal ProviderConfig stub — mirrors `hermes_cli.auth.ProviderConfig`
// (only fields used by model flows: id/name/auth_type/api_key_env_vars).
#[derive(Debug, Clone)]
pub struct ProviderConfigStub {
    pub id: String,
    pub name: String,
    pub auth_type: String,
    pub api_key_env_vars: Vec<String>,
}

impl ProviderConfigStub {
    pub fn new(id: &str, name: &str, auth_type: &str, env_vars: &[&str]) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            auth_type: auth_type.to_string(),
            api_key_env_vars: env_vars.iter().map(|s| s.to_string()).collect(),
        }
    }
}

fn resolve_api_key_provider_secret_stub(
    provider_id: &str,
    pconfig: &ProviderConfigStub,
) -> (String, String) {
    // Mirrors `hermes_cli.auth._resolve_api_key_provider_secret` — checks
    // `pconfig.api_key_env_vars` via `get_env_value_prefer_dotenv` order,
    // then credential_pool.peek(). Stub: env var only, pool skipped in slice 1.
    let _ = provider_id;
    for env_var in &pconfig.api_key_env_vars {
        if let Some(v) = get_env_value_stub(env_var) {
            if !v.trim().is_empty() && has_usable_secret_stub(&v) {
                return (v, env_var.clone());
            }
        }
    }
    (String::new(), String::new())
}

fn get_env_value_stub(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn has_usable_secret_stub(v: &str) -> bool {
    let t = v.trim();
    if t.len() < 4 {
        return false;
    }
    let lower = t.to_lowercase();
    !matches!(
        lower.as_str(),
        "*" | "**" | "***" | "changeme" | "your_api_key" | "placeholder" | "dummy" | "null" | "none"
    )
}

// ---------------------------------------------------------------------------
// _prune_replaced_custom_model_config_credentials — mirrors lines 88-138
// ---------------------------------------------------------------------------

/// Drop stale ``model_config`` credentials from inactive custom pools.
/// ``model_config`` means "the credential currently stored under
/// ``model.api_key``". After an explicit custom-endpoint switch, any old
/// custom pool still carrying that source points at the previous endpoint and
/// can be selected before the freshly saved config is tried.
/// Mirrors `_prune_replaced_custom_model_config_credentials(base_url: str, *, provider_name: str = "") -> None`
/// (lines 88-138).
pub fn prune_replaced_custom_model_config_credentials(base_url: &str, provider_name: &str) {
    // Python: try:
    //     from agent.credential_pool import CUSTOM_POOL_PREFIX, get_custom_provider_pool_key
    //     from hermes_cli.auth import read_credential_pool, write_credential_pool
    //     active_pool_key = get_custom_provider_pool_key(base_url, provider_name=...)
    //     if not active_pool_key: return
    //     pools = read_credential_pool(None)
    //     for pool_key, entries in pools.items():
    //         if not isinstance(pool_key, str) or not pool_key.startswith(CUSTOM_POOL_PREFIX)
    //            or pool_key == active_pool_key or not isinstance(entries, list): continue
    //         retained = [e for e in entries if e.get("source") != "model_config"]
    //         if changed: write_credential_pool(pool_key, retained, removed_ids=...)
    // except Exception: return
    //
    // Rust slice 1 (NEVER cargo, no agent crate): stubbed. Preserve 1:1 early-return
    // shape and error swallow for audit; real pool sweep wired when agent crate
    // is linked in a later slice.
    let _ = (base_url, provider_name);
    // Early return mirrors `if not active_pool_key: return`
    if base_url.trim().is_empty() {
        return;
    }
    // Pool iteration would go here; stubbed.
}

// helpers that would be imported lazily inside the Python try block — stubs
pub const CUSTOM_POOL_PREFIX_STUB: &str = "custom:";

pub fn get_custom_provider_pool_key_stub(base_url: &str, provider_name: Option<&str>) -> String {
    let _ = provider_name;
    if base_url.trim().is_empty() {
        String::new()
    } else {
        format!("{}:{}", CUSTOM_POOL_PREFIX_STUB, base_url.trim())
    }
}

// ---------------------------------------------------------------------------
// _prompt_auth_credentials_choice — mirrors lines 141-176
// ---------------------------------------------------------------------------

/// Prompt for reuse / reauthenticate / cancel with the standard radio UI.
/// Returns one of ``"use"``, ``"reauth"``, ``"cancel"``. Falls back to a
/// numbered prompt when curses is unavailable (piped stdin, non-TTY).
/// Mirrors `_prompt_auth_credentials_choice(title: str) -> str` (lines 141-176).
pub fn prompt_auth_credentials_choice(title: &str) -> String {
    // Python: choices = ["Use existing credentials", "Reauthenticate (new OAuth login)", "Cancel"]
    //         try: from hermes_cli.setup import _curses_prompt_choice
    //              idx = _curses_prompt_choice(title, choices, 0)
    //              if idx >= 0: print(); return ("use","reauth","cancel")[idx]
    //         except Exception: pass
    //         print(title); numbered fallback; input("  Choice [1/2/3]: ")
    //
    // Rust: std only, no curses dep in slice 1 (NEVER cargo). Preserve 1:1
    // fallback numbered path as the live path in this slice; curses path is
    // documented as the preferred path and will be wired when curses_ui slice
    // is linked.

    let choices = [
        "Use existing credentials",
        "Reauthenticate (new OAuth login)",
        "Cancel",
    ];

    // Stub curses path: try curses_prompt_choice if available, else -1
    let curses_idx = curses_prompt_choice_stub(title, &choices, 0);
    if curses_idx >= 0 {
        println!();
        return match curses_idx {
            0 => "use".to_string(),
            1 => "reauth".to_string(),
            2 => "cancel".to_string(),
            _ => "use".to_string(),
        };
    }

    println!("{}", title);
    for (i, label) in choices.iter().enumerate() {
        let marker = if i == 0 { "→" } else { " " };
        println!("  {marker} {}. {label}", i + 1);
    }
    println!();
    let choice = read_choice_input("  Choice [1/2/3]: ");
    match choice.as_str() {
        "2" => "reauth".to_string(),
        "3" => "cancel".to_string(),
        _ => "use".to_string(),
    }
}

fn curses_prompt_choice_stub(_title: &str, _choices: &[&str], _default: usize) -> i32 {
    // Mirrors `from hermes_cli.setup import _curses_prompt_choice` — curses
    // radiolist. Returns -1 when curses unavailable so caller falls through
    // to numbered fallback. Real curses impl wired in a later slice.
    -1
}

fn read_choice_input(prompt: &str) -> String {
    use std::io::{self, Write};
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut buf = String::new();
    match io::stdin().read_line(&mut buf) {
        Ok(_) => buf.trim().to_string(),
        Err(_) => "1".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Helpers shared by all _model_flow_* — mirrors lazy imports inside each flow
// ---------------------------------------------------------------------------

fn prompt_api_key_stub(
    pconfig: &ProviderConfigStub,
    existing_key: &str,
    provider_id: &str,
    existing_source: &str,
) -> (String, bool) {
    // Mirrors `from hermes_cli.main import _prompt_api_key` call inside flows
    // (e.g. line 181 in openrouter). Returns (resolved_key, abort).
    // Stub: if existing_key present, keep it (no prompt); abort false.
    // Real impl prompts K/R/C (keep/replace/cancel) via masked_secret_prompt.
    let _ = (pconfig, provider_id, existing_source);
    if !existing_key.is_empty() {
        (existing_key.to_string(), false)
    } else {
        // No existing key — in live flow would prompt for key; stub keeps empty
        // so caller still shows the "Get one at: ..." banner from Python.
        (String::new(), false)
    }
}

fn prompt_model_selection_stub(
    models: &[String],
    current_model: &str,
    _pricing: Option<&HashMap<String, String>>,
    _confirm_provider: Option<&str>,
    _confirm_base_url: Option<&str>,
    _confirm_api_key: Option<&str>,
    _unavailable: Option<&[String]>,
) -> Option<String> {
    // Mirrors `from hermes_cli.auth import _prompt_model_selection` — curses
    // searchable radiolist with confirm step. Stub: pick current_model if
    // present else first model. Returns None when `models` empty (caller
    // prints "No change.").
    if models.is_empty() {
        return None;
    }
    if !current_model.is_empty() && models.contains(&current_model.to_string()) {
        return Some(current_model.to_string());
    }
    Some(models[0].clone())
}

fn save_model_choice_stub(model: &str) {
    // Mirrors `from hermes_cli.auth import _save_model_choice`
    // Persists `model` to `~/.hermes/config.yaml` model.default + history.
    // Stub: no-op in slice 1 (config persistence wired via config_slice later).
    let _ = model;
}

fn deactivate_provider_stub() {
    // Mirrors `from hermes_cli.auth import deactivate_provider`
    // Clears `auth.json` active_provider so the next resolve_provider() falls
    // through to env/config.
    // Stub: no-op.
}

fn model_ids_stub(force_refresh: bool) -> Vec<String> {
    // Mirrors `from hermes_cli.models import model_ids(force_refresh=True)`
    // Stub: curated fallback catalog (real live fetch via /models endpoint).
    let _ = force_refresh;
    Vec::new()
}

fn get_pricing_for_provider_stub(_provider: &str, _force_refresh: bool) -> HashMap<String, String> {
    HashMap::new()
}

fn load_config_stub() -> HashMap<String, HashMap<String, String>> {
    HashMap::new()
}

fn save_config_stub(_cfg: &HashMap<String, HashMap<String, String>>) {}

fn is_interactive_cancel_input(s: &str) -> bool {
    matches!(s.trim().to_lowercase().as_str(), "3" | "cancel" | "q" | "quit")
}

// ---------------------------------------------------------------------------
// _model_flow_openrouter — mirrors lines 179-247
// ---------------------------------------------------------------------------

/// OpenRouter provider: ensure API key, then pick model.
/// Mirrors `_model_flow_openrouter(config, current_model="")` (lines 179-247).
pub fn model_flow_openrouter(
    config: &mut HashMap<String, HashMap<String, String>>,
    current_model: &str,
) {
    // Python:
    // from hermes_cli.main import _prompt_api_key
    // from hermes_constants import OPENROUTER_BASE_URL
    // from hermes_cli.auth import ProviderConfig, _prompt_model_selection, _save_model_choice, deactivate_provider
    // pconfig = ProviderConfig(id="openrouter", name="OpenRouter", auth_type="api_key", api_key_env_vars=("OPENROUTER_API_KEY",))
    // existing_key, existing_source = _existing_api_key_for_model_flow("openrouter", pconfig)
    // if not existing_key: print("Get one at: https://openrouter.ai/keys"); print()
    // _resolved, abort = _prompt_api_key(pconfig, existing_key, provider_id="openrouter", existing_source=existing_source)
    // if abort: return
    // openrouter_models = model_ids(force_refresh=True)
    // pricing = get_pricing_for_provider("openrouter", force_refresh=True)
    // selected = _prompt_model_selection(openrouter_models, current_model=..., pricing=..., confirm_provider="openrouter", ...)
    // if selected: _save_model_choice(selected); load_config/save_config provider=openrouter base_url=OPENROUTER... api_mode=chat_completions; clear_model_endpoint_credentials; deactivate_provider(); print(...)
    // else: print("No change.")

    const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";

    let pconfig = ProviderConfigStub::new(
        "openrouter",
        "OpenRouter",
        "api_key",
        &["OPENROUTER_API_KEY"],
    );
    let (existing_key, existing_source) = existing_api_key_for_model_flow("openrouter", &pconfig);
    if existing_key.is_empty() {
        println!("Get one at: https://openrouter.ai/keys");
        println!();
    }
    let (resolved, abort) =
        prompt_api_key_stub(&pconfig, &existing_key, "openrouter", &existing_source);
    if abort {
        return;
    }
    let effective_key = if !resolved.is_empty() { resolved } else { existing_key.clone() };

    let openrouter_models = model_ids_stub(true);
    let pricing = get_pricing_for_provider_stub("openrouter", true);

    let selected = prompt_model_selection_stub(
        &openrouter_models,
        current_model,
        Some(&pricing),
        Some("openrouter"),
        Some(OPENROUTER_BASE_URL),
        Some(&effective_key),
        None,
    );

    if let Some(sel) = selected {
        save_model_choice_stub(&sel);
        let mut cfg = load_config_stub();
        // Mirrors: cfg = load_config(); model = cfg.get("model"); if not dict: model={}; cfg["model"]=model
        //          model["provider"]="openrouter"; model["base_url"]=OPENROUTER_BASE_URL; model["api_mode"]="chat_completions"
        //          clear_model_endpoint_credentials(model, clear_api_mode=False); save_config(cfg); deactivate_provider()
        let model = cfg.entry("model".to_string()).or_default();
        model.insert("provider".to_string(), "openrouter".to_string());
        model.insert("base_url".to_string(), OPENROUTER_BASE_URL.to_string());
        model.insert("api_mode".to_string(), "chat_completions".to_string());
        // clear_model_endpoint_credentials(model, clear_api_mode=False) — remove api_key
        model.remove("api_key");
        save_config_stub(&cfg);
        // Sync caller's config dict so setup wizard's final save_config preserves settings (#4172 pattern)
        let caller_model = config.entry("model".to_string()).or_default();
        caller_model.insert("provider".to_string(), "openrouter".to_string());
        caller_model.insert("base_url".to_string(), OPENROUTER_BASE_URL.to_string());
        caller_model.insert("api_mode".to_string(), "chat_completions".to_string());
        deactivate_provider_stub();
        println!("Default model set to: {sel} (via OpenRouter)");
    } else {
        println!("No change.");
    }
}

// ---------------------------------------------------------------------------
// _print_moa_preset — mirrors lines 250-257
// ---------------------------------------------------------------------------

/// Print the full reference-models + aggregator breakdown for a preset.
/// Mirrors `_print_moa_preset(name: str, preset: dict) -> None` (lines 250-257).
pub fn print_moa_preset(name: &str, preset: &MoaPresetStub) {
    println!("  Preset: {name}");
    println!("  Reference models:");
    for (idx, slot) in preset.reference_models.iter().enumerate() {
        println!("    {}. {}:{}", idx + 1, slot.provider, slot.model);
    }
    println!(
        "  Aggregator:  {}:{}",
        preset.aggregator.provider, preset.aggregator.model
    );
}

#[derive(Debug, Clone, Default)]
pub struct MoaSlotStub {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Default)]
pub struct MoaPresetStub {
    pub reference_models: Vec<MoaSlotStub>,
    pub aggregator: MoaSlotStub,
}

// ---------------------------------------------------------------------------
// _model_flow_ai_gateway — mirrors lines 260-311
// ---------------------------------------------------------------------------

/// Vercel AI Gateway provider: ensure API key, then pick model with pricing.
/// Mirrors `_model_flow_ai_gateway(config, current_model="")` (lines 260-311).
pub fn model_flow_ai_gateway(
    config: &mut HashMap<String, HashMap<String, String>>,
    current_model: &str,
) {
    // Python: from hermes_constants import AI_GATEWAY_BASE_URL
    //         from hermes_cli.main import _prompt_api_key
    //         from hermes_cli.auth import PROVIDER_REGISTRY, _prompt_model_selection, ...
    //         from hermes_cli.config import get_env_value
    //         pconfig = PROVIDER_REGISTRY["ai-gateway"]; existing_key = get_env_value("AI_GATEWAY_API_KEY") or ""
    //         if not existing_key: print create-key banner; _prompt_api_key(...); if abort: return
    //         models_list = ai_gateway_model_ids(force_refresh=True); pricing = get_pricing...
    //         selected = _prompt_model_selection(models_list, current_model=..., pricing=pricing)
    //         if selected: _save_model_choice; load_config/save_config provider=ai-gateway base_url=AI_GATEWAY...

    const AI_GATEWAY_BASE_URL: &str = "https://ai-gateway.vercel.sh/v1";

    let pconfig = ProviderConfigStub::new(
        "ai-gateway",
        "Vercel AI Gateway",
        "api_key",
        &["AI_GATEWAY_API_KEY"],
    );
    let existing_key = get_env_value_stub("AI_GATEWAY_API_KEY").unwrap_or_default();
    if existing_key.is_empty() {
        println!(
            "Create API key here: https://vercel.com/d?to=%2F%5Bteam%5D%2F%7E%2Fai-gateway&title=AI+Gateway"
        );
        println!("Add a payment method to get $5 in free credits.");
        println!();
    }
    let (resolved, abort) =
        prompt_api_key_stub(&pconfig, &existing_key, "ai-gateway", "");
    if abort {
        return;
    }
    let _effective_key = if !resolved.is_empty() { resolved } else { existing_key.clone() };

    let models_list = ai_gateway_model_ids_stub(true);
    let pricing = get_pricing_for_provider_stub("ai-gateway", true);

    let selected = prompt_model_selection_stub(
        &models_list,
        current_model,
        Some(&pricing),
        None,
        None,
        None,
        None,
    );

    if let Some(sel) = selected {
        save_model_choice_stub(&sel);
        let mut cfg = load_config_stub();
        let model = cfg.entry("model".to_string()).or_default();
        model.insert("provider".to_string(), "ai-gateway".to_string());
        model.insert("base_url".to_string(), AI_GATEWAY_BASE_URL.to_string());
        model.insert("api_mode".to_string(), "chat_completions".to_string());
        save_config_stub(&cfg);
        let caller_model = config.entry("model".to_string()).or_default();
        caller_model.insert("provider".to_string(), "ai-gateway".to_string());
        caller_model.insert("base_url".to_string(), AI_GATEWAY_BASE_URL.to_string());
        caller_model.insert("api_mode".to_string(), "chat_completions".to_string());
        deactivate_provider_stub();
        println!("Default model set to: {sel} (via Vercel AI Gateway)");
    } else {
        println!("No change.");
    }
}

fn ai_gateway_model_ids_stub(_force_refresh: bool) -> Vec<String> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// _model_flow_moa — mirrors lines 314-396
// ---------------------------------------------------------------------------

/// Mixture of Agents virtual provider: pick a preset, then persist it.
/// Unlike the other provider flows there is no credential step — MoA is a
/// virtual provider whose presets reference already-configured providers. We
/// always show the preset list (even when there is only one) so the user sees
/// what they are selecting, then print the full preset breakdown on selection.
/// Mirrors `_model_flow_moa(config, current_model="")` (lines 314-396).
pub fn model_flow_moa(
    config: &mut HashMap<String, HashMap<String, String>>,
    _current_model: &str,
) {
    // Python: from hermes_cli.auth import _save_model_choice, deactivate_provider
    //         from hermes_cli.config import load_config, save_config
    //         from hermes_cli.moa_config import normalize_moa_config
    //         moa = normalize_moa_config(config.get("moa") if isinstance(config, dict) else {})
    //         presets = moa.get("presets") or {}
    //         if not presets: print("No MoA presets..."); return
    //         names = list(presets.keys()); default_name = moa.get("default_preset") or names[0]
    //         rows = [f"{n}  (agg {agg}, {ref_count} refs){suffix}" for n in names]
    //         try: from hermes_cli.setup import _curses_prompt_choice; idx = _curses...; except: fallback numbered
    //         if idx None/<0: print("No change."); return
    //         selected_name = names[idx]; preset = presets[selected_name]
    //         cfg = load_config(); model=cfg.get("model"); model["default"]=selected_name; model["provider"]="moa"
    //         clear_model_endpoint_credentials(model, clear_api_mode=True); model.pop("base_url", None); save_config(cfg); _save_model_choice
    //         print(f"Default model set to: {selected_name} (via Mixture of Agents)"); _print_moa_preset

    let moa = normalize_moa_config_stub(config);
    if moa.presets.is_empty() {
        println!("No MoA presets configured. Run `hermes moa configure <name>` first.");
        return;
    }

    let names: Vec<String> = moa.presets.keys().cloned().collect();
    let default_name = moa
        .default_preset
        .clone()
        .unwrap_or_else(|| names[0].clone());

    // Build labelled rows showing aggregator (lines 337-343)
    let mut rows: Vec<String> = Vec::new();
    for n in &names {
        if let Some(preset) = moa.presets.get(n) {
            let agg_label = if preset.aggregator.model.is_empty() {
                String::new()
            } else {
                format!("{}:{}", preset.aggregator.provider, preset.aggregator.model)
            };
            let ref_count = preset.reference_models.len();
            let suffix = if *n == default_name { "  ← default" } else { "" };
            rows.push(format!("{n}  (agg {agg_label}, {ref_count} refs){suffix}"));
        }
    }

    let default_idx = names
        .iter()
        .position(|n| *n == default_name)
        .unwrap_or(0);

    // Try curses, else fallback numbered (mirrors lines 347-368)
    let idx = curses_prompt_choice_stub("Select a Mixture of Agents preset:", &rows.iter().map(|s| s.as_str()).collect::<Vec<_>>(), default_idx);
    let idx: Option<i32> = if idx >= 0 {
        Some(idx)
    } else {
        // Fallback numbered menu (lines 351-368)
        println!("Select a Mixture of Agents preset:");
        for (i, row) in rows.iter().enumerate() {
            let marker = if i == default_idx { "→" } else { " " };
            println!("  {marker} {}. {row}", i + 1);
        }
        let raw = read_choice_input(&format!("  Choice [1-{}]: ", rows.len()));
        if raw.is_empty() {
            Some(default_idx as i32)
        } else if let Ok(n) = raw.parse::<i32>() {
            if n >= 1 && n <= rows.len() as i32 {
                Some(n - 1)
            } else {
                println!("No change.");
                return;
            }
        } else {
            println!("No change.");
            return;
        }
    };

    let idx = match idx {
        Some(v) if v >= 0 => v as usize,
        _ => {
            println!("No change.");
            return;
        }
    };
    if idx >= names.len() {
        println!("No change.");
        return;
    }

    let selected_name = names[idx].clone();
    let preset = moa.presets.get(&selected_name).cloned().unwrap_or_default();

    let mut cfg = load_config_stub();
    let model = cfg.entry("model".to_string()).or_default();
    model.insert("default".to_string(), selected_name.clone());
    model.insert("provider".to_string(), "moa".to_string());
    // clear_model_endpoint_credentials(model, clear_api_mode=True)
    model.remove("api_key");
    model.remove("api_mode");
    model.remove("base_url");
    save_config_stub(&cfg);
    save_model_choice_stub(&selected_name);
    deactivate_provider_stub();

    // Sync caller config (mirrors Python not explicitly but keeps wizard state)
    let caller_model = config.entry("model".to_string()).or_default();
    caller_model.insert("default".to_string(), selected_name.clone());
    caller_model.insert("provider".to_string(), "moa".to_string());
    caller_model.remove("api_key");
    caller_model.remove("api_mode");
    caller_model.remove("base_url");

    println!();
    println!("Default model set to: {selected_name} (via Mixture of Agents)");
    print_moa_preset(&selected_name, &preset);
}

#[derive(Debug, Clone, Default)]
struct MoaConfigStub {
    presets: HashMap<String, MoaPresetStub>,
    default_preset: Option<String>,
}

fn normalize_moa_config_stub(
    _config: &HashMap<String, HashMap<String, String>>,
) -> MoaConfigStub {
    // Mirrors `from hermes_cli.moa_config import normalize_moa_config`
    // Real impl reads `config["moa"]` dict; stub returns empty.
    MoaConfigStub::default()
}

// ---------------------------------------------------------------------------
// _model_flow_nous — mirrors lines 399-622
// ---------------------------------------------------------------------------

/// Nous Portal provider: ensure logged in, then pick model.
/// Mirrors `_model_flow_nous(config, current_model="", args=None)` (lines 399-622).
pub fn model_flow_nous(
    config: &mut HashMap<String, HashMap<String, String>>,
    current_model: &str,
    args: Option<&NousArgsStub>,
) {
    // Python imports (lines 401-418): get_provider_auth_state, _prompt_model_selection,
    // _save_model_choice, _update_config_for_provider, resolve_nous_runtime_credentials,
    // AuthError/format_auth_error, _login_nous, PROVIDER_REGISTRY,
    // get_env_value/load_config/save_config/save_env_value,
    // prompt_enable_tool_gateway

    let state = get_provider_auth_state_stub("nous");
    if state.is_none() || state.as_ref().and_then(|m| m.get("access_token")).map(|v| v.is_empty()).unwrap_or(true) {
        println!("Not logged into Nous Portal. Starting login...");
        println!();
        // Mirrors mock_args = argparse.Namespace(portal_url=..., inference_url=..., client_id=..., scope=..., no_browser=..., timeout=..., ca_bundle=..., insecure=...)
        //         _login_nous(mock_args, PROVIDER_REGISTRY["nous"])
        match login_nous_stub(args) {
            Ok(()) => {
                // Offer Tool Gateway enablement for paid subscribers
                let _ = prompt_enable_tool_gateway_stub(config);
            }
            Err(e) if e == "SystemExit" => {
                println!("Login cancelled or failed.");
                return;
            }
            Err(e) => {
                println!("Login failed: {e}");
                return;
            }
        }
        return;
    }

    // Already logged in — curated model list (lines 454-466)
    let model_ids = get_curated_nous_model_ids_stub();
    if model_ids.is_empty() {
        println!("No curated models available for Nous Portal.");
        return;
    }

    // Verify credentials (lines 468-493)
    let mut creds = match resolve_nous_runtime_credentials_stub(false) {
        Ok(c) => c,
        Err(e) => {
            let relogin_required = e.contains("relogin");
            if relogin_required {
                println!("Session expired: {e}");
                println!("Re-authenticating with Nous Portal...\n");
                match login_nous_stub(None) {
                    Ok(()) => return,
                    Err(login_exc) => {
                        println!("Re-login failed: {login_exc}");
                        return;
                    }
                }
            }
            println!("Could not verify credentials: {e}");
            return;
        }
    };

    let pricing = get_pricing_for_provider_stub("nous", false);
    let free_tier = check_nous_free_tier_stub(true);
    if !free_tier {
        // Force fresh account data after purchase (lines 502-511)
        if let Ok(refreshed) = resolve_nous_runtime_credentials_stub(true) {
            if !refreshed.is_empty() {
                creds = refreshed;
            }
        }
    }

    // Resolve portal URL (lines 515-521)
    let nous_portal_url = get_provider_auth_state_stub("nous")
        .and_then(|m| m.get("portal_base_url").cloned())
        .unwrap_or_default();

    // Free/paid tier augmentation (lines 534-561)
    let mut model_ids_mut = model_ids.clone();
    let mut pricing_mut = pricing.clone();
    let mut unavailable_models: Vec<String> = Vec::new();
    let mut unavailable_message = String::new();
    if free_tier {
        unavailable_message = get_nous_portal_entitlement_message_stub();
        let (aug_ids, aug_pricing) =
            union_with_portal_free_recommendations_stub(&model_ids_mut, &pricing_mut, &nous_portal_url);
        model_ids_mut = aug_ids;
        pricing_mut = aug_pricing;
        let (partitioned_ids, unavailable) =
            partition_nous_models_by_tier_stub(&model_ids_mut, &pricing_mut, true);
        model_ids_mut = partitioned_ids;
        unavailable_models = unavailable;
    } else {
        let (aug_ids, aug_pricing) =
            union_with_portal_paid_recommendations_stub(&model_ids_mut, &pricing_mut, &nous_portal_url);
        model_ids_mut = aug_ids;
        pricing_mut = aug_pricing;
    }

    if model_ids_mut.is_empty() && unavailable_models.is_empty() {
        println!("No models available for Nous Portal after filtering.");
        return;
    }
    if free_tier && model_ids_mut.is_empty() {
        println!("No free models currently available.");
        if !unavailable_models.is_empty() {
            let url = if nous_portal_url.is_empty() {
                DEFAULT_NOUS_PORTAL_URL.to_string()
            } else {
                nous_portal_url.trim_end_matches('/').to_string()
            };
            if unavailable_message.is_empty() {
                println!("Upgrade at {url} to access paid models.");
            } else {
                println!("{unavailable_message}");
            }
        }
        return;
    }

    println!(
        "Showing {} curated models — use \"Enter custom model name\" for others.",
        model_ids_mut.len()
    );

    let base_url = creds.get("base_url").cloned().unwrap_or_default();
    let api_key = creds.get("api_key").cloned().unwrap_or_default();
    let selected = prompt_model_selection_stub(
        &model_ids_mut,
        current_model,
        Some(&pricing_mut),
        Some("nous"),
        Some(&base_url),
        Some(&api_key),
        Some(&unavailable_models),
    );

    if let Some(sel) = selected {
        save_model_choice_stub(&sel);
        update_config_for_provider_stub("nous", &base_url);
        let mut cfg = load_config_stub();
        let model = cfg.entry("model".to_string()).or_default();
        model.insert("provider".to_string(), "nous".to_string());
        model.insert("default".to_string(), sel.clone());
        if !base_url.trim().is_empty() {
            model.insert("base_url".to_string(), base_url.trim_end_matches('/').to_string());
        } else {
            model.remove("base_url");
        }
        model.remove("api_key");
        model.remove("api_mode");
        // Clear OPENAI_BASE_URL / OPENAI_API_KEY if present
        // Mirrors lines 615-616: if get_env_value("OPENAI_BASE_URL"): save_env_value("OPENAI_BASE_URL","")
        if get_env_value_stub("OPENAI_BASE_URL").is_some() {
            save_env_value_stub("OPENAI_BASE_URL", "");
            save_env_value_stub("OPENAI_API_KEY", "");
        }
        save_config_stub(&cfg);
        // Sync caller
        let caller = config.entry("model".to_string()).or_default();
        caller.insert("provider".to_string(), "nous".to_string());
        caller.insert("default".to_string(), sel.clone());
        println!("Default model set to: {sel} (via Nous Portal)");
        let _ = prompt_enable_tool_gateway_stub(config);
    } else {
        println!("No change.");
    }
}

const DEFAULT_NOUS_PORTAL_URL: &str = "https://portal.nousresearch.com";

#[derive(Debug, Clone, Default)]
pub struct NousArgsStub {
    pub portal_url: Option<String>,
    pub inference_url: Option<String>,
    pub client_id: Option<String>,
    pub scope: Option<String>,
    pub no_browser: bool,
    pub timeout: Option<f64>,
    pub ca_bundle: Option<String>,
    pub insecure: bool,
}

fn get_provider_auth_state_stub(_provider: &str) -> Option<HashMap<String, String>> {
    None
}

fn login_nous_stub(_args: Option<&NousArgsStub>) -> Result<(), String> {
    Err("not logged in stub".to_string())
}

fn prompt_enable_tool_gateway_stub(
    _config: &HashMap<String, HashMap<String, String>>,
) -> Result<(), String> {
    Ok(())
}

fn get_curated_nous_model_ids_stub() -> Vec<String> {
    Vec::new()
}

fn resolve_nous_runtime_credentials_stub(
    _force_refresh: bool,
) -> Result<HashMap<String, String>, String> {
    Err("no credentials".to_string())
}

fn check_nous_free_tier_stub(_force_fresh: bool) -> bool {
    false
}

fn get_nous_portal_entitlement_message_stub() -> String {
    String::new()
}

fn union_with_portal_free_recommendations_stub(
    ids: &[String],
    pricing: &HashMap<String, String>,
    _portal_url: &str,
) -> (Vec<String>, HashMap<String, String>) {
    (ids.to_vec(), pricing.clone())
}

fn union_with_portal_paid_recommendations_stub(
    ids: &[String],
    pricing: &HashMap<String, String>,
    _portal_url: &str,
) -> (Vec<String>, HashMap<String, String>) {
    (ids.to_vec(), pricing.clone())
}

fn partition_nous_models_by_tier_stub(
    ids: &[String],
    _pricing: &HashMap<String, String>,
    _free_tier: bool,
) -> (Vec<String>, Vec<String>) {
    (ids.to_vec(), Vec::new())
}

fn update_config_for_provider_stub(_provider: &str, _url: &str) {}
fn save_env_value_stub(_k: &str, _v: &str) {}

// ---------------------------------------------------------------------------
// _model_flow_openai_codex — mirrors lines 624-710
// ---------------------------------------------------------------------------

/// OpenAI Codex provider: ensure logged in, then pick model.
/// Mirrors `_model_flow_openai_codex(config, current_model="")` (lines 624-710).
pub fn model_flow_openai_codex(
    _config: &mut HashMap<String, HashMap<String, String>>,
    current_model: &str,
) {
    // Python: status = get_codex_auth_status(); if logged_in: print creds ✓; choice=_prompt_auth_credentials_choice(...)
    //         if reauth: _login_openai_codex(force_new_login=True); elif cancel: return
    //         else: _login_openai_codex()
    //         _codex_token = get_codex_auth_status().api_key or resolve_codex_runtime_credentials
    //         codex_models = get_codex_model_ids(access_token=_codex_token)
    //         selected = _prompt_model_selection(codex_models, current_model=..., confirm_provider="openai-codex", ...)
    //         if selected: _save_model_choice; _update_config_for_provider("openai-codex", DEFAULT_CODEX_BASE_URL)

    const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

    let status = get_codex_auth_status_stub();
    let logged_in = status.get("logged_in").map(|v| v == "true").unwrap_or(false);

    if logged_in {
        println!("  OpenAI Codex credentials: ✓");
        println!();
        let choice = prompt_auth_credentials_choice("OpenAI Codex credentials:");
        if choice == "reauth" {
            println!("Starting a fresh OpenAI Codex login...");
            println!();
            match login_openai_codex_stub(true) {
                Ok(()) => {}
                Err(e) if e == "SystemExit" => {
                    println!("Login cancelled or failed.");
                    return;
                }
                Err(e) => {
                    println!("Login failed: {e}");
                    return;
                }
            }
            let s2 = get_codex_auth_status_stub();
            if s2.get("logged_in").map(|v| v != "true").unwrap_or(true) {
                println!("Login failed.");
                return;
            }
        } else if choice == "cancel" {
            return;
        }
    } else {
        println!("Not logged into OpenAI Codex. Starting login...");
        println!();
        match login_openai_codex_stub(false) {
            Ok(()) => {}
            Err(e) if e == "SystemExit" => {
                println!("Login cancelled or failed.");
                return;
            }
            Err(e) => {
                println!("Login failed: {e}");
                return;
            }
        }
    }

    let codex_token = get_codex_token_stub();
    let codex_models = get_codex_model_ids_stub(&codex_token);

    let selected = prompt_model_selection_stub(
        &codex_models,
        current_model,
        None,
        Some("openai-codex"),
        Some(DEFAULT_CODEX_BASE_URL),
        Some(&codex_token),
        None,
    );

    if let Some(sel) = selected {
        save_model_choice_stub(&sel);
        update_config_for_provider_stub("openai-codex", DEFAULT_CODEX_BASE_URL);
        println!("Default model set to: {sel} (via OpenAI Codex)");
    } else {
        println!("No change.");
    }
}

fn get_codex_auth_status_stub() -> HashMap<String, String> {
    HashMap::new()
}

fn login_openai_codex_stub(_force_new_login: bool) -> Result<(), String> {
    Err("not implemented".to_string())
}

fn get_codex_token_stub() -> String {
    // Mirrors: try get_codex_auth_status().api_key else resolve_codex_runtime_credentials
    String::new()
}

fn get_codex_model_ids_stub(_access_token: &str) -> Vec<String> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// _model_flow_xai_oauth — mirrors lines 712-791
// ---------------------------------------------------------------------------

/// xAI Grok OAuth (SuperGrok / Premium+) provider: ensure logged in, then pick model.
/// Mirrors `_model_flow_xai_oauth(_config, current_model="", *, args=None)` (lines 712-791).
pub fn model_flow_xai_oauth(
    _config: &mut HashMap<String, HashMap<String, String>>,
    current_model: &str,
    args: Option<&XaiArgsStub>,
) {
    const DEFAULT_XAI_OAUTH_BASE_URL: &str = "https://api.x.ai/v1";

    let status = get_xai_oauth_auth_status_stub();
    let logged_in = status.get("logged_in").map(|v| v == "true").unwrap_or(false);

    if logged_in {
        println!("  xAI Grok OAuth (SuperGrok / Premium+) credentials: ✓");
        println!();
        let choice = prompt_auth_credentials_choice(
            "xAI Grok OAuth (SuperGrok / Premium+) credentials:",
        );
        if choice == "reauth" {
            println!("Starting a fresh xAI OAuth login...");
            println!();
            match login_xai_oauth_stub(args, true) {
                Ok(()) => {}
                Err(e) if e == "SystemExit" => {
                    println!("Login cancelled or failed.");
                    return;
                }
                Err(e) => {
                    println!("Login failed: {e}");
                    return;
                }
            }
        } else if choice == "cancel" {
            return;
        }
    } else {
        println!("Not logged into xAI Grok OAuth (SuperGrok / Premium+). Starting login...");
        println!();
        match login_xai_oauth_stub(args, false) {
            Ok(()) => {}
            Err(e) if e == "SystemExit" => {
                println!("Login cancelled or failed.");
                return;
            }
            Err(e) => {
                println!("Login failed: {e}");
                return;
            }
        }
    }

    // Resolve base_url (lines 777-782) — fallback to default when pool-only creds
    let base_url = resolve_xai_oauth_runtime_credentials_stub()
        .map(|m| m.get("base_url").cloned().unwrap_or_default())
        .unwrap_or_else(|_| DEFAULT_XAI_OAUTH_BASE_URL.to_string());
    let base_url = if base_url.trim().is_empty() {
        DEFAULT_XAI_OAUTH_BASE_URL.to_string()
    } else {
        base_url.trim().trim_end_matches('/').to_string()
    };

    let models = provider_model_ids_stub("xai-oauth");
    let default_model = if current_model.is_empty() {
        models.first().cloned().unwrap_or_else(|| "grok-4.6".to_string())
    } else {
        current_model.to_string()
    };
    let selected = prompt_model_selection_stub(&models, &default_model, None, None, None, None, None);

    if let Some(sel) = selected {
        save_model_choice_stub(&sel);
        update_config_for_provider_stub("xai-oauth", &base_url);
        println!(
            "Default model set to: {sel} (via xAI Grok OAuth — SuperGrok / Premium+)"
        );
    } else {
        println!("No change.");
    }
}

#[derive(Debug, Clone, Default)]
pub struct XaiArgsStub {
    pub no_browser: bool,
    pub timeout: Option<f64>,
}

fn get_xai_oauth_auth_status_stub() -> HashMap<String, String> {
    HashMap::new()
}

fn login_xai_oauth_stub(
    _args: Option<&XaiArgsStub>,
    _force_new_login: bool,
) -> Result<(), String> {
    Err("not implemented".to_string())
}

fn resolve_xai_oauth_runtime_credentials_stub() -> Result<HashMap<String, String>, String> {
    Err("no creds".to_string())
}

fn provider_model_ids_stub(_provider: &str) -> Vec<String> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// _model_flow_qwen_oauth — mirrors lines 793-839
// ---------------------------------------------------------------------------

/// Qwen OAuth provider: reuse local Qwen CLI login, then pick model.
/// Mirrors `_model_flow_qwen_oauth(_config, current_model="")` (lines 793-839).
pub fn model_flow_qwen_oauth(
    _config: &mut HashMap<String, HashMap<String, String>>,
    current_model: &str,
) {
    const DEFAULT_QWEN_BASE_URL: &str = "https://portal.qwen.ai/v1";
    const DEFAULT_QWEN_MODELS: &[&str] = &["qwen3-coder-plus"];

    let status = get_qwen_auth_status_stub();
    let logged_in = status.get("logged_in").map(|v| v == "true").unwrap_or(false);
    if !logged_in {
        println!("Not logged into Qwen CLI OAuth.");
        println!("Run: qwen auth qwen-oauth");
        if let Some(auth_file) = status.get("auth_file") {
            println!("Expected credentials file: {auth_file}");
        }
        if let Some(err) = status.get("error") {
            println!("Error: {err}");
        }
        return;
    }

    // Try live model discovery, fall back to curated list (lines 819-825)
    let mut models: Vec<String> = Vec::new();
    if let Ok(creds) = resolve_qwen_runtime_credentials_stub(true) {
        if let (Some(api_key), Some(base_url)) = (creds.get("api_key"), creds.get("base_url")) {
            models = fetch_api_models_stub(api_key, base_url);
        }
    }
    if models.is_empty() {
        models = DEFAULT_QWEN_MODELS.iter().map(|s| s.to_string()).collect();
    }

    let default = if current_model.is_empty() {
        models.first().cloned().unwrap_or_else(|| "qwen3-coder-plus".to_string())
    } else {
        current_model.to_string()
    };

    let selected = prompt_model_selection_stub(
        &models,
        &default,
        None,
        Some("qwen-oauth"),
        Some(DEFAULT_QWEN_BASE_URL),
        None,
        None,
    );

    if let Some(sel) = selected {
        save_model_choice_stub(&sel);
        update_config_for_provider_stub("qwen-oauth", DEFAULT_QWEN_BASE_URL);
        println!("Default model set to: {sel} (via Qwen OAuth)");
    } else {
        println!("No change.");
    }
}

fn get_qwen_auth_status_stub() -> HashMap<String, String> {
    HashMap::new()
}

fn resolve_qwen_runtime_credentials_stub(
    _refresh_if_expiring: bool,
) -> Result<HashMap<String, String>, String> {
    Err("no creds".to_string())
}

fn fetch_api_models_stub(_api_key: &str, _base_url: &str) -> Vec<String> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// _model_flow_minimax_oauth — mirrors lines 841-893
// ---------------------------------------------------------------------------

/// MiniMax OAuth provider: ensure logged in, then pick model.
/// Mirrors `_model_flow_minimax_oauth(config, current_model="", args=None)` (lines 841-893).
pub fn model_flow_minimax_oauth(
    _config: &mut HashMap<String, HashMap<String, String>>,
    current_model: &str,
    args: Option<&MinimaxArgsStub>,
) {
    let state = get_provider_auth_state_stub("minimax-oauth");
    if state.is_none()
        || state
            .as_ref()
            .and_then(|m| m.get("access_token"))
            .map(|v| v.is_empty())
            .unwrap_or(true)
    {
        println!("Not logged into MiniMax. Starting OAuth login...");
        println!();
        match login_minimax_oauth_stub(args) {
            Ok(()) => {}
            Err(e) if e == "SystemExit" => {
                println!("Login cancelled or failed.");
                return;
            }
            Err(e) => {
                println!("Login failed: {e}");
                return;
            }
        }
    }

    let creds = match resolve_minimax_oauth_runtime_credentials_stub() {
        Ok(c) => c,
        Err(e) => {
            println!("{e}");
            return;
        }
    };

    let base_url = creds.get("base_url").cloned().unwrap_or_default();
    let model_ids = provider_models_minimax_oauth_stub();

    let selected = prompt_model_selection_stub(
        &model_ids,
        current_model,
        None,
        Some("minimax-oauth"),
        Some(&base_url),
        None,
        None,
    );

    if selected.is_none() {
        return;
    }
    let sel = selected.unwrap();
    save_model_choice_stub(&sel);
    update_config_for_provider_stub("minimax-oauth", &base_url);
    println!("✓ Using MiniMax model: {sel}");
}

#[derive(Debug, Clone, Default)]
pub struct MinimaxArgsStub {
    pub region: Option<String>,
    pub no_browser: bool,
    pub timeout: Option<f64>,
}

fn login_minimax_oauth_stub(_args: Option<&MinimaxArgsStub>) -> Result<(), String> {
    Err("not implemented".to_string())
}

fn resolve_minimax_oauth_runtime_credentials_stub() -> Result<HashMap<String, String>, String> {
    Err("no creds".to_string())
}

fn provider_models_minimax_oauth_stub() -> Vec<String> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// _model_flow_custom — mirrors lines 895-900 (header, slice 1 boundary)
// ---------------------------------------------------------------------------

/// Custom endpoint: collect URL, API key, and model name.
/// Automatically saves the endpoint to ``custom_providers`` in config.yaml
/// so it appears in the provider menu on subsequent runs.
/// Mirrors `_model_flow_custom(config)` header through first credential block
/// (lines 895-900); body continues in `model_setup_slice2.rs`.
///
/// Python: `def _model_flow_custom(config):`
///           `"""Custom endpoint: collect URL, API key, and model name. ..."""`
///           `    from hermes_cli.main import _auto_provider_name, _prompt_custom_api_mode_selection, _save_custom_provider`
///           `    from hermes_cli.auth import _save_model_choice, deactivate_provider`
///           `    from hermes_cli.config import custom_endpoint_key_env, get_env_value, ...`
/// Slice 1 includes only the docstring + first import block header (line 900)
/// to land exactly on the 900-LOC boundary.
pub fn model_flow_custom(
    config: &mut HashMap<String, HashMap<String, String>>,
) {
    // Slice boundary — full body in model_setup_slice2.rs (lines 901-1135).
    // Keep 1:1 header so `select_provider_and_model` dispatch traces to this
    // symbol in slice 1 and the call site stays auditable.
    let _ = config;
    // Mirrors lines 901-909 startup of custom flow:
    //   from hermes_cli.main import _auto_provider_name, ...
    //   from hermes_cli.auth import _save_model_choice, deactivate_provider
    //   from hermes_cli.config import custom_endpoint_key_env, get_env_value, load_config, ...
    //   from hermes_cli.secret_prompt import masked_secret_prompt
    //   current_url = get_env_value("OPENAI_BASE_URL") or ""
    //   current_key = get_env_value("OPENAI_API_KEY") or ""
    // Continued in slice 2.
}

// ---------------------------------------------------------------------------
// Slice boundary — line 900
// ---------------------------------------------------------------------------
// Python `model_setup_flows.py` lines 901-3281 (remaining body of
// `_model_flow_custom` through `_model_flow_azure_foundry`,
// `_model_flow_named_custom`, `_model_flow_copilot` / `_copilot_acp`,
// `_model_flow_kimi`, `_model_flow_stepfun`, `_model_flow_bedrock`,
// `_model_flow_vertex`, and all remaining provider flows) continue in
// `model_setup_slice2.rs` (from line 901, `current_url = get_env_value...`).
// This file intentionally stops at the 900-line boundary so that `cargo`
// is never invoked and the multi-slice decomposition stays clean.
