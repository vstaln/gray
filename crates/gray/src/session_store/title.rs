use super::*;

pub(super) fn read_dismissal(root: &Path, id: &str) -> Option<u64> {
    std::fs::read_to_string(root.join(format!("{id}.dismissed")))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn write_dismissal(root: &Path, id: &SessionId, last_message_at: u64) -> std::io::Result<()> {
    if !valid_session_id(id.as_str()) {
        return Err(std::io::Error::other("invalid session id"));
    }
    std::fs::write(
        root.join(format!("{}.dismissed", id.as_str())),
        last_message_at.to_string(),
    )
}

pub(super) const MAX_TITLE_CHARS: usize = 80;

pub(crate) fn clean_title(raw: &str) -> Option<String> {
    let one: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = one.chars().take(MAX_TITLE_CHARS).collect();
    (!t.is_empty()).then_some(t)
}

pub(super) fn read_title(root: &Path, id: &str) -> Option<String> {
    clean_title(&std::fs::read_to_string(root.join(format!("{id}.title"))).ok()?)
}

pub(crate) fn opener_tag(first: &str) -> Option<&'static str> {
    let first = first.trim_start();
    let head: String = first.chars().take(160).collect();
    if first.starts_with(crate::composer::transcript::BG_NOTICE_MARKER) {
        Some("background")
    } else if head.contains("subagent of a parent Gray") {
        Some("subagent")
    } else if head.starts_with("You are") && head.contains(" worker") {
        Some("worker")
    } else {
        None
    }
}

pub fn claim_auto_title(root: &Path, id: &SessionId) -> bool {
    if !valid_session_id(id.as_str()) || read_title(root, id.as_str()).is_some() {
        return false;
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(format!("{}.autotitle", id.as_str())))
        .is_ok()
}

pub fn set_auto_title(root: &Path, id: &SessionId, proposed: &str) -> std::io::Result<bool> {
    use std::io::Write as _;
    if !valid_session_id(id.as_str()) {
        return Err(std::io::Error::other("invalid session id"));
    }
    let Some(title) = clean_title(proposed) else {
        return Ok(false);
    };
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(root.join(format!("{}.title", id.as_str())))
    {
        Ok(mut file) => {
            file.write_all(title.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn write_title(root: &Path, id: &SessionId, title: &str) -> std::io::Result<Option<String>> {
    if !valid_session_id(id.as_str()) {
        return Err(std::io::Error::other("invalid session id"));
    }
    let path = root.join(format!("{}.title", id.as_str()));
    match clean_title(title) {
        Some(t) => {
            std::fs::write(&path, &t)?;
            Ok(Some(t))
        }
        None => {
            match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            Ok(None)
        }
    }
}
