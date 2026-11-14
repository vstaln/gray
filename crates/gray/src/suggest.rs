use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use gray_core::agent::Agent;
use gray_core::message::{ChatRequest, Message, Role};

const SYSTEM: &str = "Predict the user's most likely next message to this coding assistant. \
Reply with only that message, at most 12 words, no quotes.";
const EXCERPT_CHARS: usize = 800;
const MAX_WORDS: usize = 12;

static PENDING: Mutex<Option<String>> = Mutex::new(None);
static GEN: AtomicU64 = AtomicU64::new(0);

pub(crate) fn discard() {
    GEN.fetch_add(1, Ordering::Relaxed);
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(crate) fn ghost(input: &str, running: bool) -> Option<String> {
    let mut slot = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let text = slot.as_ref()?;
    if running || !input.trim().is_empty() {
        *slot = None;
        return None;
    }
    Some(text.clone())
}

pub(crate) fn take() -> Option<String> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub(crate) fn maybe_suggest(agent: Option<&Agent>, interactive: bool) {
    if !interactive || !crate::settings::prompt_suggestions() || !crate::away::is_focused() {
        return;
    }
    let Some(agent) = agent else {
        return;
    };
    let Some(req) = suggest_request(agent.messages()) else {
        return;
    };
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let stamped = GEN.load(Ordering::Relaxed);
    let provider = agent.provider_handle();
    tokio::spawn(async move {
        let Some(raw) = crate::auto_title::complete(provider, req).await else {
            return;
        };
        let Some(text) = clean_reply(&raw) else {
            return;
        };
        if GEN.load(Ordering::Relaxed) != stamped {
            return;
        }
        *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(text);
        crate::host::request_wake();
    });
}

fn clean_reply(raw: &str) -> Option<String> {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .trim_matches(|c| matches!(c, '"' | '\'' | '`' | '“' | '”'))
        .trim()
        .to_string();
    if line.is_empty() {
        return None;
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let capped = words
        .iter()
        .take(MAX_WORDS)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    (!capped.trim().is_empty()).then_some(capped)
}

fn suggest_request(messages: &[Message]) -> Option<ChatRequest> {
    let last_text = |role: Role| {
        messages
            .iter()
            .rev()
            .filter(|m| m.role == role && !m.injected)
            .map(Message::text_content)
            .find(|t| !t.trim().is_empty())
    };
    let reply = last_text(Role::Assistant)?;
    let user = last_text(Role::User)?;
    let tail = |s: &str| {
        let chars: Vec<char> = s.trim().chars().collect();
        chars[chars.len().saturating_sub(EXCERPT_CHARS)..]
            .iter()
            .collect::<String>()
    };
    Some(ChatRequest {
        system: Some(SYSTEM.to_string()),
        messages: vec![Message::user(format!(
            "Last request:\n{}\n\nLast reply:\n{}\n\nNext message:",
            tail(&user),
            tail(&reply)
        ))],
        tools: Vec::new(),
        max_tokens: Some(320),
    })
}
