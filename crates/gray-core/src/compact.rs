use crate::agent::Agent;
use crate::error::CoreError;
use crate::message::{ContentBlock, Message, Role};

/// One message's token estimate: image-aware budgeting over the shared
/// `agent_compact::est_tokens` owner (bytes/4 over billable text). Messages
/// without images delegate verbatim so text/tool/thinking estimates can never
/// drift; messages with images price each `Image` block via
/// [`image_block_tokens`] (base64 at bytes/4 would overcharge ~5× — the
/// payload is base64, not raw bytes). Block separators (`\n` in
/// `context_text`) are sub-token and ignored on the image path.
fn message_tokens(m: &Message) -> usize {
    if m.content
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }))
    {
        m.content.iter().map(block_tokens).sum()
    } else {
        crate::agent_compact::est_tokens(std::slice::from_ref(m))
    }
}

/// One block's token estimate on the image path: billable bytes/4 exactly
/// like `context_text`, except `Image` which uses [`image_block_tokens`].
pub(crate) fn block_tokens(b: &ContentBlock) -> usize {
    match b {
        ContentBlock::Text { text } => text.len() / 4,
        ContentBlock::Image { media_type, data } => image_block_tokens(media_type, data.len()),
        ContentBlock::ToolResult { content, .. } => content.len() / 4,
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

/// Token price of one `Image` block from its base64 length.
///
/// Bounded heuristic for normalized images (maximum side 2000px in gray-tools).
/// Encoded file length is not a vision token count: cap at 4096 to avoid
/// treating a several-MB screenshot as hundreds of thousands of text tokens.
/// Provider-reported usage remains authoritative after the request.
pub(crate) fn image_block_tokens(_media_type: &str, base64_len: usize) -> usize {
    (base64_len.saturating_mul(3) / 4 / 4).clamp(1_000, 4_096)
}

/// In-band compaction trigger: chat-shaped equivalent of codex v2's
/// `ResponseItem::CompactionTrigger` + summary instruction, appended to the
/// retained history for the summarization call only.
//
// Task-neutral wording, no domain (coding or otherwise) assumed: keep
// outcomes/decisions/open questions, omit superseded detail, and by default
// the summary adds no domain instructions.
// guidance shape from @howaboua/pi-auto-trees (MIT)
pub(crate) const COMPACTION_TRIGGER: &str = "Compact this conversation for context compaction: reply with ONLY a concise summary keeping the outcomes, decisions and open questions that matter for continuing this conversation; omit superseded detail. Default adds no domain instructions. No tool calls.";

/// One in-band summarization call over `history`: same system prompt + same
/// tools as live turns, plus [`COMPACTION_TRIGGER`]. Identical request prefix
/// → server prefix-cache hit on providers that support it. Custom
/// `instructions` (manual `/compact <text>`) ride the trigger with high
/// priority. Takes `&[Message]` and builds the input locally — the caller's
/// history is never mutated — and the trigger is popped from retained history
/// after (mirroring v2's `prompt_input.pop()`), so it never pollutes the
/// transcript.
pub(crate) async fn run_compaction_call(
    agent: &Agent,
    history: &[Message],
    instructions: Option<&str>,
) -> Result<String, CoreError> {
    let mut messages = history.to_vec();
    // Checkpoint-structure guidance rides the trigger so the summary
    // captures IDs/decisions/next-steps (shape from pi-codex-conversion).
    let mut trigger = format!("{COMPACTION_TRIGGER}\n\n{COMPACTION_MARKER_GUIDANCE}");
    if let Some(instructions) = instructions.map(str::trim).filter(|s| !s.is_empty()) {
        trigger.push_str(&format!(
            "\n\n<user-instructions>\nThe user provided these instructions for this summary. Follow them with high priority:\n{instructions}\n</user-instructions>"
        ));
    }
    messages.push(Message::user(trigger));
    // Reuse the captured live prefix, including hook context, without re-fetching it.
    // Before the first turn, system_text() falls back to the configured base prompt.
    let system = agent.system_text();
    agent
        .complete_with_history(
            (!system.is_empty()).then_some(system),
            messages,
            agent.tool_defs().to_vec(),
        )
        .await
}

/// Compaction-boundary recovery guidance: what the checkpoint must capture
/// and how the post-compaction turn resumes. Tool-agnostic on purpose — core
/// owns no notes/history tools, so it names the checkpoint, not the tool.
//
// guidance shape from @howaboua/pi-codex-conversion (MIT)
pub(crate) const COMPACTION_MARKER_GUIDANCE: &str = "Checkpoint the active request, checkpoint/session IDs, decisions, progress, learnings and next steps at the compaction boundary. After compaction, read the hinted checkpoint first and resume; consult history only for a missing detail.";

/// Retained-history token budget, mirroring codex v2's
/// `RETAINED_MESSAGE_TOKEN_BUDGET` verbatim.
pub(crate) const RETAINED_MESSAGE_TOKEN_BUDGET: usize = 64_000;
/// Per-group cap for retained assistant-only groups, mirroring codex v2's
/// `MAX_RETAINED_AGENT_MESSAGE_TOKENS`. `i64` kept verbatim — converted at use.
/// Single assistant messages over this are progress/completion chatter.
pub(crate) const MAX_RETAINED_AGENT_MESSAGE_TOKENS: i64 = 10_000;
/// Boundary-truncation marker, inlined between the kept head+tail halves. Gray
/// has no message marker convention; documented here. Marker bytes come out of
/// the token budget so truncated output still estimates within budget.
const TRUNCATION_MARKER: &str = "[…truncated…]";

/// Build the retained history: group atomically → retention filter →
/// newest-first budget walk → chronological order. Mirrors codex v2's
/// `build_v2_compacted_history` minus the summary append (Task 5); images are
/// charged atomically newest-first via [`image_block_tokens`].
pub(crate) fn build_retained(messages: &[Message], budget: usize) -> Vec<Message> {
    let mut kept_reversed: Vec<Vec<Message>> = Vec::new();
    let mut remaining = budget;
    for group in atomic_groups(messages)
        .into_iter()
        .filter(|g| group_is_retained(g))
        .rev()
    {
        if remaining == 0 {
            continue;
        }
        let cost = group_tokens(group).max(1);
        if cost <= remaining {
            remaining -= cost;
            kept_reversed.push(group.to_vec());
        } else if let Some(truncated) = truncate_group_to_budget(group, remaining) {
            kept_reversed.push(truncated);
            remaining = 0;
        }
        // Untruncatable boundary group (no retainable text/image): dropped,
        // walk continues with `remaining` untouched.
    }
    kept_reversed.into_iter().rev().flatten().collect()
}

/// Split chronological `messages` into atomic retain/drop groups: an assistant
/// message's `ToolUse` ids fuse with ALL immediately-following user messages
/// that share tool ids (the full batch — gray's loop pushes one `ToolResult`
/// user message per call, so a 3-call batch is 1 assistant + 3 user
/// messages); unmatched strays are singleton groups. Batches therefore
/// retain/drop atomically — never an orphaned call or result.
fn atomic_groups(messages: &[Message]) -> Vec<&[Message]> {
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

/// Assistant message with ≥1 `ToolUse` and zero text: v2's
/// descendant-progress/completion chatter equivalent.
fn is_tool_use_only(m: &Message) -> bool {
    m.role == Role::Assistant && tool_use_ids(m).next().is_some() && !has_text(m)
}

fn group_tokens(group: &[Message]) -> usize {
    group.iter().map(message_tokens).sum()
}

/// Retention filter per group, mirroring v2's
/// `is_retained_for_remote_compaction_v2`: `ToolUse`-only chatter drops
/// together with its paired results; user/system groups are always retained
/// (the budget walk sizes them); text-carrying assistant-only groups survive
/// iff within the per-group cap.
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

/// Middle-truncate a group's `Text` blocks to `budget` tokens while charging
/// `Image` blocks atomically, newest-first like codex v2's
/// `truncate_message_to_token_budget` (fits → keep; first overflow → head+tail
/// truncate; later text dropped). An image that fits is kept whole and charged;
/// one that doesn't is dropped whole — base64 is never split (mirrors codex's
/// "images are handled atomically"). Fixed-cost blocks (`Thinking`,
/// `ToolUse` args, `ToolResult` bodies) are billed by the group pricer but
/// are not truncatable here, so they are charged first; a group whose fixed
/// costs alone exceed `budget` is dropped whole (caller preserves `remaining`
/// for older groups). The 16k `COMPACT_RESERVE_TOKENS` held back in
/// `try_compact_budgeted` covers the summary/system/tools overhead outside
/// this walk. Returns `None` when nothing retainable survives (no text or image
/// kept — e.g. an image-only group priced out of budget); the caller then
/// drops the whole group with `remaining` untouched. `None` also when the
/// group holds no text or image at all.
fn truncate_group_to_budget(group: &[Message], budget: usize) -> Option<Vec<Message>> {
    let fixed: usize = group
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|b| match b {
            ContentBlock::Text { .. } | ContentBlock::Image { .. } => 0,
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
                ContentBlock::Image { media_type, data } => {
                    let price = image_block_tokens(media_type, data.len());
                    if price <= remaining {
                        remaining -= price;
                        retained_billable = true;
                    } else {
                        data.clear(); // atomic drop; `retain` below removes it
                    }
                }
                ContentBlock::Text { text } => {
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
            ContentBlock::Image { data, .. } => !data.is_empty(),
            _ => true,
        });
    }
    retained_billable.then_some(out)
}

/// Middle-truncate `text` to `max_tokens`: keep head+tail halves with
/// [`TRUNCATION_MARKER`] inline. Already-small text returns untouched (no
/// marker). Splits on char boundaries.
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
