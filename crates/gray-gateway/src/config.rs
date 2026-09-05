//! Gateway config
//!
//! Security model ("trusted gateway, explicit operator allowlist"):
//! - every platform is deny-by-default; nobody talks to the agent unless an
//!   operator put them on an allowlist (config, `{PLATFORM}_ALLOWED_USERS` env)
//!   or approved a pairing code via `gray gateway pairing approve`;
//! - `dm_policy: open` is only honored when `allowed_users` literally contains
//!   `"*"` — there is no `allow_all: true` switch;
//! - groups never pair: an unknown sender in a group is silently ignored.
use std::collections::HashMap;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform { Telegram, Discord, Slack }
impl std::str::FromStr for Platform {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "telegram" => Ok(Self::Telegram),
            "discord" => Ok(Self::Discord),
            "slack" => Ok(Self::Slack),
            _ => Err(format!("unknown platform {s}")),
        }
    }
}
impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::Telegram => write!(f, "telegram"), Self::Discord => write!(f, "discord"), Self::Slack => write!(f, "slack") }
    }
}
impl Platform {
    /// Human-facing name for menus and status lines. `Display` stays lowercase:
    /// it feeds command strings and persisted session keys (`gray:main:telegram:…`).
    pub fn label(&self) -> &'static str {
        match self { Self::Telegram => "Telegram", Self::Discord => "Discord", Self::Slack => "Slack" }
    }
    /// All platforms, for iteration in status/onboarding code.
    pub const ALL: [Platform; 3] = [Platform::Telegram, Platform::Discord, Platform::Slack];
    /// Outbound hard limit in UTF-16 code units (Telegram 4096, Discord 2000, Slack 39000).
    pub fn max_message_len(&self) -> usize {
        match self { Self::Telegram => 4096, Self::Discord => 2000, Self::Slack => 39000 }
    }
    /// Env var consulted for the user allowlist.
    pub fn allowed_users_env(&self) -> &'static str {
        match self { Self::Telegram => "TELEGRAM_ALLOWED_USERS", Self::Discord => "DISCORD_ALLOWED_USERS", Self::Slack => "SLACK_ALLOWED_USERS" }
    }
}

/// How unknown DM senders are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DmPolicy {
    /// Unknown senders get a one-time pairing code; nothing is processed until approved.
    #[default]
    Pairing,
    /// Only allowlisted senders; unknown senders are ignored silently.
    Allowlist,
    /// Public bot. Only effective when `allowed_users` contains `"*"`.
    Open,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub token: Option<String>,
    #[serde(default)] pub app_token: Option<String>,
    #[serde(default)] pub home_channel: Option<String>,
    /// Operator allowlist of platform user ids (numeric for Telegram/Discord, `U…` for Slack).
    /// `"*"` means everyone (only meaningful with `dm_policy: open`).
    #[serde(default)] pub allowed_users: Vec<String>,
    /// Extra allowlist for group/channel senders (in addition to `allowed_users`).
    #[serde(default)] pub group_allowed_users: Vec<String>,
    #[serde(default)] pub dm_policy: DmPolicy,
}
impl Default for PlatformConfig {
    fn default() -> Self {
        Self { enabled: false, token: None, app_token: None, home_channel: None, allowed_users: Vec::new(), group_allowed_users: Vec::new(), dm_policy: DmPolicy::default() }
    }
}
impl PlatformConfig {
    /// Convenience for tests and REPL wiring: enabled + token, everything else default.
    pub fn with_token(token: impl Into<String>) -> Self {
        Self { enabled: true, token: Some(token.into()), ..Default::default() }
    }
}

/// When gateway sessions reset without a manual `/reset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResetMode {
    /// Never auto-reset (current behavior).
    #[default]
    None,
    /// Reset when the session has been idle for `idle_secs`.
    Idle,
    /// Reset once per day after `at_hour` (UTC).
    Daily,
}

/// Auto-reset policy for gateway sessions. Defaults preserve current
/// behavior (never reset); checked on message routing in the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetPolicy {
    #[serde(default)]
    pub mode: ResetMode,
    #[serde(default = "default_idle_secs")]
    pub idle_secs: u64,
    /// UTC hour of the daily boundary (0-23).
    #[serde(default)]
    pub at_hour: u8,
}

impl Default for ResetPolicy {
    fn default() -> Self {
        Self { mode: ResetMode::None, idle_secs: default_idle_secs(), at_hour: 0 }
    }
}

fn default_idle_secs() -> u64 {
    3600
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    #[serde(default)] pub platforms: HashMap<Platform, PlatformConfig>,
    #[serde(default="default_group_per_user")] pub group_per_user: bool,
    #[serde(default)] pub thread_per_user: bool,
    /// Auto-start the in-process gateway when gray launches (toggle: /gateway autostart on|off).
    #[serde(default = "default_autostart")] pub autostart: bool,
    /// Tools the agent may never call while driven from a chat platform
    /// (no interactive operator to confirm). Merged with the built-in deny set.
    #[serde(default)] pub denied_tools: Vec<String>,
    /// Stream partial replies via edit-in-place where the platform supports it.
    #[serde(default = "default_true")] pub streaming: bool,
    /// Run due cron jobs inside the gateway and deliver output to each platform's home channel.
    #[serde(default = "default_true")] pub cron_delivery: bool,
    /// Auto-reset policy for gateway sessions (default: never).
    #[serde(default)] pub reset_policy: ResetPolicy,
}
fn default_group_per_user() -> bool { true }
fn default_autostart() -> bool { true }
fn default_true() -> bool { true }
impl Default for GatewayConfig {
    fn default() -> Self {
        Self { platforms: HashMap::new(), group_per_user: true, thread_per_user: false, autostart: true, denied_tools: Vec::new(), streaming: true, cron_delivery: true, reset_policy: ResetPolicy::default() }
    }
}
pub fn gray_home_dir() -> anyhow::Result<PathBuf> {
    let base = std::env::var("GRAY_HOME").or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.gray"))).map_err(|_| anyhow::anyhow!("cannot resolve home"))?;
    Ok(PathBuf::from(base))
}
pub fn gray_gateway_path() -> anyhow::Result<PathBuf> {
    gray_home_dir().map(|b| b.join("gateway.yaml"))
}
pub fn load_gateway_config() -> GatewayConfig {
    let Ok(path) = gray_gateway_path() else { return GatewayConfig::default(); };
    std::fs::read_to_string(&path).ok().and_then(|s| serde_yaml::from_str(&s).ok()).unwrap_or_default()
}
pub fn save_gateway_config(cfg: &GatewayConfig) -> anyhow::Result<()> {
    let path = gray_gateway_path()?;
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    let s = serde_yaml::to_string(cfg)?;
    std::fs::write(&path, s)?;
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?; }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_display_and_serde_are_byte_identical() {
        // Persisted session keys depend on these strings — never change them.
        for (p, s) in [(Platform::Telegram, "telegram"), (Platform::Discord, "discord"), (Platform::Slack, "slack")] {
            assert_eq!(p.to_string(), s);
            assert_eq!(serde_json::to_string(&p).unwrap(), format!("\"{s}\""));
            assert_eq!(s.parse::<Platform>().unwrap(), p);
        }
    }

    #[test]
    fn legacy_yaml_loads_with_secure_defaults() {
        let yaml = "platforms:\n  telegram:\n    enabled: true\n    token: 123:abc\n";
        let cfg: GatewayConfig = serde_yaml::from_str(yaml).unwrap();
        let t = &cfg.platforms[&Platform::Telegram];
        assert!(t.allowed_users.is_empty());
        assert_eq!(t.dm_policy, DmPolicy::Pairing);
        assert!(cfg.streaming);
        assert!(cfg.denied_tools.is_empty());
    }

    #[test]
    fn dm_policy_serde_lowercase() {
        assert_eq!(serde_yaml::to_string(&DmPolicy::Allowlist).unwrap().trim(), "allowlist");
        assert_eq!(serde_yaml::from_str::<DmPolicy>("open").unwrap(), DmPolicy::Open);
    }

    #[test]
    fn reset_policy_defaults_to_none_preserving_behavior() {
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.reset_policy.mode, ResetMode::None);
        // Legacy yaml without the key still loads and never resets.
        let yaml = "platforms:\n  telegram:\n    enabled: true\n    token: 123:abc\n";
        let cfg: GatewayConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.reset_policy.mode, ResetMode::None);
    }

    #[test]
    fn reset_policy_serde_roundtrip() {
        let yaml = "mode: idle\nidle_secs: 60\nat_hour: 3\n";
        let p: ResetPolicy = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.mode, ResetMode::Idle);
        assert_eq!(p.idle_secs, 60);
        assert_eq!(p.at_hour, 3);
        let yaml = "mode: daily\nat_hour: 4\n";
        let p: ResetPolicy = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.mode, ResetMode::Daily);
        assert_eq!(p.idle_secs, default_idle_secs());
        // Missing keys default, never fail old configs.
        let p: ResetPolicy = serde_yaml::from_str("{}\n").unwrap();
        assert_eq!(p.mode, ResetMode::None);
    }
}
