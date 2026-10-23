use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use crate::message::{ChatRequest, Message, ToolDef};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint {
    provider: u64,
    system: u64,
    tools: u64,
    messages: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Provider,
    System,
    Tools,

    HistoryAt(usize),

    HistoryTruncated { from: usize, to: usize },
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Change::Provider => write!(f, "provider"),
            Change::System => write!(f, "system"),
            Change::Tools => write!(f, "tools"),
            Change::HistoryAt(k) => write!(f, "history@{k}"),
            Change::HistoryTruncated { from, to } => write!(f, "history truncated {from}->{to}"),
        }
    }
}

fn digest<T: serde::Serialize + ?Sized>(value: &T) -> u64 {
    let mut h = DefaultHasher::new();
    serde_json::to_string(value)
        .unwrap_or_default()
        .hash(&mut h);
    h.finish()
}

impl Fingerprint {
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Digest of the stored message at `index`; lets a caller re-derive
    /// wire-only entries (the synthetic continue tail) during prefix proofs.
    pub fn message_digest_at(&self, index: usize) -> Option<u64> {
        self.messages.get(index).copied()
    }

    /// True when a request built from `model_id`/`system`/`tools`/`messages`
    /// provably repeats the last sent request as a prefix — the provider
    /// would serve it from the prompt cache. Extra trailing entries are
    /// fine: they extend the cache rather than invalidating it.
    pub fn shared_prefix_ok(
        &self,
        model_id: &str,
        system: Option<&str>,
        tools: &[ToolDef],
        messages: &[Message],
    ) -> bool {
        self.provider == digest(model_id)
            && self.system == digest(&system.unwrap_or_default())
            && self.tools == digest(&tools)
            && messages.len() >= self.messages.len()
            && self
                .messages
                .iter()
                .zip(messages.iter())
                .all(|(stored, m)| *stored == digest(m))
    }
}

/// Digest of one message, exposed for re-deriving wire-only tail entries.
pub(crate) fn message_digest(m: &Message) -> u64 {
    digest(m)
}

pub fn fingerprint(model_id: &str, req: &ChatRequest) -> Fingerprint {
    Fingerprint {
        provider: digest(model_id),
        system: digest(req.system.as_deref().unwrap_or_default()),
        tools: digest(&req.tools),
        messages: req.messages.iter().map(digest).collect(),
    }
}

/// Plain-language cause for a cache-miss notice. `None` when the change sits at
/// the tail of the history (normal growth), which cannot cost a cache miss
/// worth explaining.
pub fn describe(change: Change, announced: Option<&str>, prev_len: usize) -> Option<String> {
    let known = |r: &str| match r {
        "media payload budget" => "older images dropped from context".to_string(),
        "stale output mask" => "old tool output trimmed".to_string(),
        "history rewrite" => "history rewritten".to_string(),
        other => other.to_string(),
    };
    Some(match change {
        Change::Provider => "model changed".to_string(),
        Change::System => "system prompt changed".to_string(),
        Change::Tools => "tool list changed".to_string(),
        Change::HistoryAt(k) => match announced {
            Some(r) => known(r),
            None if k + 1 >= prev_len => return None,
            None => format!("history changed at message {}", k + 1),
        },
        Change::HistoryTruncated { .. } => match announced {
            Some(r) => known(r),
            None => "history truncated".to_string(),
        },
    })
}

pub fn diff(prev: &Fingerprint, next: &Fingerprint) -> Option<Change> {
    if prev.provider != next.provider {
        return Some(Change::Provider);
    }
    if prev.system != next.system {
        return Some(Change::System);
    }
    if prev.tools != next.tools {
        return Some(Change::Tools);
    }
    if let Some(k) = prev
        .messages
        .iter()
        .zip(&next.messages)
        .position(|(a, b)| a != b)
    {
        return Some(Change::HistoryAt(k));
    }
    (next.messages.len() < prev.messages.len()).then_some(Change::HistoryTruncated {
        from: prev.messages.len(),
        to: next.messages.len(),
    })
}

/// Where each sent request's fingerprint is reported; the REPL persists it
/// beside the session file.
pub type FingerprintSink = Arc<dyn Fn(&Fingerprint) + Send + Sync>;

#[derive(Debug, Default)]
pub struct PrefixGuard {
    last: Option<Fingerprint>,
    expected: Option<&'static str>,
}

impl PrefixGuard {
    pub fn expect_change(&mut self, reason: &'static str) {
        self.expected = Some(reason);
    }

    pub fn last(&self) -> Option<&Fingerprint> {
        self.last.as_ref()
    }

    /// Seed from a saved session so the first request after a restart is
    /// compared with the last one sent before it.
    pub fn seed(&mut self, fp: Fingerprint) {
        if self.last.is_none() {
            self.last = Some(fp);
        }
    }

    pub fn observe(
        &mut self,
        model_id: &str,
        req: &ChatRequest,
    ) -> Option<(Change, Option<&'static str>)> {
        let next = fingerprint(model_id, req);
        let change = self.last.as_ref().and_then(|prev| diff(prev, &next));
        self.last = Some(next);
        let expected = self.expected.take();
        change.map(|c| (c, expected))
    }
}
