//! Where the bot can post: the guilds it is in, their channels newest-first,
//! and the DM with the owner. Discord access sits behind a trait so the
//! sorting and shaping are tested without a network.

use anyhow::Context;
use serde_json::Value;

/// One destination the user may pick.
#[derive(Clone)]
pub struct Destination {
    pub id: String,
    pub label: String,
    /// Guild channels sort by snowflake (time-ordered) newest first.
    pub sort_key: u64,
    pub is_dm: bool,
}

/// A guild the bot belongs to.
pub struct Guild {
    pub id: String,
    pub name: String,
}

/// What the picker needs from Discord. Implementations must never put the
/// token into an error message.
pub trait ChannelSource {
    fn guilds(&self) -> anyhow::Result<Vec<Guild>>;
    fn channels(&self, guild_id: &str) -> anyhow::Result<Vec<Destination>>;
    /// The DM channel between the bot and `owner_id`.
    fn dm_channel(&self, owner_id: &str) -> anyhow::Result<Destination>;
}

/// The real thing: Discord REST over the just-written bot token.
pub struct RestChannels {
    token: String,
    runtime: tokio::runtime::Runtime,
}

const BASE: &str = "https://discord.com/api/v10";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

impl RestChannels {
    pub fn new(token: &str) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("cannot start the HTTP runtime: {e}"))?;
        Ok(Self {
            token: token.to_string(),
            runtime,
        })
    }

    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| anyhow::anyhow!("cannot build the HTTP client: {e}"))?;
        let response = client
            .get(format!("{BASE}{path}"))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Discord did not answer: {e}"))?;
        let status = response.status();
        let body = response
            .json::<Value>()
            .await
            .map_err(|e| anyhow::anyhow!("Discord sent something unreadable: {e}"))?;
        if !status.is_success() {
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            anyhow::bail!("Discord refused ({status}): {message}");
        }
        Ok(body)
    }

    async fn post_dm(&self, owner_id: &str) -> anyhow::Result<Value> {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| anyhow::anyhow!("cannot build the HTTP client: {e}"))?;
        let response = client
            .post(format!("{BASE}/users/@me/channels"))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "recipient_id": owner_id }))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Discord did not answer: {e}"))?;
        let status = response.status();
        let body = response
            .json::<Value>()
            .await
            .map_err(|e| anyhow::anyhow!("Discord sent something unreadable: {e}"))?;
        if !status.is_success() {
            let message = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            anyhow::bail!("Discord refused ({status}): {message}");
        }
        Ok(body)
    }
}

/// Channel types the bot can post into: text (0) and announcement (5).
fn postable(kind: u64) -> bool {
    matches!(kind, 0 | 5)
}

impl ChannelSource for RestChannels {
    fn guilds(&self) -> anyhow::Result<Vec<Guild>> {
        let body = self.runtime.block_on(self.get("/users/@me/guilds"))?;
        let list = body.as_array().context("guild list was not a list")?;
        Ok(list
            .iter()
            .filter_map(|g| {
                Some(Guild {
                    id: g.get("id")?.as_str()?.to_string(),
                    name: g.get("name")?.as_str()?.to_string(),
                })
            })
            .collect())
    }

    fn channels(&self, guild_id: &str) -> anyhow::Result<Vec<Destination>> {
        let body = self
            .runtime
            .block_on(self.get(&format!("/guilds/{guild_id}/channels")))?;
        let list = body.as_array().context("channel list was not a list")?;
        let mut out: Vec<Destination> = list
            .iter()
            .filter(|c| c.get("type").and_then(Value::as_u64).is_some_and(postable))
            .filter_map(|c| {
                Some(Destination {
                    id: c.get("id")?.as_str()?.to_string(),
                    label: c
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("(unnamed)")
                        .to_string(),
                    sort_key: snowflake_key(c.get("id")?.as_str()?),
                    is_dm: false,
                })
            })
            .collect();
        newest_first(&mut out);
        Ok(out)
    }

    fn dm_channel(&self, owner_id: &str) -> anyhow::Result<Destination> {
        let body = self.runtime.block_on(self.post_dm(owner_id))?;
        let id = body
            .get("id")
            .and_then(Value::as_str)
            .context("Discord did not return a DM channel")?;
        Ok(Destination {
            id: id.to_string(),
            label: "DM with the bot".to_string(),
            sort_key: snowflake_key(id),
            is_dm: true,
        })
    }
}

/// Snowflake IDs are time-ordered; the newest channel has the highest ID.
fn snowflake_key(id: &str) -> u64 {
    id.parse::<u64>().unwrap_or(0)
}

fn newest_first(list: &mut [Destination]) {
    list.sort_unstable_by_key(|d| std::cmp::Reverse(d.sort_key));
}

/// The full picker list for one guild: the owner's DM first (the plugin's
/// historical home channel), then the guild's channels newest-first.
pub fn destinations_for(
    source: &dyn ChannelSource,
    guild_id: &str,
    owner_id: &str,
) -> anyhow::Result<Vec<Destination>> {
    let mut channels = source.channels(guild_id)?;
    newest_first(&mut channels);
    let mut out = Vec::new();
    if let Ok(dm) = source.dm_channel(owner_id) {
        out.push(dm);
    }
    out.extend(channels);
    Ok(out)
}

#[path = "channel_picker_tests.rs"]
#[cfg(test)]
mod tests;
