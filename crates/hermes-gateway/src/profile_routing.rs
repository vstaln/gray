//! Profile-based routing for the gateway with hierarchical matching.
//!
//! Allows a single Hermes instance to route specific Discord guilds/channels/threads
//! to different profiles — each with their own model, tools, memory, and persona.
//!
//! Matching priority (most specific first):
//!   1. platform + chat_id + thread_id (exact thread)  — specificity 14
//!   2. platform + chat_id (channel route)             — specificity 6
//!   3. platform + guild_id (guild/server route)       — specificity 2
//!   4. No match                                       → default profile
//!
//! Parent-chain matching:
//! For Discord threads and forum posts, `parent_chat_id` carries the
//! direct parent (the channel for a thread, the forum channel for a post).
//! Routes keyed on a channel match both direct messages and messages in
//! any thread/post whose parent is that channel.
//!
//! Configuration (config.yaml):
//! ```yaml
//! gateway:
//!   profile_routes:
//!     - name: server-default
//!       platform: discord
//!       guild_id: "YOUR_GUILD_ID"
//!       profile: server-profile
//!
//!     - name: special-channel
//!       platform: discord
//!       guild_id: "YOUR_GUILD_ID"
//!       chat_id: "YOUR_CHANNEL_ID"
//!       profile: channel-profile
//!
//!     - name: thread-route
//!       platform: discord
//!       chat_id: "YOUR_CHANNEL_ID"
//!       thread_id: "YOUR_THREAD_ID"
//!       profile: thread-profile
//! ```

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An explicit route matched a profile this gateway does not serve.
#[derive(Debug, thiserror::Error)]
#[error("profile route rejected: {0}")]
pub struct ProfileRouteRejected(pub String);

impl ProfileRouteRejected {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

// ---------------------------------------------------------------------------
// ProfileRoute
// ---------------------------------------------------------------------------

/// A single routing rule that maps a platform scope to a profile.
///
/// Mirrors `gateway/profile_routing.py::ProfileRoute`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRoute {
    pub name: String,
    pub platform: String,
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guild_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl ProfileRoute {
    pub fn new(
        name: impl Into<String>,
        platform: impl Into<String>,
        profile: impl Into<String>,
        guild_id: Option<String>,
        chat_id: Option<String>,
        thread_id: Option<String>,
        enabled: bool,
    ) -> Self {
        Self {
            name: name.into(),
            platform: platform.into(),
            profile: profile.into(),
            guild_id: guild_id.filter(|s| !s.is_empty()),
            chat_id: chat_id.filter(|s| !s.is_empty()),
            thread_id: thread_id.filter(|s| !s.is_empty()),
            enabled,
        }
    }

    /// Higher value = more specific match.
    /// guild_id +2, chat_id +4, thread_id +8 — mirrors Python `specificity`.
    pub fn specificity(&self) -> u8 {
        let mut s: u8 = 0;
        if self.guild_id.as_deref().is_some_and(|v| !v.is_empty()) {
            s += 2;
        }
        if self.chat_id.as_deref().is_some_and(|v| !v.is_empty()) {
            s += 4;
        }
        if self.thread_id.as_deref().is_some_and(|v| !v.is_empty()) {
            s += 8;
        }
        s
    }

    /// Return true if this route matches the given source fields.
    ///
    /// All configured discriminators are matched conjunctively (AND): every
    /// discriminator that the route declares must hold. `chat_id` supports
    /// hierarchical matching for Discord forums/threads:
    /// - Direct channel match: chat_id == route.chat_id
    /// - Thread in channel: parent_chat_id == route.chat_id
    /// A route declaring both `guild_id` and `chat_id` requires both to
    /// match (a chat match alone does not satisfy a guild constraint).
    pub fn matches(
        &self,
        platform: &str,
        guild_id: Option<&str>,
        chat_id: Option<&str>,
        thread_id: Option<&str>,
        parent_chat_id: Option<&str>,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        if self.platform != platform {
            return false;
        }
        if let Some(tid) = self.thread_id.as_deref() {
            if !tid.is_empty() && Some(tid) != thread_id {
                return false;
            }
        }
        if let Some(cid) = self.chat_id.as_deref() {
            if !cid.is_empty() && Some(cid) != chat_id && Some(cid) != parent_chat_id {
                return false;
            }
        }
        if let Some(gid) = self.guild_id.as_deref() {
            if !gid.is_empty() && Some(gid) != guild_id {
                return false;
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Validation helpers — mirrors hermes_cli/profiles.py
// ---------------------------------------------------------------------------

const RESERVED_NAMES: &[&str] = &["hermes", "default", "test", "tmp", "root", "sudo"];

fn normalize_profile_name(name: &str) -> Result<String, String> {
    let stripped = name.trim();
    if stripped.is_empty() {
        return Err("profile name cannot be empty".to_string());
    }
    if stripped.eq_ignore_ascii_case("default") {
        return Ok("default".to_string());
    }
    Ok(stripped.to_lowercase())
}

fn is_valid_profile_id(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    let b = name.as_bytes();
    // first char: [a-z0-9]
    let first = b[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    b.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

fn validate_profile_name(name: &str) -> Result<(), String> {
    if name == "default" {
        return Ok(());
    }
    if !is_valid_profile_id(name) {
        return Err(format!(
            "Invalid profile name {name:?}. Must match [a-z0-9][a-z0-9_-]{{0,63}}"
        ));
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(format!(
            "Profile name {name:?} is reserved — it collides with either the Hermes installation itself or a common system binary.  Pick a different name."
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Parse / match — mirrors Python free functions
// ---------------------------------------------------------------------------

/// Parse `profile_routes` from config.yaml into `ProfileRoute` objects.
///
/// Returns routes sorted by specificity (most specific first).
/// `raw` is `None` or an empty slice → empty vec (mirrors `if not raw: return []`).
pub fn parse_profile_routes(raw: Option<&[serde_json::Value]>) -> Vec<ProfileRoute> {
    let Some(entries) = raw else {
        return Vec::new();
    };
    if entries.is_empty() {
        return Vec::new();
    }
    let mut routes: Vec<ProfileRoute> = Vec::new();
    for entry in entries {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let platform = obj
            .get("platform")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let mut profile = obj
            .get("profile")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if platform.is_empty() || profile.is_empty() {
            log::warn!("Skipping profile route {name}: missing platform or profile");
            continue;
        }

        // Validate profile name to prevent path traversal — mirrors lazy import
        // of hermes_cli.profiles.normalize/validate.
        match normalize_profile_name(&profile).and_then(|n| {
            validate_profile_name(&n).map(|_| n).map_err(|e| e)
        }) {
            Ok(normalized) => profile = normalized,
            Err(_) => {
                log::warn!("Skipping profile route {name}: invalid profile name {profile:?}");
                continue;
            }
        }

        let guild_id = obj
            .get("guild_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let chat_id = obj
            .get("chat_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let thread_id = obj
            .get("thread_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let enabled = obj
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        routes.push(ProfileRoute::new(
            name, platform, profile, guild_id, chat_id, thread_id, enabled,
        ));
    }
    // Sort: most specific first so the first match wins.
    routes.sort_by(|a, b| b.specificity().cmp(&a.specificity()));
    log::debug!("Loaded {} profile routes (most-specific-first)", routes.len());
    routes
}

/// Return the best-matching route, or None for no match.
///
/// Iterates in specificity order; first matching route wins.
pub fn match_profile_route<'a>(
    routes: &'a [ProfileRoute],
    platform: &str,
    guild_id: Option<&str>,
    chat_id: Option<&str>,
    thread_id: Option<&str>,
    parent_chat_id: Option<&str>,
) -> Option<&'a ProfileRoute> {
    for route in routes {
        if route.matches(platform, guild_id, chat_id, thread_id, parent_chat_id) {
            return Some(route);
        }
    }
    None
}
