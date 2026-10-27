use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use gray_core::agent::{Agent, Provider};
use gray_core::event::StreamEvent;
use gray_core::message::{ChatRequest, Message, Role};

use crate::repl::SessionState;

const TIMEOUT: Duration = Duration::from_secs(30);

const MAX_WORDS: usize = 8;

const EXCERPT_CHARS: usize = 600;
const SYSTEM: &str = "You name chat sessions. Reply with a short title of about six words \
that says what the session is about. Plain text only: no quotes, no trailing period, \
no preamble.";

pub(crate) fn maybe_start(state: &SessionState, agent: &Agent) {
    if crate::session_store::session_origin_from_env().is_some() {
        return;
    }
    let Some((first_user, reply)) = first_exchange(agent.messages()) else {
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

pub(crate) fn first_exchange(messages: &[Message]) -> Option<(String, String)> {
    let has_reply = |m: &Message| m.role == Role::Assistant && !m.text_content().trim().is_empty();
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
