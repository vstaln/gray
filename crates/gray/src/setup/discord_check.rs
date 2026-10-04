//! Discord onboarding: ask Discord about a bot token before anything is
//! written. One `GET /applications/@me` with the bot token answers it all:
//! the token works, whether Message Content Intent is on, how many servers
//! the bot is in, and who owns the application (so the owner is allowlisted
//! without Developer Mode). Hermes parity: plugins/platforms/discord/onboarding.py.
//!
//! The parsers are pure and tested with fixture JSON; only
//! [`check_bot_token`] touches the network. No error ever carries the token.

use serde_json::Value;

pub const API_BASE: &str = "https://discord.com/api/v10";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Every permission the bridge exercises (text, threads, reactions, voice),
/// named so the integer in the invite link and the docs can be re-derived
/// instead of trusted.
pub const INVITE_PERMISSION_BITS: &[(&str, u32)] = &[
    ("Add Reactions", 6),
    ("View Channels", 10),
    ("Send Messages", 11),
    ("Embed Links", 14),
    ("Attach Files", 15),
    ("Read Message History", 16),
    ("Connect", 20),
    ("Speak", 21),
    ("Create Public Threads", 35),
    ("Send Messages in Threads", 38),
];

pub const INVITE_PERMISSIONS: u64 = {
    let mut sum = 0u64;
    let mut i = 0;
    while i < INVITE_PERMISSION_BITS.len() {
        sum |= 1 << INVITE_PERMISSION_BITS[i].1;
        i += 1;
    }
    sum
};

/// Application flags: the full and the limited variant of each intent.
const FLAGS_GUILD_MEMBERS: u64 = (1 << 14) | (1 << 15);
const FLAGS_MESSAGE_CONTENT: u64 = (1 << 18) | (1 << 19);
const TEAM_MEMBER_ACCEPTED: u64 = 2;

/// What Discord said about the application behind a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotCheck {
    pub app_id: String,
    pub bot_name: String,
    /// `(user_id, username)`: the application owner, or every accepted
    /// member of the owning team.
    pub owners: Vec<(String, String)>,
    pub message_content: bool,
    pub server_members: bool,
    pub server_count: Option<u64>,
}

impl BotCheck {
    /// `integration_type=0` is a server install, so Discord skips the "add
    /// to server or to my apps?" question for apps that allow user installs.
    pub fn invite_url(&self) -> String {
        format!(
            "https://discord.com/oauth2/authorize?client_id={}&scope=bot+applications.commands&permissions={INVITE_PERMISSIONS}&integration_type=0",
            self.app_id
        )
    }

    /// The Bot page, where the Privileged Gateway Intents toggles live.
    pub fn bot_settings_url(&self) -> String {
        format!(
            "https://discord.com/developers/applications/{}/bot",
            self.app_id
        )
    }
}

/// Why a token could not be confirmed. `Display` never includes the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// 401: Discord does not know this token.
    Rejected,
    /// Any other HTTP answer: unverifiable, not proven wrong.
    Status(u16),
    /// No answer at all (offline, DNS, timeout) or an unreadable one.
    Unreachable(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::Rejected => write!(f, "Discord rejected the token"),
            CheckError::Status(code) => write!(f, "Discord answered {code}"),
            CheckError::Unreachable(why) => write!(f, "{why}"),
        }
    }
}

/// The header Discord wants for bot tokens: `Bot <token>`, never `Bearer`.
pub fn auth_header(token: &str) -> String {
    format!("Bot {token}")
}

/// Strip what a rich-text paste adds: non-ASCII (curly quotes, lookalike
/// glyphs, zero-width spaces) can never be in an HTTP header, then the
/// surrounding whitespace. Applied before the check and before the save.
pub fn clean_token(raw: &str) -> String {
    raw.chars()
        .filter(char::is_ascii)
        .collect::<String>()
        .trim()
        .to_string()
}

/// A real bot token is dot-separated base64 and never purely numeric; a
/// numeric paste is the application ID from the General Information page.
pub fn token_shape_error(token: &str) -> Option<&'static str> {
    let token = token.trim();
    if !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit()) {
        return Some(
            "That looks like a numeric application ID, not a bot token. Paste the bot token from the Discord Developer Portal (Bot page), not the application ID (General Information page).",
        );
    }
    None
}

/// Whitespace or control characters inside the token: not header-safe.
pub fn has_inner_break(token: &str) -> bool {
    token
        .chars()
        .any(|c| c.is_ascii_whitespace() || c.is_ascii_control())
}

/// Read a `GET /applications/@me` body into a [`BotCheck`].
pub fn parse_application(app: &Value) -> Result<BotCheck, String> {
    let app_id = id_string(app.get("id")).ok_or("Discord's answer had no application id")?;
    let flags = app.get("flags").and_then(Value::as_u64).unwrap_or(0);
    let bot_name = app
        .get("bot")
        .and_then(|b| b.get("username"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| app.get("name").and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .unwrap_or(&app_id)
        .to_string();
    Ok(BotCheck {
        bot_name,
        owners: owners(app),
        message_content: flags & FLAGS_MESSAGE_CONTENT != 0,
        server_members: flags & FLAGS_GUILD_MEMBERS != 0,
        server_count: app.get("approximate_guild_count").and_then(Value::as_u64),
        app_id,
    })
}

/// The owner, or every accepted member of the owning team.
pub fn owners(app: &Value) -> Vec<(String, String)> {
    let user = |u: &Value| -> Option<(String, String)> {
        let id = id_string(u.get("id"))?;
        let name = u
            .get("username")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(&id)
            .to_string();
        Some((id, name))
    };
    if let Some(team) = app.get("team").filter(|t| t.is_object()) {
        return team
            .get("members")
            .and_then(Value::as_array)
            .map(|members| {
                members
                    .iter()
                    .filter(|m| {
                        m.get("membership_state").and_then(Value::as_u64)
                            == Some(TEAM_MEMBER_ACCEPTED)
                    })
                    .filter_map(|m| m.get("user").and_then(user))
                    .collect()
            })
            .unwrap_or_default();
    }
    app.get("owner").and_then(user).into_iter().collect()
}

/// Snowflakes arrive as strings; tolerate a bare number too.
fn id_string(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Start from whoever is already allowed and only add: reconfiguring never
/// drops an entry. Order is kept, duplicates are not.
pub fn merge_allowed(existing: &[String], extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in existing.iter().chain(extra) {
        let id = id.trim();
        if !id.is_empty() && !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// The lines that walk the operator to the intent toggle.
pub fn intent_lines(check: &BotCheck) -> Vec<String> {
    vec![
        "Message Content Intent is OFF: Discord will refuse the bot's connection until it's on."
            .to_string(),
        format!("Turn it on here: {}", check.bot_settings_url()),
        "(Privileged Gateway Intents \u{2192} Message Content Intent \u{2192} Save Changes)"
            .to_string(),
    ]
}

/// The invite lines; a bot in no server yet gets the more direct wording.
pub fn invite_lines(check: &BotCheck) -> Vec<String> {
    let head = if check.server_count == Some(0) {
        "The bot isn't in any server yet. Open this link to add it to yours:"
    } else {
        "Invite link (adds the bot to a server with the permissions gray uses):"
    };
    vec![
        head.to_string(),
        check.invite_url(),
        "Once you share a server with the bot you can also DM it directly.".to_string(),
    ]
}

/// "@alice" or "@alice, @bob" for the allowlist question and report.
pub fn owner_names(check: &BotCheck) -> String {
    check
        .owners
        .iter()
        .map(|(_, name)| format!("@{name}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Ask Discord about `token`. Runs on its own thread with its own runtime,
/// so it is safe from inside or outside an async context.
pub fn check_bot_token(token: &str) -> Result<BotCheck, CheckError> {
    check_bot_token_at(API_BASE, token)
}

pub fn check_bot_token_at(base: &str, token: &str) -> Result<BotCheck, CheckError> {
    let (base, token) = (base.to_string(), token.trim().to_string());
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| CheckError::Unreachable(format!("cannot start the HTTP runtime: {e}")))?;
        runtime.block_on(fetch_application(&base, &token))
    })
    .join()
    .unwrap_or_else(|_| {
        Err(CheckError::Unreachable(
            "the token check crashed".to_string(),
        ))
    })
}

async fn fetch_application(base: &str, token: &str) -> Result<BotCheck, CheckError> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| CheckError::Unreachable(format!("cannot build the HTTP client: {e}")))?;
    let response = client
        .get(format!("{base}/applications/@me"))
        .header(reqwest::header::AUTHORIZATION, auth_header(token))
        .send()
        .await
        .map_err(|e| CheckError::Unreachable(format!("Discord did not answer: {e}")))?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(CheckError::Rejected);
    }
    if !status.is_success() {
        return Err(CheckError::Status(status.as_u16()));
    }
    let body = response
        .json::<Value>()
        .await
        .map_err(|e| CheckError::Unreachable(format!("Discord sent something unreadable: {e}")))?;
    parse_application(&body).map_err(CheckError::Unreachable)
}

#[path = "discord_check_tests.rs"]
#[cfg(test)]
pub(crate) mod tests;
