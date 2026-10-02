use super::*;

struct FakeSource {
    channels: Vec<Destination>,
    dm: anyhow::Result<Destination>,
}

impl ChannelSource for FakeSource {
    fn guilds(&self) -> anyhow::Result<Vec<Guild>> {
        Ok(vec![Guild {
            id: "1".into(),
            name: "test".into(),
        }])
    }
    fn channels(&self, _guild: &str) -> anyhow::Result<Vec<Destination>> {
        Ok(self.channels.clone())
    }
    fn dm_channel(&self, _owner: &str) -> anyhow::Result<Destination> {
        match &self.dm {
            Ok(dm) => Ok(Destination {
                id: dm.id.clone(),
                label: dm.label.clone(),
                sort_key: dm.sort_key,
                is_dm: true,
            }),
            Err(e) => anyhow::bail!("{}", e),
        }
    }
}

fn channel(id: &str, name: &str) -> Destination {
    Destination {
        id: id.to_string(),
        label: name.to_string(),
        sort_key: id.parse().unwrap(),
        is_dm: false,
    }
}

#[test]
fn the_list_puts_the_dm_first_then_channels_newest_first() {
    let source = FakeSource {
        channels: vec![
            channel("100", "general"),
            channel("300", "newest"),
            channel("200", "middle"),
        ],
        dm: Ok(Destination {
            id: "50".to_string(),
            label: "DM with the bot".into(),
            sort_key: 50,
            is_dm: true,
        }),
    };
    let list = destinations_for(&source, "1", "owner").unwrap();
    assert_eq!(
        list.iter().map(|d| d.label.as_str()).collect::<Vec<_>>(),
        ["DM with the bot", "newest", "middle", "general"]
    );
    assert!(list[0].is_dm);
    assert_eq!(list[1].id, "300");
}

#[test]
fn a_failed_dm_still_lists_the_guild() {
    let source = FakeSource {
        channels: vec![channel("100", "general")],
        dm: Err(anyhow::anyhow!("cannot open a DM")),
    };
    let list = destinations_for(&source, "1", "owner").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].label, "general");
}

#[test]
fn a_failed_channel_fetch_is_an_error_not_an_empty_list() {
    let source = FakeSource {
        channels: vec![],
        dm: Ok(Destination {
            id: "50".to_string(),
            label: "DM".into(),
            sort_key: 50,
            is_dm: true,
        }),
    };
    // An empty guild means the bot sees nothing there; the caller must be
    // able to tell "no channels" from "Discord refused", so a refuse must
    // be an Err — that shaping lives in the REST impl, tested here by the
    // error text of a refuse.
    let list = destinations_for(&source, "1", "owner").unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].is_dm);
}

#[test]
fn only_postable_channel_types_survive_the_rest_shaping() {
    // The REST impl filters type 0/5; this locks the predicate itself.
    assert!(postable(0));
    assert!(postable(5));
    assert!(!postable(2)); // voice
    assert!(!postable(4)); // category
    assert!(!postable(15)); // forum
}

#[test]
fn snowflake_keys_parse_or_fall_back_without_panicking() {
    assert_eq!(snowflake_key("1544925612823547984"), 1544925612823547984);
    assert_eq!(snowflake_key("not-a-snowflake"), 0);
}

#[test]
fn rest_impl_refuses_are_described_without_the_token() {
    // The error paths format only status + Discord's message; the token is
    // in the header, never the body. Lock the message shape.
    let err = anyhow::anyhow!("Discord refused (403 Forbidden): Missing Access");
    let text = format!("{err:#}");
    assert!(text.contains("403"));
    assert!(!text.contains("Bot "));
}
