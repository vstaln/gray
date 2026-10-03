//! Setup-time probes of the user's Claude CLI: binary presence and login
//! state. Port of the Hermes DirectSDK `directsdk_setup.py` probes
//! (NousResearch/hermes-plugin-claude-subscription-directsdk, MIT).
//!
//! Both probes are offline with respect to Anthropic: `auth status` reads
//! the local credential store. Anything unexpected degrades to the pinned
//! catalog rather than failing setup.

use std::path::PathBuf;

/// `claude` is missing (or not on PATH): install hint, never a spawn panic.
pub const INSTALL_HINT: &str = "`claude` not found on PATH. Install it with \
    `npm install -g @anthropic-ai/claude-code`, then `claude auth login`. \
    Override the binary with CLAUDE_SUB_COMMAND=/path/to/claude.";
/// `claude` is present but has no usable login here.
pub const LOGIN_HINT: &str =
    "Claude Code is installed but not logged in. Run `claude auth login` and retry.";

fn env_override() -> Option<String> {
    std::env::var("CLAUDE_SUB_COMMAND")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            std::env::var("CLAUDE_SUBSCRIPTION_DIRECTSDK_COMMAND")
                .ok()
                .filter(|v| !v.is_empty())
        })
}

/// Resolve the `claude` binary: explicit override, then PATH.
pub fn resolve_command() -> Option<String> {
    if let Some(v) = env_override() {
        return Some(v);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in ["claude", "claude.exe", "claude.cmd"] {
            let p: PathBuf = dir.join(name);
            if p.is_file() {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Fail-closed auth env: a set value would forward the subscription bearer
/// somewhere else, or confuse native backend routing.
pub fn conflicting_env() -> Option<String> {
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_FOUNDRY_API_KEY",
    ] {
        if std::env::var_os(key).is_some() {
            return Some(key.to_string());
        }
    }
    for key in [
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ] {
        if let Some(v) = std::env::var_os(key)
            && !matches!(
                v.to_string_lossy().to_lowercase().as_str(),
                "" | "0" | "false" | "no" | "off"
            )
        {
            return Some(key.to_string());
        }
    }
    None
}

/// Child env for every spawn: never inherit a conflicting value, always
/// quiet the CLI's nonessential traffic.
pub fn child_env() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| {
            ![
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_FOUNDRY_API_KEY",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
            ]
            .contains(&k.as_str())
        })
        .collect();
    for (k, v) in [
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
        ("DISABLE_TELEMETRY", "1"),
        ("DISABLE_ERROR_REPORTING", "1"),
    ] {
        if !out.iter().any(|(k2, _)| k2 == k) {
            out.push((k.to_string(), v.to_string()));
        }
    }
    out
}

#[path = "setup_tests.rs"]
#[cfg(test)]
mod tests;
