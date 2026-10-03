//! Pinned native routes. Port of the Hermes DirectSDK `model_catalog.py`
//! (NousResearch/hermes-plugin-claude-subscription-directsdk, MIT).
//!
//! A loopback gateway needs explicit long-context selection: native runs a
//! plain id inside its 200K default and selects 1M only for an explicit
//! `[1m]` route. Unpinned ids run at the 200K native default and are never
//! guessed up to 1M.

/// Canonical id → context window.
fn context_windows() -> &'static [(&'static str, u32)] {
    &[
        ("claude-sonnet-5", 1_000_000),
        ("claude-haiku-4-5-20251001", 200_000),
        ("claude-opus-5-5", 1_000_000),
        ("claude-opus-5", 1_000_000),
        ("claude-opus-4-8", 1_000_000),
        ("claude-fable-5-1", 1_000_000),
    ]
}

fn aliases() -> &'static [(&'static str, &'static str)] {
    &[
        ("sonnet", "claude-sonnet-5"),
        ("haiku", "claude-haiku-4-5-20251001"),
        ("claude-haiku-4-5", "claude-haiku-4-5-20251001"),
        ("opus", "claude-opus-5-5"),
        ("fable", "claude-fable-5-1"),
    ]
}

fn canonical(model: &str) -> &str {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    aliases()
        .iter()
        .find(|(a, _)| *a == base)
        .map(|(_, c)| *c)
        .unwrap_or(base)
}

/// Context window for a route: pinned table, else the 200K native default.
/// `[1m]`-suffixed ids report `None` (no guess exceeds the 1M native budget).
pub fn context_window(model: &str) -> Option<u32> {
    let canon = canonical(model);
    if let Some((_, w)) = context_windows().iter().find(|(id, _)| *id == canon) {
        return Some(*w);
    }
    if model.ends_with("[1m]") {
        return None;
    }
    Some(200_000)
}

/// Native `--model` selection: pinned 1M ids get `[1m]`, 200K ids go bare
/// (Haiku 4.5 has no 1M route), unpinned ids pass through untouched.
pub fn native_model(model: &str) -> Result<String, String> {
    let canon = canonical(model);
    match context_windows().iter().find(|(id, _)| *id == canon) {
        Some((_, 1_000_000)) => Ok(format!("{canon}[1m]")),
        Some((_, 200_000)) => {
            if model.ends_with("[1m]") {
                return Err("Haiku 4.5 does not support a 1M context window".to_string());
            }
            Ok(canon.to_string())
        }
        _ => Ok(model.to_string()),
    }
}

/// Every routable id: canonical ids, `[1m]` variants of 1M routes, aliases.
pub fn all_ids() -> Vec<String> {
    let mut out = Vec::new();
    for (id, w) in context_windows() {
        out.push(id.to_string());
        if *w == 1_000_000 {
            out.push(format!("{id}[1m]"));
        }
    }
    for (alias, _) in aliases() {
        out.push(alias.to_string());
    }
    out.sort();
    out.dedup();
    out
}

/// Display name for a route id.
pub fn display_name(id: &str) -> String {
    let base = id.strip_suffix("[1m]").unwrap_or(id);
    let canon = canonical(base);
    let short = canon
        .strip_prefix("claude-")
        .unwrap_or(canon)
        .replace('-', " ");
    if id.ends_with("[1m]") {
        format!("Claude {short} 1M")
    } else {
        format!("Claude {short}")
    }
}

#[path = "catalog_tests.rs"]
#[cfg(test)]
mod tests;
