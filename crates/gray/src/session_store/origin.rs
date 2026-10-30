use super::*;

pub(super) fn sanitize_origin_tag(raw: &str) -> Option<String> {
    let t = raw.trim().to_ascii_lowercase();
    (!t.is_empty()
        && t.len() <= 32
        && t.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'))
    .then_some(t)
}

pub(super) static INHERITED_SESSION: std::sync::OnceLock<Option<String>> =
    std::sync::OnceLock::new();

pub(super) static RESTART_PARENT_PID: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();

pub fn capture_inherited_session_env() {
    INHERITED_SESSION.get_or_init(|| {
        std::env::var("GRAY_SESSION_ID")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    });
    RESTART_PARENT_PID.get_or_init(|| {
        std::env::var(RESTART_PARENT_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|&p| p != std::process::id())
    });
}

pub fn session_origin_from_env() -> Option<String> {
    if let Some(v) = std::env::var("GRAY_SESSION_ORIGIN")
        .ok()
        .and_then(|s| sanitize_origin_tag(&s))
    {
        return Some(v);
    }
    if std::env::var("GRAY_SUBAGENTS_ACTIVE").as_deref() == Ok("1") {
        return Some("subagent".to_string());
    }
    if INHERITED_SESSION.get().and_then(Option::as_deref).is_some() {
        return Some("nested".to_string());
    }
    None
}

pub fn print_session_origin() -> Option<String> {
    session_origin_from_env().or_else(|| Some("print".to_string()))
}
