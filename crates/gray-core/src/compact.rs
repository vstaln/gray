use crate::agent::Agent;
use crate::error::CoreError;
use crate::message::{ContentBlock, Message, Role};

fn message_tokens(m: &Message) -> usize {
    if m.content
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. } | ContentBlock::Media { .. }))
    {
        m.content.iter().map(block_tokens).sum()
    } else {
        crate::agent_compact::est_tokens(std::slice::from_ref(m))
    }
}

pub(crate) fn block_tokens(b: &ContentBlock) -> usize {
    match b {
        ContentBlock::Text { text } => text.len() / 4,
        ContentBlock::Image { media_type, data } => image_block_tokens(media_type, data.len()),

        ContentBlock::Media {
            media_type, data, ..
        } => image_block_tokens(media_type, data.len()),
        ContentBlock::ToolResult { content, .. } => content.len() / 4,
        ContentBlock::StructuredInput {
            protocol,
            version,
            kind,
            payload,
        } => format!("{protocol}{version}{kind}{payload}").len() / 4,
        ContentBlock::ToolUse { name, args, .. } => format!("{name}{args}").len() / 4,
        ContentBlock::Thinking {
            text,
            encrypted_content,
            ..
        } => match encrypted_content {
            Some(blob) => (text.len() + blob.len()) / 4,
            None => text.len() / 4,
        },
    }
}

pub(crate) fn image_block_tokens(_media_type: &str, base64_len: usize) -> usize {
    (base64_len.saturating_mul(3) / 4 / 4).clamp(1_000, 4_096)
}

pub(crate) const SUMMARY_ENVELOPE_PREFIX: &str =
    "The conversation history before this point was compacted into the following summary:";

pub(crate) const COMPACTION_TRIGGER: &str = "Summarize this conversation so far for continuing it: outcomes, decisions, open questions. No tool calls.";

/// Appended by the turn loop when the wire request ends on an assistant
/// message; kept here so compaction can re-derive the wire-only tail for
/// prefix proofs.
pub(crate) const CONTINUE_TAIL: &str =
    "Continue the pending user request using the retained context and summary.";

fn trigger_message(instructions: Option<&str>) -> Message {
    let mut trigger = COMPACTION_TRIGGER.to_string();
    if let Some(instructions) = instructions.map(str::trim).filter(|s| !s.is_empty()) {
        trigger.push_str(&format!(
            "\n\n<user-instructions>\nThe user provided these instructions for this summary. Follow them with high priority:\n{instructions}\n</user-instructions>"
        ));
    }
    Message::user_injected(trigger)
}

pub(crate) async fn run_compaction_call(
    agent: &Agent,
    history: &[Message],
    instructions: Option<&str>,
) -> Result<String, CoreError> {
    let mut messages = history.to_vec();
    messages.push(trigger_message(instructions));

    let system = agent.system_text();
    agent
        .complete_isolated(
            (!system.is_empty()).then_some(system),
            messages,
            agent.request_tools(),
        )
        .await
}

/// The pi-cached-compaction trick: rebuild the summary call as the last
/// turn's wire messages plus the appended trigger, so the provider serves
/// almost all of it from the prompt cache it already holds. `history` must
/// be the un-elided wire view (`scrubbed_messages`). Returns None — caller
/// runs the normal cache-off path — unless the prefix guard proves every
/// cached byte is reproduced.
pub(crate) fn shared_prefix_compaction_messages(
    agent: &Agent,
    history: &[Message],
    instructions: Option<&str>,
) -> Option<Vec<Message>> {
    let fp = agent.prefix_guard.last()?;
    let mut messages = history.to_vec();
    // A turn that ended on an assistant message carried a synthetic continue
    // tail on the wire; re-derive it so the prefix can still match.
    if fp.message_count() == messages.len() + 1
        && fp.message_digest_at(messages.len())
            == Some(crate::prefix_guard::message_digest(&Message::user_injected(
                CONTINUE_TAIL,
            )))
    {
        messages.push(Message::user_injected(CONTINUE_TAIL));
    }
    // The request is only provably shared when the candidate covers every
    // message the provider cached; growth since then (the turn's reply) is
    // fine — it sits after the cached prefix.
    if messages.len() < fp.message_count() {
        return None;
    }
    messages.push(trigger_message(instructions));
    let system = agent.system_text();
    fp.shared_prefix_ok(
        agent.provider.model_id(),
        (!system.is_empty()).then_some(system),
        &agent.request_tools(),
        &messages,
    )
    .then_some(messages)
}

pub(crate) const RETAINED_MESSAGE_TOKEN_BUDGET: usize = 20_000;

pub(crate) const MAX_RETAINED_AGENT_MESSAGE_TOKENS: i64 = 10_000;

const TRUNCATION_MARKER: &str = "[…truncated…]";

pub(crate) const ARC_STUB_MIN_BYTES: usize = 8_192;

pub(crate) fn tool_result_stub(id: &str, bytes: usize, session_id: Option<&str>) -> String {
    let file = match session_id {
        Some(sid) => format!("~/.gray/sessions/{sid}.jsonl"),
        None => "this session's file under ~/.gray/sessions/".to_string(),
    };
    format!(
        "[tool output elided from context ({bytes} bytes). The full output survives in the session transcript; recover it with: grep '{id}' {file}]"
    )
}

fn has_stubbable_result(group: &[Message]) -> bool {
    group
        .iter()
        .flat_map(|m| &m.content)
        .any(|b| matches!(b, ContentBlock::ToolResult { content, .. } if content.len() >= ARC_STUB_MIN_BYTES))
}

fn stub_large_tool_results(group: &mut [Message], session_id: Option<&str>) {
    for m in group.iter_mut() {
        for b in m.content.iter_mut() {
            if let ContentBlock::ToolResult { id, content, .. } = b
                && content.len() >= ARC_STUB_MIN_BYTES
            {
                *content = tool_result_stub(id, content.len(), session_id);
            }
        }
    }
}

const RECENT_IMAGE_GROUPS: usize = 2;

const IMAGE_STUB_MIN_BYTES: usize = 8 * 1024;

fn media_bytes(b: &ContentBlock) -> Option<usize> {
    match b {
        ContentBlock::Image { data, .. } | ContentBlock::Media { data, .. } => Some(data.len()),
        _ => None,
    }
}

fn drop_media_block(b: &mut ContentBlock) {
    if let ContentBlock::Image { media_type, data }
    | ContentBlock::Media {
        media_type, data, ..
    } = b
    {
        let stub = format!(
            "[{media_type} elided from context ({} bytes) to stay under the provider's per-request media limit. View the source file again if you still need it]",
            data.len()
        );
        *b = ContentBlock::text(stub);
    }
}

fn stub_large_media(group: &mut [Message]) {
    for m in group.iter_mut() {
        for b in m.content.iter_mut() {
            let (media_type, data) = match b {
                ContentBlock::Image { media_type, data }
                | ContentBlock::Media {
                    media_type, data, ..
                } if data.len() >= IMAGE_STUB_MIN_BYTES => (media_type.clone(), data.clone()),
                _ => continue,
            };
            let bytes = data.len();
            *b = match crate::paths::materialize_media(&media_type, &data) {
                Some(path) => ContentBlock::text(format!(
                    "[{media_type} elided from context ({bytes} bytes). Saved to {}; read it to re-view]",
                    path.display()
                )),
                None => ContentBlock::text(format!(
                    "[{media_type} elided from context ({bytes} bytes). The original survives in the session transcript and source file]"
                )),
            };
        }
    }
}

pub(crate) fn stub_all_media(messages: &mut [Message]) -> usize {
    let mut stubbed = 0;
    for b in messages.iter_mut().flat_map(|m| m.content.iter_mut()) {
        if media_bytes(b).is_some() {
            drop_media_block(b);
            stubbed += 1;
        }
    }
    stubbed
}

// Providers cap inline media per request well below any token window
// (OpenRouter rejects >30MB of images with a 413; Anthropic allows 100
// images). Images are priced at ≤4k tokens each, so token-driven compaction
// never fires first — a screenshot-heavy session hit the cap at 27% of a 1M
// window. Over the max, everything older than the newest `KEEP` worth is
// stubbed; the gap to max means the prefix only changes once per ~10MB.
// Stubs write nothing to disk: the pixels already live in the source file
// and the session transcript.
pub(crate) const MEDIA_PAYLOAD_MAX_BYTES: usize = 20_000_000;
pub(crate) const MEDIA_PAYLOAD_KEEP_BYTES: usize = 10_000_000;
pub(crate) const MEDIA_PAYLOAD_MAX_COUNT: usize = 80;
pub(crate) const MEDIA_PAYLOAD_KEEP_COUNT: usize = 40;

pub(crate) fn media_over_budget(messages: &[Message]) -> bool {
    let (count, bytes) = messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(media_bytes)
        .fold((0, 0), |(count, bytes), n| (count + 1, bytes + n));
    count > MEDIA_PAYLOAD_MAX_COUNT || bytes > MEDIA_PAYLOAD_MAX_BYTES
}

pub(crate) fn elide_media_over_payload_budget(messages: &mut [Message]) -> usize {
    if !media_over_budget(messages) {
        return 0;
    }
    let (mut kept, mut kept_bytes, mut elided) = (0, 0, 0);
    let mut full = false;
    for b in messages
        .iter_mut()
        .rev()
        .flat_map(|m| m.content.iter_mut().rev())
    {
        let Some(n) = media_bytes(b) else { continue };
        full =
            full || kept == MEDIA_PAYLOAD_KEEP_COUNT || kept_bytes + n > MEDIA_PAYLOAD_KEEP_BYTES;
        if full {
            drop_media_block(b);
            elided += 1;
        } else {
            kept += 1;
            kept_bytes += n;
        }
    }
    elided
}

#[cfg(test)]
pub(crate) fn build_retained(messages: &[Message], budget: usize) -> Vec<Message> {
    build_retained_with_session(messages, budget, None)
}

pub(crate) fn build_retained_with_session(
    messages: &[Message],
    budget: usize,
    session_id: Option<&str>,
) -> Vec<Message> {
    let mut kept_reversed: Vec<Vec<Message>> = Vec::new();
    let mut remaining = budget;
    for group in atomic_groups(messages).into_iter().rev() {
        if remaining == 0 {
            continue;
        }

        let mut stubbed_group: Vec<Message>;
        let group: &[Message] = if group_is_retained(group) {
            group
        } else if group.iter().any(is_tool_use_only) && has_stubbable_result(group) {
            stubbed_group = group.to_vec();
            stub_large_tool_results(&mut stubbed_group, session_id);
            &stubbed_group
        } else {
            let cap = remaining.min(MAX_RETAINED_AGENT_MESSAGE_TOKENS as usize);
            if let Some(truncated) = truncate_group_to_budget(group, cap, session_id) {
                remaining = remaining.saturating_sub(group_tokens(&truncated).max(1));
                kept_reversed.push(truncated);
            }
            continue;
        };
        let cost = group_tokens(group).max(1);
        if cost <= remaining {
            remaining -= cost;
            kept_reversed.push(group.to_vec());
        } else if let Some(truncated) = truncate_group_to_budget(group, remaining, session_id) {
            kept_reversed.push(truncated);
            remaining = 0;
        }
    }

    for group in kept_reversed.iter_mut().skip(RECENT_IMAGE_GROUPS) {
        stub_large_media(group);
    }
    kept_reversed.into_iter().rev().flatten().collect()
}

pub(crate) const SOFT_ELIDE_RATIO: f64 = 0.6;

pub(crate) const SOFT_ELIDE_KEEP_RATIO: f64 = 0.3;

pub(crate) const SOFT_ELIDE_MIN_RECENT_GROUPS: usize = 2;

pub(crate) const SOFT_ELIDE_REARM_RATIO: f64 = 0.1;

pub(crate) fn soft_elide_threshold(window: usize) -> usize {
    (window as f64 * SOFT_ELIDE_RATIO) as usize
}

pub(crate) fn soft_elide_rearm_step(window: usize) -> usize {
    (window as f64 * SOFT_ELIDE_REARM_RATIO) as usize
}

pub(crate) fn soft_elide_keep_budget(window: usize) -> usize {
    (window as f64 * SOFT_ELIDE_KEEP_RATIO) as usize
}

pub fn price_cliff_ceiling(cliff: usize, reserve: usize) -> Option<usize> {
    cliff
        .checked_sub((cliff / 10).max(reserve))
        .filter(|ceiling| *ceiling > 0)
}

pub(crate) const PRICE_CLIFF_KEEP_DIVISOR: usize = 4;

pub(crate) fn elide_stale_tool_results(
    messages: &mut [Message],
    keep_budget: usize,
    session_id: Option<&str>,
) -> usize {
    let boundary = {
        let groups = atomic_groups(messages);
        let mut remaining = keep_budget;
        let mut boundary = messages.len();
        for (kept, group) in groups.iter().rev().enumerate() {
            let cost = group_tokens(group).max(1);
            if kept >= SOFT_ELIDE_MIN_RECENT_GROUPS && cost > remaining {
                break;
            }
            remaining = remaining.saturating_sub(cost);
            boundary -= group.len();
        }
        boundary
    };
    let mut elided = 0;
    for m in messages[..boundary].iter_mut() {
        for b in m.content.iter_mut() {
            if let ContentBlock::ToolResult { id, content, .. } = b
                && content.len() >= ARC_STUB_MIN_BYTES
            {
                *content = tool_result_stub(id, content.len(), session_id);
                elided += 1;
            }
        }
    }
    elided
}

pub(crate) fn atomic_groups(messages: &[Message]) -> Vec<&[Message]> {
    let mut groups = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        if messages[i].role == Role::Assistant && tool_use_ids(&messages[i]).next().is_some() {
            let mut j = i + 1;
            while j < messages.len()
                && messages[j].role == Role::User
                && tool_ids_intersect(&messages[i], &messages[j])
            {
                j += 1;
            }
            if j > i + 1 {
                groups.push(&messages[i..j]);
                i = j;
                continue;
            }
        }
        groups.push(&messages[i..i + 1]);
        i += 1;
    }
    groups
}

fn tool_use_ids(m: &Message) -> impl Iterator<Item = &str> {
    m.content.iter().filter_map(|b| match b {
        ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
        _ => None,
    })
}

fn tool_ids_intersect(assistant: &Message, user: &Message) -> bool {
    tool_use_ids(assistant).any(|id| {
        user.content.iter().any(|b| match b {
            ContentBlock::ToolResult { id: result_id, .. } => result_id == id,
            _ => false,
        })
    })
}

fn has_text(m: &Message) -> bool {
    m.content.iter().any(|b| match b {
        ContentBlock::Text { text } => !text.is_empty(),
        _ => false,
    })
}

fn is_tool_use_only(m: &Message) -> bool {
    m.role == Role::Assistant && tool_use_ids(m).next().is_some() && !has_text(m)
}

fn group_tokens(group: &[Message]) -> usize {
    group.iter().map(message_tokens).sum()
}

fn group_is_retained(group: &[Message]) -> bool {
    if group.iter().any(is_tool_use_only) {
        return false;
    }
    if group
        .iter()
        .any(|m| matches!(m.role, Role::User | Role::System))
    {
        return true;
    }
    i64::try_from(group_tokens(group)).unwrap_or(i64::MAX) <= MAX_RETAINED_AGENT_MESSAGE_TOKENS
}

fn truncate_group_to_budget(
    group: &[Message],
    budget: usize,
    session_id: Option<&str>,
) -> Option<Vec<Message>> {
    if let Some(fitted) = fit_group_to_budget(group, budget) {
        return Some(fitted);
    }
    if has_stubbable_result(group) {
        let mut stubbed = group.to_vec();
        stub_large_tool_results(&mut stubbed, session_id);
        return fit_group_to_budget(&stubbed, budget);
    }
    None
}

fn fit_group_to_budget(group: &[Message], budget: usize) -> Option<Vec<Message>> {
    let fixed: usize = group
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|b| match b {
            ContentBlock::Text { .. } | ContentBlock::Image { .. } | ContentBlock::Media { .. } => {
                0
            }
            ContentBlock::Thinking {
                encrypted_content, ..
            } => encrypted_content.as_deref().map_or(0, |e| e.len() / 4),
            _ => block_tokens(b),
        })
        .sum();
    if fixed > budget {
        return None;
    }
    let mut out = group.to_vec();
    let mut remaining = budget - fixed;
    let mut retained_billable = false;
    for msg in out.iter_mut().rev() {
        for block in msg.content.iter_mut().rev() {
            match block {
                ContentBlock::Image { media_type, data }
                | ContentBlock::Media {
                    media_type, data, ..
                } => {
                    let price = image_block_tokens(media_type, data.len());
                    if price <= remaining {
                        remaining -= price;
                        retained_billable = true;
                    } else {
                        data.clear();
                    }
                }
                ContentBlock::Text { text } | ContentBlock::Thinking { text, .. } => {
                    if text.is_empty() {
                        continue;
                    }
                    if remaining == 0 {
                        text.clear();
                    } else if text.len() / 4 <= remaining {
                        remaining -= text.len() / 4;
                        retained_billable = true;
                    } else {
                        *text = truncate_text_to_budget(text, remaining);
                        remaining = 0;
                        retained_billable = true;
                    }
                }
                _ => {}
            }
        }
        msg.content.retain(|b| match b {
            ContentBlock::Text { text } => !text.is_empty(),
            ContentBlock::Thinking {
                text,
                encrypted_content,
                item_id,
                ..
            } => !text.is_empty() || encrypted_content.is_some() || item_id.is_some(),
            ContentBlock::Image { data, .. } | ContentBlock::Media { data, .. } => !data.is_empty(),
            _ => true,
        });
    }
    retained_billable.then_some(out)
}

fn truncate_text_to_budget(text: &str, max_tokens: usize) -> String {
    if text.len() / 4 <= max_tokens {
        return text.to_string();
    }
    let usable = max_tokens
        .saturating_mul(4)
        .saturating_sub(TRUNCATION_MARKER.len());
    let (head, tail) = split_head_tail(text, usable);
    format!("{head}{TRUNCATION_MARKER}{tail}")
}

fn split_head_tail(s: &str, usable: usize) -> (&str, &str) {
    let right = usable - usable / 2;
    let head_end = s.floor_char_boundary((usable / 2).min(s.len()));
    let tail_start = s.ceil_char_boundary(s.len().saturating_sub(right));
    let tail_start = tail_start.max(head_end);
    (&s[..head_end], &s[tail_start..])
}

#[path = "compact_mod_tests.rs"]
#[cfg(test)]
mod tests;

pub(crate) fn anchor_message(messages: &[Message]) -> Option<Message> {
    let first = messages.first()?;
    if first.role != Role::User {
        return None;
    }

    let mut anchor = first.clone();
    stub_large_media(std::slice::from_mut(&mut anchor));
    Some(anchor)
}
