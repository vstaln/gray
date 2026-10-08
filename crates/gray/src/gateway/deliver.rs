//! What happens to a turn's reply: silence tokens, then the outbox.

use std::path::Path;

use super::event::Kind;
use super::outbox::Intent;
use super::settings::Settings;
use crate::cron::store::Origin as Route;

/// The silence tokens. A reply that is (or, for autonomous turns, starts or
/// ends with) one of these is not delivered. `HEARTBEAT_OK` is OpenClaw's
/// convention, which models reach for unprompted.
pub const SILENT_TOKENS: [&str; 3] = ["NO_REPLY", "[SILENT]", "HEARTBEAT_OK"];

/// Dual strictness (Hermes): a user turn is silent only when the whole reply
/// is exactly a token; an autonomous turn when its first or last non-empty
/// line is a token. Empty text is silent for both.
pub fn is_silent(text: &str, kind: Kind) -> bool {
    if text.trim().is_empty() {
        return true;
    }
    if !kind.autonomous() {
        return SILENT_TOKENS.contains(&text.trim());
    }
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    lines.next().is_some_and(is_token_line) || lines.next_back().is_some_and(is_token_line)
}

/// A bare silence token, forgiving of the markdown emphasis autonomous
/// replies tend to wrap it in (`**NO_REPLY**`, `` `[SILENT]` ``).
fn is_token_line(line: &str) -> bool {
    let bare = line.trim_matches(|c| c == '*' || c == '`' || c == ' ' || c == '\t');
    SILENT_TOKENS.contains(&bare)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Enqueued for its platform.
    Deliver(Intent),
    /// Not delivered; the reason goes to the activity log.
    Suppressed(&'static str),
}

/// Decide and act: silent → `Suppressed("silent")` (logged); otherwise
/// enqueue for `route`, falling back to the session's `last_route`, then
/// `settings.owners[0]`, then the `local` inbox.
// ponytail: no quiet hours / hourly cap; add when heartbeat output gets noisy.
pub fn decide(
    dir: &Path,
    settings: &Settings,
    key: &str,
    kind: Kind,
    route: Option<Route>,
    last_route: Option<Route>,
    text: &str,
) -> Decision {
    if is_silent(text, kind) {
        suppressed(dir, key, kind, "silent", text);
        return Decision::Suppressed("silent");
    }
    let route = route
        .or(last_route)
        .or_else(|| settings.owners.first().cloned());
    let intent = Intent::new(key, kind, route, text);
    if let Err(e) = super::outbox::enqueue(dir, &intent) {
        log::warn!("gateway: cannot enqueue delivery for {key}: {e:#}");
        super::activity::log(
            dir,
            "deliver_failed",
            serde_json::json!({"key": key, "kind": kind.as_str(), "error": format!("{e:#}")}),
        );
        return Decision::Suppressed("enqueue failed");
    }
    super::activity::log(
        dir,
        "queued",
        serde_json::json!({"key": key, "kind": kind.as_str(), "platform": &intent.platform,
            "text": text.chars().take(300).collect::<String>()}),
    );
    Decision::Deliver(intent)
}

/// A reply that goes nowhere, with the reason a `gateway activity` tail
/// can explain.
fn suppressed(dir: &Path, key: &str, kind: Kind, reason: &'static str, text: &str) {
    super::activity::log(
        dir,
        "suppressed",
        serde_json::json!({"key": key, "kind": kind.as_str(), "reason": reason,
            "text": text.chars().take(200).collect::<String>()}),
    );
}

#[path = "deliver_tests.rs"]
#[cfg(test)]
mod tests;
