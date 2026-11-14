use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gray_core::agent::Agent;
use gray_core::message::{ChatRequest, Message, Role};

const AWAY_AFTER: Duration = Duration::from_secs(5 * 60);
const EXCERPT_CHARS: usize = 1500;
const SYSTEM: &str = "The user stepped away from this coding session and just came back. \
In one or two short sentences, remind them what they were doing and what comes next. \
Plain text only, no preamble.";

static LOST_AT: Mutex<Option<Instant>> = Mutex::new(None);
static RETURNED: AtomicBool = AtomicBool::new(false);

pub(crate) fn focus(gained: bool) {
    let mut lost = LOST_AT.lock().unwrap_or_else(|e| e.into_inner());
    if !gained {
        lost.get_or_insert_with(Instant::now);
        return;
    }
    if lost.take().is_some_and(|t| t.elapsed() >= AWAY_AFTER) && crate::settings::away_recap() {
        RETURNED.store(true, Ordering::Relaxed);
        crate::host::request_wake();
    }
}

pub(crate) fn maybe_recap(agent: Option<&Agent>) {
    if !RETURNED.swap(false, Ordering::Relaxed) {
        return;
    }
    let Some(agent) = agent else { return };
    let Some(req) = recap_request(agent.messages()) else {
        return;
    };
    let provider = agent.provider_handle();
    tokio::spawn(async move {
        if let Some(text) = crate::auto_title::complete(provider, req).await {
            let text = text.trim();
            if !text.is_empty() {
                crate::host::queue_say(format!("※ recap: {text}"));
            }
        }
    });
}

fn recap_request(messages: &[Message]) -> Option<ChatRequest> {
    let last_text = |role: Role| {
        messages
            .iter()
            .rev()
            .filter(|m| m.role == role && !m.injected)
            .map(Message::text_content)
            .find(|t| !t.trim().is_empty())
    };
    let user = last_text(Role::User)?;
    let reply = last_text(Role::Assistant).unwrap_or_default();
    let tail = |s: &str| {
        let chars: Vec<char> = s.trim().chars().collect();
        chars[chars.len().saturating_sub(EXCERPT_CHARS)..]
            .iter()
            .collect::<String>()
    };
    Some(ChatRequest {
        system: Some(SYSTEM.to_string()),
        messages: vec![Message::user(format!(
            "Last request:\n{}\n\nLast reply:\n{}\n\nRecap:",
            tail(&user),
            tail(&reply)
        ))],
        tools: Vec::new(),
        max_tokens: Some(160),
    })
}

pub(crate) fn is_focused() -> bool {
    LOST_AT.lock().unwrap_or_else(|e| e.into_inner()).is_none()
}
