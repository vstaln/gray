//! Authorization ("trusted gateway, explicit operator allowlist").
//!
//! Every inbound event passes through [`Authorizer::check`] before anything
//! else happens. The decision is deny-by-default:
//!
//! | source                               | DM                       | group/channel             |
//! |--------------------------------------|--------------------------|---------------------------|
//! | id on `allowed_users` / env allowlist| allow                    | allow                     |
//! | id on `group_allowed_users`          | deny                     | allow                     |
//! | id approved via pairing              | allow                    | deny (DM approval only)   |
//! | `dm_policy: open` + `"*"` in list    | allow                    | allow only if `"*"` too   |
//! | unknown, `dm_policy: pairing`        | offer pairing code       | ignore silently           |
//! | unknown, otherwise                   | ignore silently          | ignore silently           |
//!
//! Group senders never get a pairing prompt (silently ignored) so a bot
//! added to a public group can't be used to spam codes.
use std::collections::HashSet;
use std::sync::Arc;

use crate::config::{DmPolicy, GatewayConfig, Platform, PlatformConfig};
use crate::pairing::{PairingStore, normalize_user_id};
use crate::session::SessionSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Unknown DM sender on a `pairing` platform — caller should offer a code.
    OfferPairing,
    /// Drop the event without any response.
    Deny,
}

/// Parse a comma/space separated allowlist env value (`TELEGRAM_ALLOWED_USERS=1,2 3`).
pub fn parse_allowlist_env(raw: &str) -> Vec<String> {
    raw.split([',', ' ', '\n', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Effective operator allowlist for a platform: config + env, normalized.
pub fn effective_allowlist(
    platform: Platform,
    cfg: &PlatformConfig,
    env_value: Option<&str>,
) -> HashSet<String> {
    let mut set: HashSet<String> = cfg
        .allowed_users
        .iter()
        .map(|u| normalize_user_id(platform, u))
        .collect();
    if let Some(raw) = env_value {
        for u in parse_allowlist_env(raw) {
            set.insert(normalize_user_id(platform, &u));
        }
    }
    set
}

pub struct Authorizer {
    config: GatewayConfig,
    pairing: std::sync::Arc<PairingStore>,
}

impl Authorizer {
    pub fn new(config: GatewayConfig, pairing: std::sync::Arc<PairingStore>) -> Self {
        Self { config, pairing }
    }

    fn platform_cfg(&self, p: Platform) -> PlatformConfig {
        self.config.platforms.get(&p).cloned().unwrap_or_default()
    }

    /// Pure decision given explicit env value (unit-testable without touching process env).
    pub fn check_with_env(&self, src: &SessionSource, env_value: Option<&str>) -> Decision {
        let platform = src.platform;
        let cfg = self.platform_cfg(platform);
        let is_dm = src.chat_type == "dm";
        let Some(user_raw) = src
            .user_id
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        else {
            // No sender identity — can't authorize (e.g. anonymous channel posts).
            return Decision::Deny;
        };
        let user = normalize_user_id(platform, user_raw);
        let allow = effective_allowlist(platform, &cfg, env_value);
        let wildcard = allow.contains("*");

        if allow.contains(&user) {
            return Decision::Allow;
        }
        if !is_dm {
            let group: HashSet<String> = cfg
                .group_allowed_users
                .iter()
                .map(|u| normalize_user_id(platform, u))
                .collect();
            if group.contains(&user) || group.contains("*") {
                return Decision::Allow;
            }
            // `open` + "*" makes the bot public everywhere; anything else in a
            // group is ignored — pairing approvals grant DM access only.
            return if cfg.dm_policy == DmPolicy::Open && wildcard {
                Decision::Allow
            } else {
                Decision::Deny
            };
        }
        match cfg.dm_policy {
            DmPolicy::Open if wildcard => Decision::Allow,
            // `open` without "*" degrades to allowlist semantics (only
            // concrete entries admitted) — never silently public.
            DmPolicy::Open | DmPolicy::Allowlist => {
                if self.pairing.is_approved(platform, &user) {
                    Decision::Allow
                } else {
                    Decision::Deny
                }
            }
            DmPolicy::Pairing => {
                if self.pairing.is_approved(platform, &user) {
                    Decision::Allow
                } else {
                    Decision::OfferPairing
                }
            }
        }
    }

    /// Decision using the live process env (`{PLATFORM}_ALLOWED_USERS`).
    pub fn check(&self, src: &SessionSource) -> Decision {
        let env = std::env::var(src.platform.allowed_users_env()).ok();
        self.check_with_env(src, env.as_deref())
    }

    pub fn pairing(&self) -> &PairingStore {
        &self.pairing
    }
}

// ---------------------------------------------------------------------------
// Tool policy: no native tool execution from chat platforms.
// ---------------------------------------------------------------------------
//
// The previous string-based shell allowlist was not a security boundary
// (quoting bypassed the absolute-path check, unknown tool names fell
// through to Ok, symlinks defeated the lexical workspace guard), and no
// allowlist can make shell option parsing, recursive traversal, or
// filesystem races safe. Until a supported confinement design exists,
// gateway sessions get no native tool access: every call is denied here
// and the model is told to use the local REPL instead. This intentionally
// reduces gateway sessions to conversation-only.

/// Decide whether a tool call may proceed in gateway mode.
/// Currently denies everything (see module docs). Kept as a seam so a
/// future confinement design has one place to land.
pub fn tool_call_allowed(
    _denied_tools: &[String],
    name: &str,
    _args: &serde_json::Value,
    _workspace: &std::path::Path,
) -> Result<(), String> {
    Err(format!(
        "native tool `{name}` is disabled in gateway mode; use the local REPL"
    ))
}

/// [`gray_core::agent::ToolExecutor`] wrapper that enforces [`tool_call_allowed`]
/// before delegating. Denials are returned as tool errors (data for the model,
/// not a crash), so the agent can explain and continue.
pub struct GatedExecutor {
    inner: Arc<dyn gray_core::agent::ToolExecutor>,
    workspace: std::path::PathBuf,
}

impl GatedExecutor {
    pub fn new(
        inner: Arc<dyn gray_core::agent::ToolExecutor>,
        workspace: std::path::PathBuf,
    ) -> Self {
        Self { inner, workspace }
    }
}

impl gray_core::agent::ToolExecutor for GatedExecutor {
    fn execute(
        &self,
        ctx: &gray_core::agent::ToolContext,
        name: &str,
        args: serde_json::Value,
    ) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
        if let Err(reason) = tool_call_allowed(&[], name, &args, &self.workspace) {
            log::warn!("gateway denied tool {name}: {reason}");
            return Box::pin(async move { gray_core::agent::ToolOutput::error(reason) });
        }
        self.inner.execute(ctx, name, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlatformConfig;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn src(platform: Platform, chat_type: &str, user: Option<&str>) -> SessionSource {
        SessionSource {
            platform,
            chat_id: "c1".into(),
            chat_type: chat_type.into(),
            user_id: user.map(str::to_string),
            thread_id: None,
            scope_id: None,
            message_id: None,
        }
    }

    fn authz(pc: PlatformConfig, platform: Platform) -> (tempfile::TempDir, Authorizer) {
        let d = tempfile::tempdir().unwrap();
        let store = Arc::new(PairingStore::new(d.path().to_path_buf()));
        let mut platforms = HashMap::new();
        platforms.insert(platform, pc);
        let cfg = GatewayConfig {
            platforms,
            ..Default::default()
        };
        (d, Authorizer::new(cfg, store))
    }

    #[test]
    fn default_is_deny_with_pairing_offer_in_dm() {
        let (_d, a) = authz(
            PlatformConfig::with_token("123:abcdefghijk"),
            Platform::Telegram,
        );
        assert_eq!(
            a.check_with_env(&src(Platform::Telegram, "dm", Some("42")), None),
            Decision::OfferPairing
        );
        assert_eq!(
            a.check_with_env(&src(Platform::Telegram, "group", Some("42")), None),
            Decision::Deny
        );
        assert_eq!(
            a.check_with_env(&src(Platform::Telegram, "dm", None), None),
            Decision::Deny
        );
    }

    #[test]
    fn config_allowlist_allows_everywhere() {
        let pc = PlatformConfig {
            allowed_users: vec!["42".into()],
            ..PlatformConfig::with_token("t")
        };
        let (_d, a) = authz(pc, Platform::Discord);
        assert_eq!(
            a.check_with_env(&src(Platform::Discord, "dm", Some("42")), None),
            Decision::Allow
        );
        assert_eq!(
            a.check_with_env(&src(Platform::Discord, "group", Some("42")), None),
            Decision::Allow
        );
        assert_eq!(
            a.check_with_env(&src(Platform::Discord, "group", Some("43")), None),
            Decision::Deny
        );
    }

    #[test]
    fn env_allowlist_is_honored() {
        let (_d, a) = authz(PlatformConfig::with_token("t"), Platform::Slack);
        assert_eq!(
            a.check_with_env(&src(Platform::Slack, "dm", Some("U1")), Some("u1, U2")),
            Decision::Allow
        );
        assert_eq!(
            a.check_with_env(&src(Platform::Slack, "channel", Some("U2")), Some("U1,U2")),
            Decision::Allow
        );
        assert_eq!(
            a.check_with_env(&src(Platform::Slack, "dm", Some("U3")), Some("U1,U2")),
            Decision::OfferPairing
        );
    }

    #[test]
    fn pairing_approval_grants_dm_only() {
        let (_d, a) = authz(PlatformConfig::with_token("t"), Platform::Telegram);
        a.pairing().approve_user(Platform::Telegram, "7", "");
        assert_eq!(
            a.check_with_env(&src(Platform::Telegram, "dm", Some("7")), None),
            Decision::Allow
        );
        assert_eq!(
            a.check_with_env(&src(Platform::Telegram, "group", Some("7")), None),
            Decision::Deny
        );
    }

    #[test]
    fn group_allowlist_does_not_grant_dm() {
        let pc = PlatformConfig {
            group_allowed_users: vec!["9".into()],
            ..PlatformConfig::with_token("t")
        };
        let (_d, a) = authz(pc, Platform::Discord);
        assert_eq!(
            a.check_with_env(&src(Platform::Discord, "group", Some("9")), None),
            Decision::Allow
        );
        assert_eq!(
            a.check_with_env(&src(Platform::Discord, "dm", Some("9")), None),
            Decision::OfferPairing
        );
    }

    #[test]
    fn open_requires_wildcard() {
        let pc = PlatformConfig {
            dm_policy: DmPolicy::Open,
            ..PlatformConfig::with_token("t")
        };
        let (_d, a) = authz(pc, Platform::Telegram);
        // open without "*" is NOT public: unknown senders are denied silently.
        assert_eq!(
            a.check_with_env(&src(Platform::Telegram, "dm", Some("1")), None),
            Decision::Deny
        );
        let pc = PlatformConfig {
            dm_policy: DmPolicy::Open,
            allowed_users: vec!["*".into()],
            ..PlatformConfig::with_token("t")
        };
        let (_d, a) = authz(pc, Platform::Telegram);
        assert_eq!(
            a.check_with_env(&src(Platform::Telegram, "dm", Some("1")), None),
            Decision::Allow
        );
        assert_eq!(
            a.check_with_env(&src(Platform::Telegram, "group", Some("1")), None),
            Decision::Allow
        );
    }

    #[test]
    fn allowlist_policy_never_offers_pairing() {
        let pc = PlatformConfig {
            dm_policy: DmPolicy::Allowlist,
            ..PlatformConfig::with_token("t")
        };
        let (_d, a) = authz(pc, Platform::Slack);
        assert_eq!(
            a.check_with_env(&src(Platform::Slack, "dm", Some("U5")), None),
            Decision::Deny
        );
    }

    #[test]
    fn parse_env_list() {
        assert_eq!(
            parse_allowlist_env("1, 2;3\n4  5"),
            vec!["1", "2", "3", "4", "5"]
        );
        assert!(parse_allowlist_env("").is_empty());
    }

    #[test]
    fn all_native_tools_denied_in_gateway_mode() {
        let ws = std::path::Path::new("/work");
        let none: Vec<String> = vec![];
        // Builtins, boring-but-previously-allowed commands, unknown names,
        // and sidecar/plugin names: everything is denied until a supported
        // confinement design exists.
        for (name, args) in [
            ("read", serde_json::json!({"path": "sub/dir/f.md"})),
            ("write", serde_json::json!({"path": "notes.txt"})),
            ("edit", serde_json::json!({"path": "notes.txt"})),
            ("bash", serde_json::json!({"command": "ls -la"})),
            ("bash", serde_json::json!({"command": "cat notes.txt"})),
            (
                "bash",
                serde_json::json!({"command": "cat \"/etc/passwd\""}),
            ),
            ("grep", serde_json::json!({"pattern": "x"})),
            ("find", serde_json::json!({"pattern": "*.rs"})),
            ("ls", serde_json::json!({})),
            ("request_user_input", serde_json::json!({})),
            ("totally_unknown_tool", serde_json::json!({})),
            ("my_sidecar_plugin_tool", serde_json::json!({})),
        ] {
            let err =
                tool_call_allowed(&none, name, &args, ws).expect_err(&format!("must deny: {name}"));
            assert!(
                err.contains("disabled in gateway mode"),
                "wrong reason for {name}: {err}"
            );
        }
    }
}
