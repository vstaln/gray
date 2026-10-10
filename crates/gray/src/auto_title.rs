//! Auto-naming: a short title for a session after its first reply.
//!
//! One background model call per session, on the session's own provider. The
//! result goes to `<id>.title`, the same sidecar a manual rename writes, so the
//! picker needs no other change. It never delays the prompt, and it is silent
//! on failure: the picker keeps the first-message preview.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use gray_core::agent::{Agent, Provider};
use gray_core::event::StreamEvent;
use gray_core::message::{ChatRequest, Message, Role};

use crate::repl::SessionState;

/// Longest wait for the title call. Past it, the preview stays.
const TIMEOUT: Duration = Duration::from_secs(30);
/// Words kept from the model's line. Titles read best short.
const MAX_WORDS: usize = 8;
/// Characters of each side of the exchange sent to the model.
const EXCERPT_CHARS: usize = 600;
const SYSTEM: &str = "You name chat sessions. Reply with a short title of about six words \
that says what the session is about. Plain text only: no quotes, no trailing period, \
no preamble.";

/// Starts the title call when this turn produced the session's first reply.
/// Returns at once; the call runs in the background.
pub(crate) fn maybe_start(state: &SessionState, agent: &Agent, initial_count: usize) {
    // Gray-made sessions (cron, subagent, print) are never named.
    if crate::session_store::session_origin_from_env().is_some() {
        return;
    }
    let Some((first_user, reply)) = first_exchange(agent.messages(), initial_count) else {
        return;
    };
    if crate::session_store::opener_tag(&first_user).is_some() {
        return;
    }
    let root = state.store.root_dir().to_path_buf();
    if !crate::session_store::claim_auto_title(&root, &state.session_id) {
        return;
    }
    let id = state.session_id.clone();
    let provider = agent.provider_handle();
    tokio::spawn(async move {
        let Some(raw) = complete(provider, title_request(&first_user, &reply)).await else {
            return;
        };
        let Some(title) = title_from_reply(&raw) else {
            return;
        };
        if let Err(e) = crate::session_store::set_auto_title(&root, &id, &title) {
            log::debug!(target: "gray_title", "auto title for {}: {e}", id.as_str());
        }
    });
}

/// The opening user message and the first assistant reply, when this turn
/// (the messages from `initial_count` on) produced the session's first reply.
/// A session resumed after it already had a reply is never named.
pub(crate) fn first_exchange(
    messages: &[Message],
    initial_count: usize,
) -> Option<(String, String)> {
    let has_reply = |m: &Message| m.role == Role::Assistant && !m.text_content().trim().is_empty();
    let before = &messages[..initial_count.min(messages.len())];
    if before.iter().any(has_reply) {
        return None;
    }
    let reply = messages.iter().find(|m| has_reply(m))?.text_content();
    let first_user = messages
        .iter()
        .find(|m| m.role == Role::User && !m.injected)?
        .text_content();
    if first_user.trim().is_empty() {
        return None;
    }
    Some((first_user, reply))
}

pub(crate) fn title_request(first_user: &str, reply: &str) -> ChatRequest {
    let body = format!(
        "First message:\n{}\n\nFirst reply:\n{}\n\nTitle for this session:",
        excerpt(first_user),
        excerpt(reply),
    );
    ChatRequest {
        system: Some(SYSTEM.to_string()),
        messages: vec![Message::user(body)],
        tools: Vec::new(),
        max_tokens: Some(64),
    }
}

fn excerpt(s: &str) -> String {
    s.trim().chars().take(EXCERPT_CHARS).collect()
}

/// The title from the model's reply: its first non-empty line, with a leading
/// "Title:", quotes, markdown and trailing punctuation dropped, capped at
/// [`MAX_WORDS`] words. `None` when nothing is left.
pub(crate) fn title_from_reply(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = if line.to_ascii_lowercase().starts_with("title:") {
        &line[6..]
    } else {
        line
    };
    let line = line.trim_matches(|c: char| c.is_whitespace() || "\"'`*#_".contains(c));
    let line = line.trim_end_matches(|c: char| ".!?:;".contains(c));
    let words: Vec<&str> = line.split_whitespace().take(MAX_WORDS).collect();
    crate::session_store::clean_title(&words.join(" "))
}

/// Sends the request and collects the reply's text. `None` on a stream error,
/// an empty reply, or the timeout.
async fn complete(provider: Arc<dyn Provider>, req: ChatRequest) -> Option<String> {
    let mut stream = provider.stream(req);
    let drain = async {
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::TextDelta { delta }) => text.push_str(&delta),
                Ok(StreamEvent::MessageComplete { .. }) => break,
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        Some(text)
    };
    tokio::time::timeout(TIMEOUT, drain).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_store::{SessionId, write_title};

    #[test]
    fn a_model_line_becomes_a_short_plain_title() {
        assert_eq!(
            title_from_reply("\"Fixing the login redirect loop.\"").as_deref(),
            Some("Fixing the login redirect loop")
        );
        assert_eq!(
            title_from_reply("Title: **Parser bug in tags**").as_deref(),
            Some("Parser bug in tags")
        );
        assert_eq!(
            title_from_reply("\n\n  Debugging flaky CI runs  \nextra prose").as_deref(),
            Some("Debugging flaky CI runs")
        );
    }

    #[test]
    fn long_titles_are_capped_in_words() {
        assert_eq!(
            title_from_reply("one two three four five six seven eight nine ten").as_deref(),
            Some("one two three four five six seven eight")
        );
    }

    #[test]
    fn empty_replies_name_nothing() {
        assert_eq!(title_from_reply("   \n \n"), None);
        assert_eq!(title_from_reply("\"\""), None);
    }

    #[test]
    fn the_first_exchange_is_named_only_when_this_turn_gave_it() {
        let turn = vec![
            Message::user("fix the login loop"),
            Message::assistant("Found it: the redirect"),
        ];
        assert_eq!(
            first_exchange(&turn, 0),
            Some(("fix the login loop".into(), "Found it: the redirect".into()))
        );
        // A session resumed after it already had a reply is left alone.
        let resumed = vec![
            Message::user("old"),
            Message::assistant("old reply"),
            Message::user("again"),
            Message::assistant("new"),
        ];
        assert_eq!(first_exchange(&resumed, 2), None);
    }

    #[test]
    fn injected_notices_are_never_the_opener() {
        let msgs = vec![
            Message::user_injected("[Background task notification] done"),
            Message::user("real question"),
            Message::assistant("answer"),
        ];
        assert_eq!(
            first_exchange(&msgs, 0).map(|(user, _)| user),
            Some("real question".into())
        );
    }

    #[test]
    fn a_session_is_asked_once() {
        let dir = tempfile::tempdir().unwrap();
        let id = SessionId::new("tectonic-nickel-cyclone");
        assert!(crate::session_store::claim_auto_title(dir.path(), &id));
        assert!(!crate::session_store::claim_auto_title(dir.path(), &id));
    }

    #[test]
    fn a_manual_title_blocks_the_auto_name_and_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let id = SessionId::new("fungal-plasma-glacier");
        write_title(dir.path(), &id, "My name").unwrap();
        assert!(!crate::session_store::claim_auto_title(dir.path(), &id));
        assert!(!crate::session_store::set_auto_title(dir.path(), &id, "Model name").unwrap());
        let kept = std::fs::read_to_string(dir.path().join("fungal-plasma-glacier.title")).unwrap();
        assert_eq!(kept, "My name");
    }

    #[test]
    fn an_auto_title_lands_once_on_an_untitled_session() {
        let dir = tempfile::tempdir().unwrap();
        let id = SessionId::new("hyperbolic-yttrium-resonance");
        assert!(crate::session_store::claim_auto_title(dir.path(), &id));
        assert!(crate::session_store::set_auto_title(dir.path(), &id, "Resonance tuning").unwrap());
        assert!(!crate::session_store::set_auto_title(dir.path(), &id, "Second try").unwrap());
        let got =
            std::fs::read_to_string(dir.path().join("hyperbolic-yttrium-resonance.title")).unwrap();
        assert_eq!(got, "Resonance tuning");
    }

    #[test]
    fn subagent_and_background_openers_are_recognised() {
        assert_eq!(
            crate::session_store::opener_tag("[Background task notification] x"),
            Some("background")
        );
        assert_eq!(
            crate::session_store::opener_tag("You are a worker for the parser"),
            Some("worker")
        );
        assert_eq!(crate::session_store::opener_tag("How do I fix this?"), None);
    }
}
