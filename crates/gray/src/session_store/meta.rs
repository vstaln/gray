use super::*;

/// What survives a history rewrite: a tool round by its call id (soft
/// elision rewrites the result body, not the id), anything else by content.
pub(super) fn carry_key(m: &Message) -> Option<String> {
    let id = m.content.iter().find_map(|b| match b {
        ContentBlock::ToolUse { id, .. } | ContentBlock::ToolResult { id, .. } => Some(id),
        _ => None,
    });
    match id {
        Some(id) => Some(format!("{:?}:{id}", m.role)),
        None => serde_json::to_string(m).ok(),
    }
}

/// Timestamp/usage/duration of the active entries (after the last
/// boundary), so a replacement re-states kept messages as they were rather
/// than stamping the whole transcript with the rewrite time.
pub(super) fn carried_meta(
    text: &str,
) -> std::collections::HashMap<String, std::collections::VecDeque<CarriedMeta>> {
    let entries: Vec<SessionEntry> = text
        .lines()
        .skip(1)
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let start = entries
        .iter()
        .rposition(|e| e.compaction_boundary)
        .map_or(0, |i| i + 1);
    let mut map: std::collections::HashMap<_, std::collections::VecDeque<_>> = Default::default();
    for e in &entries[start..] {
        if let Some(k) = carry_key(&e.message) {
            map.entry(k)
                .or_default()
                .push_back((e.timestamp, e.usage, e.duration_ms));
        }
    }
    map
}

impl SessionMeta {
    pub fn new(
        id: SessionId,
        timestamp: u64,
        cwd: impl Into<PathBuf>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            id,
            timestamp,
            cwd: cwd.into(),
            model: model.into(),
            origin: None,
        }
    }

    pub fn with_origin(mut self, origin: Option<String>) -> Self {
        self.origin = origin;
        self
    }
}
