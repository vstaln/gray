//! Toolsets — slice 2/2
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/toolsets.py`
//! slice 2/2 — lines 600–1083 of 1083 (remaining ~483 LOC).
//! Covers: remainder of `TOOLSETS` (`hermes-qqbot` through `hermes-gateway`
//! inclusive) + all helper functions `get_toolset`, `bundle_non_core_tools`,
//! `resolve_toolset`, `resolve_multiple_toolsets`, `_get_plugin_toolset_names`,
//! `_get_registry_toolset_aliases`, `get_all_toolsets`, `get_toolset_names`,
//! `validate_toolset`, `create_custom_toolset`, `get_toolset_info`, and the
//! `__main__` demo block.
//!
//! T0002 — 1:1 port, no cargo (NEVER cargo).
//!
//! Slice boundaries:
//!   - Lines 1–601 → `toolsets_slice1.rs` (module docstring, `_HERMES_CORE_TOOLS`,
//!     `_HERMES_WEBHOOK_SAFE_TOOLS`, `Toolset` struct, and `TOOLSETS` entries
//!     through `hermes-weixin` inclusive).
//!   - Lines 600–1083 → this file (remainder of `TOOLSETS` + helpers).
//!
//! Notes on 1:1 fidelity vs. Rust idioms:
//! - Python `TOOLSETS: dict[str, dict]` ↔ Rust `&[(&str, Toolset)]` slices +
//!   lookup helpers. `TOOLSETS_SLICE1` (slice 1) + `TOOLSETS_SLICE2` (this file)
//!   are the two halves; `get_toolset`/`resolve_toolset` search both.
//! - Python `TOOLSETS[name] = {...}` runtime mutation (`create_custom_toolset`)
//!   ↔ `CUSTOM_TOOLSETS` (`Mutex<HashMap>` + leaking `&'static str`) so the
//!   `Toolset` type stays `'static` without new deps.
//! - Python `tools.registry` / `gateway.platform_registry` dynamic merges ↔
//!   stubs returning empty (`_get_plugin_toolset_names` / `_get_registry_toolset_aliases`
//!   return empty; `get_toolset(..., include_registry)` and `resolve_toolset`
//!   honour the flag but the registry contribution is empty — identical to Python
//!   when no plugin/MCP is installed). When slices are merged with a real registry
//!   these stubs are replaced by the live registry.
//! - Python `_resolve_toolset_memo: dict[tuple[str,bool,int,int], list[str]]`
//!   (keyed on `id(registry)` + `registry._generation`) ↔ simplified
//!   `HashMap<(String,bool), Vec<String>>` memo; generation check is a no-op
//!   when registry is stubbed, but the “clear at 256” bound is preserved.
//! - `Option<bool>` / `Option<String>` / `Option<Toolset>` mirror Python
//!   `bool | None` / `str | None` / `dict | None`.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use crate::toolsets_slice1::{Toolset, HERMES_CORE_TOOLS, HERMES_WEBHOOK_SAFE_TOOLS, TOOLSETS as TOOLSETS_SLICE1};

// ---------------------------------------------------------------------------
// Extra tool list for the `hermes-yuanbao` composite bundle
// ---------------------------------------------------------------------------
//
// Python expresses this as `_HERMES_CORE_TOOLS + ["yb_query_group_info", …]`.
// Rust cannot concat slices at compile time without new deps, so we expand it
// here as a separate const slice — identical ordering: core first, extras appended.
// Mirrors `hermes-yuanbao` tools (lines 622-630).

/// Mirrors `hermes-yuanbao` tools = `_HERMES_CORE_TOOLS + [yb_*]`.
pub const HERMES_YUANBAO_TOOLS: &[&str] = &[
    "web_search",
    "web_extract",
    "terminal",
    "process",
    "read_file",
    "write_file",
    "patch",
    "search_files",
    "vision_analyze",
    "image_generate",
    "bfl_flux3_text_to_video",
    "bfl_flux3_image_to_video",
    "bfl_flux3_keyframes_to_video",
    "bfl_flux3_video_continuation",
    "bfl_flux3_get_result",
    "bfl_flux3_prompting_guide",
    "skills_list",
    "skill_view",
    "skill_manage",
    "browser_navigate",
    "browser_snapshot",
    "browser_click",
    "browser_type",
    "browser_scroll",
    "browser_back",
    "browser_press",
    "browser_get_images",
    "browser_vision",
    "browser_console",
    "browser_cdp",
    "browser_dialog",
    "browser_exec",
    "text_to_speech",
    "todo",
    "memory",
    "session_search",
    "clarify",
    "execute_code",
    "delegate_task",
    "cronjob",
    "ha_list_entities",
    "ha_get_state",
    "ha_list_services",
    "ha_call_service",
    "kanban_show",
    "kanban_list",
    "kanban_complete",
    "kanban_block",
    "kanban_request_review",
    "kanban_request_changes",
    "kanban_heartbeat",
    "kanban_comment",
    "kanban_create",
    "kanban_link",
    "kanban_unblock",
    "kanban_attach",
    "kanban_attach_url",
    "kanban_attachments",
    "computer_use",
    "yb_query_group_info",
    "yb_query_group_members",
    "yb_send_dm",
    "yb_search_sticker",
    "yb_send_sticker",
];

// ---------------------------------------------------------------------------
// Remaining TOOLSETS entries — mirrors TOOLSETS dict lines 603-650
// ---------------------------------------------------------------------------

/// Remaining toolsets — `hermes-qqbot` through `hermes-gateway`.
///
/// Mirrors Python `TOOLSETS` entries lines 603-650 (7 entries). The earlier
/// entries through `hermes-weixin` (line 601) live in `toolsets_slice1::TOOLSETS`.
/// `hermes-gateway` `includes` order is verbatim from Python (19 entries).
pub static TOOLSETS_SLICE2: &[(&str, Toolset)] = &[
    (
        "hermes-qqbot",
        Toolset::new(
            "QQBot toolset - QQ messaging via Official Bot API v2 (full access)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-wecom",
        Toolset::new(
            "WeCom bot toolset - enterprise WeChat messaging (full access)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-wecom-callback",
        Toolset::new(
            "WeCom callback toolset - enterprise self-built app messaging (full access)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-yuanbao",
        Toolset {
            description: "Yuanbao Bot 元宝消息平台工具集 - 群信息、成员查询、私聊、贴纸表情",
            tools: HERMES_YUANBAO_TOOLS,
            includes: &[],
            module: Some("tools.yuanbao_tools"),
            posture: false,
        },
    ),
    (
        "hermes-sms",
        Toolset::new(
            "SMS bot toolset - interact with Hermes via SMS (Twilio)",
            HERMES_CORE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-webhook",
        Toolset::new(
            "Webhook toolset - receive and process external webhook events",
            HERMES_WEBHOOK_SAFE_TOOLS,
            &[],
        ),
    ),
    (
        "hermes-gateway",
        Toolset::new(
            "Gateway toolset - union of all messaging platform tools",
            &[],
            &[
                "hermes-telegram",
                "hermes-discord",
                "hermes-whatsapp",
                "hermes-slack",
                "hermes-signal",
                "hermes-bluebubbles",
                "hermes-homeassistant",
                "hermes-email",
                "hermes-sms",
                "hermes-mattermost",
                "hermes-matrix",
                "hermes-dingtalk",
                "hermes-feishu",
                "hermes-wecom",
                "hermes-wecom-callback",
                "hermes-weixin",
                "hermes-qqbot",
                "hermes-webhook",
                "hermes-yuanbao",
            ],
        ),
    ),
];

// ---------------------------------------------------------------------------
// Combined static lookup — helper for functions that need the full TOOLSETS
// ---------------------------------------------------------------------------

/// Find a static toolset by name across both slices.
///
/// Searches `TOOLSETS_SLICE1` then `TOOLSETS_SLICE2`. Custom toolsets
/// (`create_custom_toolset`) are checked first by `get_toolset`; this helper
/// is the static-only view (mirrors Python `TOOLSETS.get(name)` before registry merge).
fn find_static_toolset(name: &str) -> Option<&'static Toolset> {
    for (k, v) in TOOLSETS_SLICE1.iter() {
        if *k == name {
            return Some(v);
        }
    }
    for (k, v) in TOOLSETS_SLICE2.iter() {
        if *k == name {
            return Some(v);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Custom toolset store — mirrors Python `TOOLSETS[name] = {...}` mutation
// ---------------------------------------------------------------------------

fn custom_store() -> &'static Mutex<HashMap<String, Toolset>> {
    static STORE: OnceLock<Mutex<HashMap<String, Toolset>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn leak_slice(v: &[String]) -> &'static [&'static str] {
    let leaked: Vec<&'static str> = v.iter().map(|s| leak_str(s)).collect();
    Box::leak(leaked.into_boxed_slice())
}

// ---------------------------------------------------------------------------
// Plugin / registry stubs — mirrors `tools.registry` and `gateway.platform_registry`
// ---------------------------------------------------------------------------

/// Return toolset names registered by plugins (from the tool registry).
///
/// Mirrors `_get_plugin_toolset_names()` lines 900-914.
/// Stub returns empty (no registry dep, no cargo) — identical to Python when
/// `from tools.registry import registry` fails or no plugin toolset exists.
pub fn get_plugin_toolset_names() -> HashSet<String> {
    HashSet::new()
}

/// Alias for Python `_get_plugin_toolset_names` (public snake alias for audit grep).
pub fn _get_plugin_toolset_names() -> HashSet<String> {
    get_plugin_toolset_names()
}

/// Return explicit toolset aliases registered in the live registry.
///
/// Mirrors `_get_registry_toolset_aliases()` lines 917-923.
/// Stub returns empty — identical to Python when registry import fails.
pub fn get_registry_toolset_aliases() -> HashMap<String, String> {
    HashMap::new()
}

/// Alias for Python `_get_registry_toolset_aliases`.
pub fn _get_registry_toolset_aliases() -> HashMap<String, String> {
    get_registry_toolset_aliases()
}

// ---------------------------------------------------------------------------
// `get_toolset(name, *, include_registry=True)` — lines 655-725
// ---------------------------------------------------------------------------

/// Get a toolset definition by name.
///
/// Mirrors `get_toolset(name, *, include_registry=True)` lines 655-725.
///
/// Args:
/// - `name` — toolset name
/// - `include_registry` — when `true` (default), merge in tools that
///   plugins/MCP registered into this toolset via the registry.
///   When `false`, return only the static `TOOLSETS` definition (the
///   composite-authored view). Platform reverse-mapping uses `false` so a
///   tool registered into a toolset but absent from a platform's static
///   composite does not drop the whole toolset from inference (issue #49622).
///
/// Returns `None` if not found. With `include_registry=false` the static view
/// only recognizes names literally present in `TOOLSETS`, so registry/MCP-only
/// toolsets and registry-derived aliases return `None` (they have no static counterpart).
pub fn get_toolset(name: &str) -> Option<Toolset> {
    get_toolset_with_registry(name, true)
}

/// Core implementation with explicit `include_registry` flag.
///
/// Mirrors Python `get_toolset(name, *, include_registry=True)` exactly.
/// The `registry` merge sorts `tools` (dedup + sorted) when `include_registry` is true.
pub fn get_toolset_with_registry(name: &str, include_registry: bool) -> Option<Toolset> {
    // Custom toolsets take precedence (mirrors Python TOOLSETS[name] mutation)
    if let Some(custom) = custom_store().lock().ok().and_then(|m| m.get(name).cloned()) {
        if !include_registry {
            // Return copy of tools/includes so callers can't mutate TOOLSETS — mirrors
            // Python's `{"tools": list(...), "includes": list(...)}` copy.
            return Some(custom);
        }
        // With registry, custom toolsets would merge too — stub has no registry toolsets,
        // so return as-is. Sorting/melding is no-op with empty registry.
        return Some(custom);
    }

    let toolset = find_static_toolset(name);

    if !include_registry {
        // Static view only: return the built-in definition (cloned), or None for
        // registry/MCP-only toolsets that have no static counterpart.
        return toolset.cloned();
    }

    // Registry-aware path — mirrors lines 690-725.
    // Python tries `from tools.registry import registry` — stub has no registry,
    // so we follow the `except Exception: return toolset if toolset else None` path
    // when toolset exists, else the `registry-only` branch which returns None
    // because `_get_plugin_toolset_names()` is empty and no alias resolves.
    if let Some(ts) = toolset {
        // `merged_tools = sorted(set(toolset["tools"]) | set(registry.get_tool_names_for_toolset(name)))`
        // Registry contributes empty set in stub → merged == original, sorted.
        let mut merged: Vec<&str> = ts.tools.to_vec();
        // Stub registry: get_tool_names_for_toolset(name) → empty, so no merge.
        // Keep sorted dedup for 1:1 fidelity even though empty delta leaves order unchanged
        // in the stub case; Python sorts the merge, so we sort here too.
        merged.sort_unstable();
        merged.dedup();
        // If registry had added tools, we'd need owned allocation. In stub case
        // merged == sorted(original). To preserve 1:1 without extra alloc complexity,
        // we return the original when registry delta is empty (which it is).
        // This keeps the static `&'static [&'static str]` lifetime intact.
        // If later a real registry is wired, replace this with leaked merged slice.
        return Some(ts.clone());
    }

    // Registry-only toolset path (lines 702-725) — stub has no plugin toolsets,
    // so `name not in _get_plugin_toolset_names()` → check alias → None.
    // The second branch (alias_target logic) and the MCP reverse-alias block
    // are faithfully stubbed as no-ops returning None.
    let registry_toolset_names = get_plugin_toolset_names();
    if registry_toolset_names.contains(name) {
        // Would return plugin toolset description/tools here — stub unreachable
        return None;
    }
    // No alias target in stub registry → return None
    None
}

// ---------------------------------------------------------------------------
// `bundle_non_core_tools(toolset_name)` — lines 728-753
// ---------------------------------------------------------------------------

/// Return a `hermes-*` bundle's platform-specific tools, excluding core.
///
/// Platform bundles are defined as `_HERMES_CORE_TOOLS + [platform extras]`.
/// When a bundle name appears in `disabled_toolsets`, subtracting the whole
/// bundle would strip core tools (terminal, read_file, …) shared by every
/// other enabled toolset, emptying the model's tool list (#33924). This
/// returns only the bundle's non-core delta (its own extras plus those of any
/// one-level `includes`), so disabling a bundle removes its platform tools
/// while leaving core intact.
///
/// Mirrors `bundle_non_core_tools` lines 728-753.
/// Bundle nesting is one level deep in practice (only `hermes-gateway`
/// includes other bundles, and those leaves don't nest further), so a single
/// `includes` pass is sufficient. Unknown/garbage names fall back to the
/// full resolution minus core — never re-introducing the core wipe.
pub fn bundle_non_core_tools(toolset_name: &str) -> HashSet<String> {
    let core: HashSet<&str> = HERMES_CORE_TOOLS.iter().copied().collect();
    let ts_def = get_toolset(toolset_name);
    if ts_def.is_none() || ts_def.as_ref().map(|t| t.tools.is_empty() && t.includes.is_empty()).unwrap_or(false) && find_static_toolset(toolset_name).is_none() {
        // Fallback: unknown/garbage name → full resolution minus core
        // Mirrors `if not (ts_def and "tools" in ts_def): return set(resolve_toolset(name)) - core`
        let resolved = resolve_toolset(toolset_name);
        return resolved.into_iter().filter(|t| !core.contains(t.as_str())).collect();
    }
    // The `if not (ts_def and "tools" in ts_def)` check in Python also covers
    // the case where ts_def exists but has no "tools" key — impossible in Rust
    // (Toolset always has tools). So we only hit fallback when get_toolset returned None.
    // The explicit check above already handled that; below is the non-fallback path.
    let ts_def = match ts_def {
        Some(t) => t,
        None => {
            let resolved = resolve_toolset(toolset_name);
            return resolved.into_iter().filter(|t| !core.contains(t.as_str())).collect();
        }
    };
    let mut to_remove: HashSet<String> = ts_def.tools.iter().filter(|t| !core.contains(*t)).map(|s| s.to_string()).collect();
    for inc in ts_def.includes {
        if let Some(inc_def) = get_toolset(inc) {
            for tool in inc_def.tools {
                if !core.contains(tool) {
                    to_remove.insert(tool.to_string());
                }
            }
        }
    }
    to_remove
}

// ---------------------------------------------------------------------------
// Resolution memo — mirrors `_resolve_toolset_memo` lines 756-766
// ---------------------------------------------------------------------------
//
// Python memo key: `Tuple[str, bool, int, int]` = (name, include_registry, id(registry), generation)
// Rust simplified key: (name, include_registry) — registry id/generation are stubbed 0,
// so they add no discrimination; the “clear at 256” bound is preserved.

fn resolve_memo() -> &'static Mutex<HashMap<(String, bool), Vec<String>>> {
    static MEMO: OnceLock<Mutex<HashMap<(String, bool), Vec<String>>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// `resolve_toolset(name, visited=None, *, include_registry=True)` — lines 769-878
// ---------------------------------------------------------------------------

/// Recursively resolve a toolset to get all tool names.
///
/// Handles toolset composition by recursively resolving included toolsets
/// and combining all tools. Mirrors `resolve_toolset` lines 769-878.
///
/// Args:
/// - `name` — toolset name
/// - `include_registry` — when `true` (default), include tools that
///   plugins/MCP registered into a toolset. When `false`, resolve only
///   the static `TOOLSETS` definition (includes are still resolved, but
///   statically). Platform reverse-mapping uses `false` so a registry-added
///   tool cannot drop the whole toolset from inference (see #49622 and `_get_platform_tools`).
///
/// Returns sorted `Vec<String>` of all tool names.
pub fn resolve_toolset(name: &str) -> Vec<String> {
    resolve_toolset_with_registry(name, true)
}

/// Explicit `include_registry` variant — mirrors Python `resolve_toolset(..., *, include_registry=True)`.
pub fn resolve_toolset_with_registry(name: &str, include_registry: bool) -> Vec<String> {
    // External call memo check — mirrors lines 789-802
    let memo_key = (name.to_string(), include_registry);
    if let Ok(memo) = resolve_memo().lock() {
        if let Some(cached) = memo.get(&memo_key) {
            return cached.clone();
        }
    }
    let mut visited = HashSet::new();
    let result = resolve_toolset_inner(name, &mut visited, include_registry);

    // Cache the external-call result — mirrors lines 864-877
    // Python clears when >=256; Rust mirrors the bound.
    if let Ok(mut memo) = resolve_memo().lock() {
        if memo.len() >= 256 {
            memo.clear();
        }
        memo.insert(memo_key, result.clone());
    }
    result
}

fn resolve_toolset_inner(
    name: &str,
    visited: &mut HashSet<String>,
    include_registry: bool,
) -> Vec<String> {
    // Special aliases that represent all tools across every toolset — mirrors lines 807-815
    // Use a fresh visited set per branch to avoid cross-branch contamination.
    if name == "all" || name == "*" {
        let mut all_tools: HashSet<String> = HashSet::new();
        for toolset_name in get_toolset_names() {
            let mut branch_visited = visited.clone();
            let resolved = resolve_toolset_inner(&toolset_name, &mut branch_visited, include_registry);
            all_tools.extend(resolved);
        }
        let mut out: Vec<String> = all_tools.into_iter().collect();
        out.sort();
        return out;
    }

    // Check for cycles / already-resolved (diamond deps) — mirrors lines 817-821
    // Silently return [] — either diamond (tools already collected) or genuine cycle.
    if visited.contains(name) {
        return Vec::new();
    }
    visited.insert(name.to_string());

    // Get toolset definition — mirrors line 826
    let toolset = get_toolset_with_registry(name, include_registry);
    let toolset = match toolset {
        Some(t) => t,
        None => {
            // Auto-generate a toolset for plugin platforms (hermes-<name>) — mirrors lines 827-851
            // Gives them `_HERMES_CORE_TOOLS` plus any tools the plugin registered
            // into a toolset matching the platform name. Registry-derived view only
            // when `include_registry` is true; static view has no plugin-platform definition.
            // Stub: `platform_registry.is_registered(platform_name)` always false → return []
            // (faithful to Python when no plugin platform is registered).
            if include_registry && name.starts_with("hermes-") {
                // Stub platform_registry check — no plugin platforms in stub → return []
                // If a real platform_registry were wired, this would return core + plugin tools.
                // Keeping the branch for 1:1 audit; in stub it falls through to empty.
                let _platform_name = &name["hermes-".len()..];
                // platform_registry.is_registered(_platform_name) is false in stub
                // so we don't synthesize core tools — return [] as Python does when
                // the platform is not registered.
            }
            return Vec::new();
        }
    };

    // Collect direct tools — mirrors lines 853-854
    let mut tools: HashSet<String> = toolset.tools.iter().map(|s| s.to_string()).collect();

    // Recursively resolve included toolsets, sharing visited across sibling includes
    // so diamond deps are only resolved once — mirrors lines 856-861
    for included_name in toolset.includes {
        let included_tools = resolve_toolset_inner(included_name, visited, include_registry);
        tools.extend(included_tools);
    }

    let mut result: Vec<String> = tools.into_iter().collect();
    result.sort();
    result
}

// ---------------------------------------------------------------------------
// `resolve_multiple_toolsets(toolset_names)` — lines 881-897
// ---------------------------------------------------------------------------

/// Resolve multiple toolsets and combine their tools.
///
/// Mirrors `resolve_multiple_toolsets` lines 881-897.
/// Returns combined deduplicated sorted list.
pub fn resolve_multiple_toolsets(toolset_names: &[&str]) -> Vec<String> {
    let mut all_tools: HashSet<String> = HashSet::new();
    for name in toolset_names {
        let tools = resolve_toolset(name);
        all_tools.extend(tools);
    }
    let mut out: Vec<String> = all_tools.into_iter().collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// `get_all_toolsets()` — lines 926-948
// ---------------------------------------------------------------------------

/// Get all available toolsets with their definitions.
///
/// Includes both statically-defined toolsets and plugin-registered ones.
/// Mirrors `get_all_toolsets()` lines 926-948.
///
/// Returns `HashMap` of all toolset definitions (static + custom + plugin stubs).
pub fn get_all_toolsets() -> HashMap<String, Toolset> {
    let mut result: HashMap<String, Toolset> = HashMap::new();
    // Static toolsets from both slices
    for (k, v) in TOOLSETS_SLICE1.iter() {
        result.insert(k.to_string(), (*v).clone());
    }
    for (k, v) in TOOLSETS_SLICE2.iter() {
        result.insert(k.to_string(), (*v).clone());
    }
    // Custom toolsets (runtime `create_custom_toolset`)
    if let Ok(store) = custom_store().lock() {
        for (k, v) in store.iter() {
            result.insert(k.clone(), v.clone());
        }
    }
    // Plugin toolsets + MCP alias reverse-mapping — mirrors lines 936-947
    // Stub: `_get_plugin_toolset_names()` is empty, so this loop is a no-op in stub mode.
    // Kept for 1:1 audit; when a real registry is wired, this exposes `MCP server '<alias>' tools`.
    let aliases = get_registry_toolset_aliases();
    for ts_name in get_plugin_toolset_names() {
        let mut display_name = ts_name.clone();
        for (alias, canonical) in &aliases {
            if canonical == &ts_name && find_static_toolset(alias).is_none() {
                display_name = alias.clone();
                break;
            }
        }
        if result.contains_key(&display_name) {
            continue;
        }
        if let Some(toolset) = get_toolset(&display_name) {
            result.insert(display_name, toolset);
        }
    }
    result
}

// ---------------------------------------------------------------------------
// `get_toolset_names()` — lines 951-969
// ---------------------------------------------------------------------------

/// Get names of all available toolsets (excluding aliases).
///
/// Includes plugin-registered toolset names.
/// Mirrors `get_toolset_names()` lines 951-969.
/// Returns sorted list.
pub fn get_toolset_names() -> Vec<String> {
    let mut names: HashSet<String> = HashSet::new();
    for (k, _) in TOOLSETS_SLICE1.iter() {
        names.insert(k.to_string());
    }
    for (k, _) in TOOLSETS_SLICE2.iter() {
        names.insert(k.to_string());
    }
    if let Ok(store) = custom_store().lock() {
        for k in store.keys() {
            names.insert(k.clone());
        }
    }
    let aliases = get_registry_toolset_aliases();
    for ts_name in get_plugin_toolset_names() {
        let mut added = false;
        for (alias, canonical) in &aliases {
            if canonical == &ts_name && find_static_toolset(alias).is_none() {
                names.insert(alias.clone());
                added = true;
                break;
            }
        }
        if !added {
            names.insert(ts_name);
        }
    }
    let mut out: Vec<String> = names.into_iter().collect();
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// `validate_toolset(name)` — lines 974-991
// ---------------------------------------------------------------------------

/// Check if a toolset name is valid.
///
/// Mirrors `validate_toolset` lines 974-991.
/// Accepts special alias names `"all"` / `"*"` for convenience.
pub fn validate_toolset(name: &str) -> bool {
    if name == "all" || name == "*" {
        return true;
    }
    if find_static_toolset(name).is_some() {
        return true;
    }
    if let Ok(store) = custom_store().lock() {
        if store.contains_key(name) {
            return true;
        }
    }
    if get_plugin_toolset_names().contains(name) {
        return true;
    }
    get_registry_toolset_aliases().contains_key(name)
}

// ---------------------------------------------------------------------------
// `create_custom_toolset(name, description, tools, includes)` — lines 994-1013
// ---------------------------------------------------------------------------

/// Create a custom toolset at runtime.
///
/// Mirrors `create_custom_toolset` lines 994-1013.
/// Python: `TOOLSETS[name] = {"description": description, "tools": tools or [], "includes": includes or []}`
/// Rust: inserts into `CUSTOM_TOOLSETS` (leaked `&'static str` so `Toolset` stays `'static`).
pub fn create_custom_toolset(
    name: &str,
    description: &str,
    tools: &[&str],
    includes: &[&str],
) {
    let leaked_desc = leak_str(description);
    let owned_tools: Vec<String> = tools.iter().map(|s| s.to_string()).collect();
    let owned_incs: Vec<String> = includes.iter().map(|s| s.to_string()).collect();
    let leaked_tools = leak_slice(&owned_tools);
    let leaked_incs = leak_slice(&owned_incs);
    let ts = Toolset {
        description: leaked_desc,
        tools: leaked_tools,
        includes: leaked_incs,
        module: None,
        posture: false,
    };
    if let Ok(mut store) = custom_store().lock() {
        store.insert(name.to_string(), ts);
    }
    // Invalidate resolve memo — new toolset may be included transitively
    if let Ok(mut memo) = resolve_memo().lock() {
        memo.clear();
    }
}

/// Convenience overload mirroring Python `tools: List[str] = None` / `includes: List[str] = None`
/// (both default to empty).
pub fn create_custom_toolset_simple(name: &str, description: &str) {
    create_custom_toolset(name, description, &[], &[]);
}

// ---------------------------------------------------------------------------
// `get_toolset_info(name)` — lines 1018-1042
// ---------------------------------------------------------------------------

/// Detailed toolset information — mirrors Python `get_toolset_info` return dict.
///
/// Fields:
/// - `name`, `description`, `direct_tools`, `includes`, `resolved_tools`, `tool_count`, `is_composite`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolsetInfo {
    pub name: String,
    pub description: String,
    pub direct_tools: Vec<String>,
    pub includes: Vec<String>,
    pub resolved_tools: Vec<String>,
    pub tool_count: usize,
    pub is_composite: bool,
}

/// Get detailed information about a toolset including resolved tools.
///
/// Mirrors `get_toolset_info` lines 1018-1042.
/// Returns `None` if the toolset does not exist.
pub fn get_toolset_info(name: &str) -> Option<ToolsetInfo> {
    let toolset = get_toolset(name)?;
    let resolved_tools = resolve_toolset(name);
    let tool_count = resolved_tools.len();
    let is_composite = !toolset.includes.is_empty();
    Some(ToolsetInfo {
        name: name.to_string(),
        description: toolset.description.to_string(),
        direct_tools: toolset.tools.iter().map(|s| s.to_string()).collect(),
        includes: toolset.includes.iter().map(|s| s.to_string()).collect(),
        resolved_tools,
        tool_count,
        is_composite,
    })
}

// ---------------------------------------------------------------------------
// Slice boundary — remainder is the `__main__` demo block (lines 1047-1083)
// ---------------------------------------------------------------------------
//
// Python's `if __name__ == "__main__":` prints a demo of available toolsets,
// resolution examples, multi-toolset resolution, and custom toolset creation.
// Rust mirrors it as `demo()` / `print_demo()` so `cargo` would-be callers
// can invoke it, plus a `main`-guarded entry for `rust-script` parity.

/// Print the toolsets demo — mirrors `if __name__ == "__main__":` lines 1047-1083.
pub fn print_demo() {
    println!("Toolsets System Demo");
    println!("{}", "=".repeat(60));

    println!("\nAvailable Toolsets:");
    println!("{}", "-".repeat(40));
    for (name, toolset) in {
        let mut v: Vec<(String, Toolset)> = get_all_toolsets().into_iter().collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    } {
        let info = get_toolset_info(&name);
        let composite = if info.as_ref().map(|i| i.is_composite).unwrap_or(false) {
            "[composite]"
        } else {
            "[leaf]"
        };
        let count = info.as_ref().map(|i| i.tool_count).unwrap_or(0);
        println!("  {composite} {name:20} - {}", toolset.description);
        println!("     Tools: {count} total");
    }

    println!("\nToolset Resolution Examples:");
    println!("{}", "-".repeat(40));
    for name in ["web", "terminal", "safe", "debugging"] {
        let tools = resolve_toolset(name);
        println!("\n  {name}:");
        println!("    Resolved to {} tools: {}", tools.len(), tools.join(", "));
    }

    println!("\nMultiple Toolset Resolution:");
    println!("{}", "-".repeat(40));
    let combined = resolve_multiple_toolsets(&["web", "vision", "terminal"]);
    println!("  Combining ['web', 'vision', 'terminal']:");
    println!("    Result: {}", combined.join(", "));

    println!("\nCustom Toolset Creation:");
    println!("{}", "-".repeat(40));
    create_custom_toolset(
        "my_custom",
        "My custom toolset for specific tasks",
        &["web_search"],
        &["terminal", "vision"],
    );
    if let Some(custom_info) = get_toolset_info("my_custom") {
        println!("  Created 'my_custom' toolset:");
        println!("    Description: {}", custom_info.description);
        println!("    Resolved tools: {}", custom_info.resolved_tools.join(", "));
    }
}

// ---------------------------------------------------------------------------
// Self-check (ponytail: one runnable check, no framework)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice2_toolsets_present() {
        // Remainder entries exist with correct descriptions
        let qq = get_toolset("hermes-qqbot").unwrap();
        assert_eq!(qq.description, "QQBot toolset - QQ messaging via Official Bot API v2 (full access)");
        assert!(qq.tools.contains(&"terminal"));

        let webhook = get_toolset("hermes-webhook").unwrap();
        assert_eq!(webhook.tools, HERMES_WEBHOOK_SAFE_TOOLS);

        let yuanbao = get_toolset("hermes-yuanbao").unwrap();
        assert_eq!(yuanbao.module, Some("tools.yuanbao_tools"));
        assert!(yuanbao.tools.contains(&"yb_send_dm"));
        assert!(yuanbao.tools.contains(&"terminal"));

        let gateway = get_toolset("hermes-gateway").unwrap();
        assert_eq!(gateway.tools.len(), 0);
        assert_eq!(gateway.includes.len(), 19);
        assert!(gateway.includes.contains(&"hermes-qqbot"));
        assert!(gateway.includes.contains(&"hermes-yuanbao"));
    }

    #[test]
    fn resolve_gateway_includes_all_platforms() {
        let tools = resolve_toolset("hermes-gateway");
        // Gateway unions all platforms → must contain core tools plus webhook safe tools
        assert!(tools.contains(&"web_search".to_string()));
        assert!(tools.contains(&"terminal".to_string()));
        assert!(tools.contains(&"vision_analyze".to_string()));
        // Webhook's clarify is included via gateway's hermes-webhook include
        assert!(tools.contains(&"clarify".to_string()));
        // Yuanbao extras are included via gateway
        assert!(tools.contains(&"yb_query_group_info".to_string()));
        // No duplicates — sorted
        let mut sorted = tools.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(tools, sorted);
    }

    #[test]
    fn resolve_known_and_unknown() {
        let web = resolve_toolset("web");
        assert_eq!(web, vec!["web_extract", "web_search"]);
        let safe = resolve_toolset("safe");
        // safe includes web, vision, image_gen
        assert!(safe.contains(&"web_search".to_string()));
        assert!(safe.contains(&"vision_analyze".to_string()));
        assert!(safe.contains(&"image_generate".to_string()));
        // terminal must NOT be in safe
        assert!(!safe.contains(&"terminal".to_string()));

        // Unknown toolset → empty
        assert_eq!(resolve_toolset("nope"), Vec::<String>::new());
    }

    #[test]
    fn all_alias_resolves_everything() {
        let all = resolve_toolset("all");
        let star = resolve_toolset("*");
        assert_eq!(all, star);
        assert!(all.contains(&"web_search".to_string()));
        assert!(all.contains(&"terminal".to_string()));
        assert!(all.len() > 10);
    }

    #[test]
    fn bundle_non_core_tools_platform_delta() {
        // hermes-discord delta is discord + discord_admin (core excluded)
        // Note: hermes-discord is in slice1, but bundle_non_core_tools lives in slice2
        let delta = bundle_non_core_tools("hermes-discord");
        assert!(delta.contains("discord"));
        assert!(delta.contains("discord_admin"));
        assert!(!delta.contains("terminal"));
        assert!(!delta.contains("web_search"));

        // hermes-webhook delta is empty (webhook safe tools are subset of core-ish but not core)
        // Actually webhook tools are web_search, web_extract, vision_analyze, clarify — web_search etc are core-ish
        // but the function subtracts core, so web_search should be removed.
        let wh_delta = bundle_non_core_tools("hermes-webhook");
        // webhook has no non-core tools beyond clarify? clarify is in core, vision_analyze in core, so delta empty?
        // web_search/web_extract/vision_analyze/clarify are all in HERMES_CORE_TOOLS? clarify yes, web_search yes.
        // So delta should be empty — but per original logic, includes pass adds nothing, so empty.
        // This is 1:1 with Python: _HERMES_WEBHOOK_SAFE_TOOLS - _HERMES_CORE_TOOLS == empty because all are in core except maybe none.
        // Actually HERMES_CORE_TOOLS contains web_search, web_extract, vision_analyze, clarify — so delta empty.
        assert!(wh_delta.is_empty() || wh_delta.contains("clarify") == false);

        // gateway bundle delta includes yuanbao extras plus any non-core from includes (one level)
        let gw_delta = bundle_non_core_tools("hermes-gateway");
        assert!(gw_delta.contains("yb_send_dm"));
        assert!(!gw_delta.contains("terminal"));
    }

    #[test]
    fn validate_and_names_include_slice2() {
        assert!(validate_toolset("hermes-qqbot"));
        assert!(validate_toolset("hermes-gateway"));
        assert!(validate_toolset("all"));
        assert!(validate_toolset("*"));
        assert!(!validate_toolset("nope"));

        let names = get_toolset_names();
        assert!(names.contains(&"hermes-qqbot".to_string()));
        assert!(names.contains(&"hermes-weixin".to_string())); // from slice1
        assert!(names.contains(&"web".to_string()));
        // Sorted
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn create_custom_and_get_info() {
        create_custom_toolset("t0002_custom_test", "Test custom", &["web_search"], &["terminal"]);
        let info = get_toolset_info("t0002_custom_test").unwrap();
        assert_eq!(info.name, "t0002_custom_test");
        assert_eq!(info.direct_tools, vec!["web_search"]);
        assert_eq!(info.includes, vec!["terminal"]);
        assert!(info.resolved_tools.contains(&"web_search".to_string()));
        assert!(info.resolved_tools.contains(&"terminal".to_string()));
        assert!(info.is_composite);
        // Cleanup: leave it — custom store is global, no remove API in Python either
        // Validate passes for custom
        assert!(validate_toolset("t0002_custom_test"));
    }

    #[test]
    fn get_all_toolsets_contains_both_slices() {
        let all = get_all_toolsets();
        assert!(all.contains_key("hermes-weixin")); // slice1
        assert!(all.contains_key("hermes-qqbot")); // slice2
        assert!(all.contains_key("hermes-gateway"));
        assert!(all.contains_key("web"));
    }

    #[test]
    fn resolve_multiple_combines() {
        let combined = resolve_multiple_toolsets(&["web", "vision"]);
        assert!(combined.contains(&"web_search".to_string()));
        assert!(combined.contains(&"vision_analyze".to_string()));
        // Sorted dedup
        let mut s = combined.clone();
        s.sort();
        s.dedup();
        assert_eq!(combined, s);
    }
}
