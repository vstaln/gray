# Hermes Progress Bubbles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace gray's streaming edit-in-place answer (one message re-edited with growing text + `▍`) with Hermes-style progress bubbles on all edit-capable platforms (Discord, Telegram, Slack): tool lines accumulate in one bubble, the bubble is deleted when the turn ends, and the final answer is delivered as fresh message(s).

**Architecture:** New pure state machine `progress.rs` (line composition, `(×N)` dedup, overflow split — no IO, fully unit-tested) plus a `ProgressBubble` IO task in `daemon.rs` replacing `Streamer`. New `delete_message` capability on the adapter trait with real implementations per platform. Hermes reference: `/home/vstaln/hermes-rs/crates/hermes-gateway/src/turn.rs` (`on_tool_event` gate order, `drain_progress_messages` pump, `cleanup_msg_ids`).

**Tech Stack:** Rust, tokio (existing `UnboundedSender` task pattern), twilight 0.16 (`discord` feature), teloxide (`telegram` feature), slack-morphism (`slack` feature). No new dependencies.

**Spec:** User decisions 2026-09-05: scope = all edit-capable platforms; bubble deleted after turn; no live answer text while working (final answer only). Typing loop stays as-is (already the Discord analog of Hermes `set_status_text`).

## Global Constraints

- Workspace builds offline: real deps stay behind existing features (`telegram`, `discord`, `slack`, `all-platforms`); `cargo test -p gray-gateway` must pass with NO features enabled.
- Follow the existing per-file `#[cfg(feature)]` / `#[cfg(not(feature))]` stub pattern (log + succeed in stubs, mirroring `edit_message` stubs).
- No new config flags: `config.streaming` keeps its meaning ("live chat activity while working"); bubbles are always deleted (Hermes `cleanup_progress=true` behavior, hardcoded).
- Repo flow: feature branch + PR against `main` (`main` is protected, `test` check required). Never push to `main` directly.

---

## File map

- Create: `crates/gray-gateway/src/progress.rs` — pure progress-line state machine (Hermes `drain_progress_messages` core minus IO).
- Modify: `crates/gray-gateway/src/lib.rs` — add `pub mod progress;` (file lists modules alphabetically: after `platform`, before `session`).
- Modify: `crates/gray-gateway/src/platform.rs` — add `delete_message` default method to `BasePlatformAdapter`.
- Modify: `crates/gray-gateway/src/discord.rs` — implement `delete_message` via twilight.
- Modify: `crates/gray-gateway/src/telegram.rs` — implement `delete_message` via teloxide.
- Modify: `crates/gray-gateway/src/slack.rs` — implement `delete_message` via slack-morphism `chat_delete`.
- Modify: `crates/gray-gateway/src/daemon.rs` — replace `StreamMsg`/`Streamer`/`finalize_stream` with `ProgressMsg`/`ProgressBubble`; rewire `run_agent` `on_event`; change finish path to delete-then-`reply()`; replace `finalize_stream_chunks_after_edit` test.

---

### Task 1: Pure progress state machine

**Files:**
- Create: `crates/gray-gateway/src/progress.rs`
- Modify: `crates/gray-gateway/src/lib.rs` (add `pub mod progress;` between `platform` and `session`)
- Test: unit tests inside `progress.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing (pure; takes `&str` / `&serde_json::Value`, both already in the dependency tree).
- Produces (used by Task 4): `tool_start_line(name: &str) -> String`, `tool_end_line(name: &str, args: &serde_json::Value) -> String`, `struct ProgressLines` with `new()`, `push_start(name)`, `push_end(name, args)`, `text() -> String`, `split_groups(max_utf16: usize) -> Vec<String>`.

Behavior (ported from Hermes `turn.rs` `on_tool_event` #12 + `drain_progress_messages` `split_overflow`):
- Start line: `⏳ {name}…`.
- End line: if args is null/empty object → `🔧 {name}…`; else compact JSON (`serde_json::to_string`, fallback `"…"`) truncated to 80 chars by char boundary → `🔧 {name}: "{preview}"`.
- `push_end` replaces the last line ONLY if it is exactly the `⏳` start line for the same tool name; otherwise pushes a new line.
- `push_start`/`push_end` dedup: if the composed line equals the last line, rewrite it as `{line} (×{n})` with n starting at 2 and incrementing while identical lines repeat (Hermes `ProgressMsg::Dedup` renders `{text} (×{count+1})`).
- `text()` joins lines with `\n`.
- `split_groups(max_utf16)`: Hermes `split_overflow` — walk lines, accumulate into groups; a line starts a new group when adding it would push the joined group text over `max_utf16` (length measured in UTF-16 code units like the rest of the gateway). Returns each group joined with `\n`. A single overlong line forms its own group (no truncation — send path failures are best-effort, same as today).

- [ ] **Step 1: Write the failing tests.** Create `crates/gray-gateway/src/progress.rs` containing ONLY the test module below (no implementation yet), plus the `pub mod progress;` line in `lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn start_line_format() {
        assert_eq!(tool_start_line("terminal"), "⏳ terminal…");
    }

    #[test]
    fn end_line_without_args() {
        assert_eq!(tool_end_line("read", &json!(null)), "🔧 read…");
        assert_eq!(tool_end_line("read", &json!({})), "🔧 read…");
    }

    #[test]
    fn end_line_with_args_preview() {
        let l = tool_end_line("terminal", &json!({"command": "ls -la"}));
        assert_eq!(l, "🔧 terminal: \"{\"command\":\"ls -la\"}\"");
    }

    #[test]
    fn end_line_truncates_long_args_to_80_chars() {
        let big = "x".repeat(200);
        let l = tool_end_line("exec", &json!({"command": big}));
        assert!(l.chars().count() <= "🔧 exec: \"\"".chars().count() + 80);
    }

    #[test]
    fn end_replaces_matching_start_line() {
        let mut p = ProgressLines::new();
        p.push_start("terminal");
        p.push_end("terminal", &json!({"command": "ls"}));
        assert_eq!(p.text(), "🔧 terminal: \"{\"command\":\"ls\"}\"");
    }

    #[test]
    fn end_pushes_when_last_line_is_different_tool() {
        let mut p = ProgressLines::new();
        p.push_start("read");
        p.push_end("terminal", &json!(null));
        assert_eq!(p.text(), "⏳ read…\n🔧 terminal…");
    }

    #[test]
    fn consecutive_identical_lines_dedup_with_count() {
        let mut p = ProgressLines::new();
        p.push_start("execute_code");
        p.push_start("execute_code");
        p.push_start("execute_code");
        assert_eq!(p.text(), "⏳ execute_code… (×3)");
    }

    #[test]
    fn dedup_resets_after_different_line() {
        let mut p = ProgressLines::new();
        p.push_start("a");
        p.push_start("a");
        p.push_start("b");
        p.push_start("b");
        assert_eq!(p.text(), "⏳ a… (×2)\n⏳ b… (×2)");
    }

    #[test]
    fn split_groups_keeps_short_lines_together() {
        let mut p = ProgressLines::new();
        p.push_start("a");
        p.push_start("b");
        assert_eq!(p.split_groups(2000), vec!["⏳ a…\n⏳ b…".to_string()]);
    }

    #[test]
    fn split_groups_rolls_overflow_into_new_group() {
        let mut p = ProgressLines::new();
        p.push_start("aaaa");
        p.push_start("bbbb");
        // "⏳ aaaa…" is 7 UTF-16 units; cap 10 forces a split.
        assert_eq!(p.split_groups(10), vec!["⏳ aaaa…".to_string(), "⏳ bbbb…".to_string()]);
    }
}
```

- [ ] **Step 2: Run to verify they fail.**

Run: `cargo test -p gray-gateway progress::` (from `/home/vstaln/gray`)
Expected: FAIL with "unresolved import" / "cannot find function" (module exists but items don't).

- [ ] **Step 3: Write minimal implementation** (append above the test module):

```rust
//! Hermes-style progress bubbles (port of `hermes-gateway/src/turn.rs`
//! `on_tool_event` composition + `drain_progress_messages` grouping).
//!
//! Pure state machine — no IO. The daemon task in [`crate::daemon`] owns
//! sending, editing and deleting the bubble; this only decides WHAT text
//! goes in it.

/// Max preview chars for tool args in one bubble line.
pub const PROGRESS_PREVIEW_CAP: usize = 80;

/// `⏳ {name}…` — a tool just started.
pub fn tool_start_line(name: &str) -> String {
    format!("⏳ {name}…")
}

/// `🔧 {name}: "{compact args}"`, or `🔧 {name}…` when there are no args.
pub fn tool_end_line(name: &str, args: &serde_json::Value) -> String {
    match summarize_args(args) {
        Some(preview) => format!("🔧 {name}: \"{preview}\""),
        None => format!("🔧 {name}…"),
    }
}

fn summarize_args(args: &serde_json::Value) -> Option<String> {
    if args.is_null() || args == &serde_json::Value::Object(Default::default()) {
        return None;
    }
    let compact = serde_json::to_string(args).unwrap_or_else(|_| "…".into());
    Some(truncate_chars(&compact, PROGRESS_PREVIEW_CAP))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Accumulated bubble lines with Hermes `(×N)` dedup.
#[derive(Debug, Default)]
pub struct ProgressLines {
    lines: Vec<String>,
}

impl ProgressLines {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn push_start(&mut self, name: &str) {
        self.push_dedup(tool_start_line(name));
    }

    /// Replaces the last line when it is this tool's still-open `⏳` line,
    /// otherwise appends (dedup still applies).
    pub fn push_end(&mut self, name: &str, args: &serde_json::Value) {
        let start = tool_start_line(name);
        if self.lines.last().is_some_and(|l| *l == start) {
            self.lines.pop();
        }
        self.push_dedup(tool_end_line(name, args));
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Hermes `split_overflow`: group lines so each joined group fits in
    /// `max_utf16` UTF-16 units. One overlong line forms its own group.
    pub fn split_groups(&self, max_utf16: usize) -> Vec<String> {
        let mut groups: Vec<String> = Vec::new();
        let mut cur: Vec<&str> = Vec::new();
        let mut cur_len = 0usize; // joined length incl. newlines
        for line in &self.lines {
            let lw = utf16_len(line);
            let add = if cur.is_empty() { lw } else { lw + 1 };
            if !cur.is_empty() && cur_len + add > max_utf16 {
                groups.push(cur.join("\n"));
                cur.clear();
                cur_len = 0;
            }
            cur.push(line);
            cur_len += if cur.len() == 1 { lw } else { lw + 1 };
        }
        if !cur.is_empty() {
            groups.push(cur.join("\n"));
        }
        groups
    }

    /// Append, collapsing consecutive identical lines into `{line} (×N)`.
    fn push_dedup(&mut self, msg: String) {
        let base = strip_count_suffix(self.lines.last().map(String::as_str).unwrap_or(""));
        if base == msg {
            let n = count_suffix(self.lines.last().map(String::as_str).unwrap_or("")) + 1;
            *self.lines.last_mut().unwrap() = format!("{msg} (×{n})");
        } else {
            self.lines.push(msg);
        }
    }
}

/// `"foo (×3)"` → `"foo"`; anything else unchanged.
fn strip_count_suffix(line: &str) -> &str {
    match line.rfind(" (×") {
        Some(i) if line.ends_with(')') && line[i + 4..line.len() - 1].chars().all(|c| c.is_ascii_digit()) => &line[..i],
        _ => line,
    }
}

/// `"foo (×3)"` → `3`; anything else → `1`.
fn count_suffix(line: &str) -> usize {
    match line.rfind(" (×") {
        Some(i) if line.ends_with(')') => line[i + 4..line.len() - 1].parse().unwrap_or(1),
        _ => 1,
    }
}
```

- [ ] **Step 4: Run to verify they pass.**

Run: `cargo test -p gray-gateway progress::`
Expected: all 10 PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/gray-gateway/src/progress.rs crates/gray-gateway/src/lib.rs
git commit -m "feat(gateway): pure progress-bubble state machine (Hermes grouping/dedup)"
```

---

### Task 2: `delete_message` on the adapter trait + all three platforms

**Files:**
- Modify: `crates/gray-gateway/src/platform.rs` (trait default, next to `edit_message` at ~line 80)
- Modify: `crates/gray-gateway/src/discord.rs` (next to `edit_message` at ~line 448)
- Modify: `crates/gray-gateway/src/telegram.rs` (next to `edit_message` at ~line 438)
- Modify: `crates/gray-gateway/src/slack.rs` (next to `edit_message` at ~line 328)
- Test: no new test files; verify with `cargo check` (all feature combos) + existing suite. Deletion is best-effort at the call site, so stub-path coverage comes free in Task 4's bubble test.

**Interfaces:**
- Consumes: existing `SendResult::{ok, fail}` constructors.
- Produces (used by Task 4): `async fn delete_message(&self, chat: &str, message_id: &str) -> SendResult` on `BasePlatformAdapter`.

- [ ] **Step 1: Add the trait default** in `platform.rs` right after `edit_message`:

```rust
/// Delete a previously sent message (progress-bubble cleanup). Default
/// fails non-retryable; platforms with a delete API override it.
/// Best-effort at call sites — a failed delete never fails the turn.
async fn delete_message(&self, _chat: &str, _message_id: &str) -> SendResult {
    SendResult::fail(format!("{} does not support message deletion", self.platform()), false)
}
```

- [ ] **Step 2: Implement per platform.** Each goes immediately after that file's `edit_message`. All three follow the same shape: parse ids with the file's existing helpers, call delete, `ok` on success, retryable `fail` on error; stub (`not(feature)`) branch logs + `ok` (mirrors the existing edit stubs).

Discord (`discord.rs`, needs `twilight_http::Client` in scope already as `client`):

```rust
async fn delete_message(&self, chat: &str, message_id: &str) -> SendResult {
    #[cfg(feature = "discord")]
    {
        let Some(client) = self.client.lock().unwrap().clone() else {
            return SendResult::fail("discord not connected", false);
        };
        let (Ok(cid), Ok(mid)) = (chat.parse::<u64>(), message_id.parse::<u64>()) else {
            return SendResult::fail(format!("invalid discord ids {chat:?}/{message_id:?}"), false);
        };
        return match client
            .delete_message(twilight_model::id::Id::new(cid), twilight_model::id::Id::new(mid))
            .await
        {
            Ok(_) => SendResult::ok(Some(message_id.to_string())),
            Err(e) => SendResult::fail(format!("discord delete: {e}"), true),
        };
    }
    #[cfg(not(feature = "discord"))]
    {
        log::info!("[discord] delete {chat}/{message_id}");
        SendResult::ok(Some(message_id.to_string()))
    }
}
```

Telegram (`telegram.rs`, mirrors its `edit_message`: `parse_chat_target(chat)` for `cid`, `message_id.parse::<i32>()` for `mid`):

```rust
async fn delete_message(&self, chat: &str, message_id: &str) -> SendResult {
    #[cfg(feature = "telegram")]
    {
        use teloxide::prelude::*;
        use teloxide::types::MessageId;
        let Some(bot) = self.client.lock().unwrap().clone() else {
            return SendResult::fail("telegram not connected", false);
        };
        let Ok((cid, _)) = parse_chat_target(chat) else {
            return SendResult::fail(format!("invalid telegram chat id {chat:?}"), false);
        };
        let Ok(mid) = message_id.parse::<i32>() else {
            return SendResult::fail(format!("invalid telegram message id {message_id:?}"), false);
        };
        return match bot.delete_message(ChatId(cid), MessageId(mid)).await {
            Ok(_) => SendResult::ok(Some(message_id.to_string())),
            Err(e) => SendResult::fail(format!("telegram delete: {e}"), true),
        };
    }
    #[cfg(not(feature = "telegram"))]
    {
        log::info!("[telegram] delete {chat}/{message_id}");
        SendResult::ok(Some(message_id.to_string()))
    }
}
```

Slack (`slack.rs`, mirrors its `edit_message` session/request pattern; `SlackApiChatDeleteRequest::new(channel, ts)` + `client.open_session(&token).chat_delete(&req)` — confirm exact names against the `chat_update` call above it; both come from `slack_morphism::prelude::*`):

```rust
async fn delete_message(&self, chat: &str, message_id: &str) -> SendResult {
    #[cfg(feature = "slack")]
    {
        use slack_morphism::prelude::*;
        let Some(client) = self.client.lock().unwrap().clone() else {
            return SendResult::fail("slack not connected", false);
        };
        let Ok((channel, _)) = parse_chat_target(chat) else {
            return SendResult::fail(format!("invalid slack channel {chat:?}"), false);
        };
        let token = SlackApiToken::new(self.bot_token.clone().into());
        let req = SlackApiChatDeleteRequest::new(
            SlackChannelId(channel),
            SlackTs(message_id.to_string()),
        );
        return match client.open_session(&token).chat_delete(&req).await {
            Ok(_) => SendResult::ok(Some(message_id.to_string())),
            Err(e) => SendResult::fail(format!("slack delete: {e}"), true),
        };
    }
    #[cfg(not(feature = "slack"))]
    {
        log::info!("[slack] delete {chat}/{message_id}");
        SendResult::ok(Some(message_id.to_string()))
    }
}
```

- [ ] **Step 3: Verify all feature combos compile.**

Run: `cargo check -p gray-gateway` then `cargo check -p gray-gateway --features all-platforms` then `cargo test -p gray-gateway` (from `/home/vstaln/gray`)
Expected: all green (compiler is the review for the slack-morphism request names — if `SlackApiChatDeleteRequest`/`chat_delete` don't match the vendored version, fix names to the sibling `chat_update` call's shapes and re-run).

- [ ] **Step 4: Commit.**

```bash
git add crates/gray-gateway/src/platform.rs crates/gray-gateway/src/discord.rs crates/gray-gateway/src/telegram.rs crates/gray-gateway/src/slack.rs
git commit -m "feat(gateway): delete_message on adapter trait + discord/telegram/slack"
```

---

### Task 3: Rewire agent events to progress events (no chat text while working)

**Files:**
- Modify: `crates/gray-gateway/src/daemon.rs` (`run_agent` `on_event` closure, ~line 520)
- Test: covered by Task 4's bubble test (no isolated test — the closure only forwards).

**Interfaces:**
- Consumes: `AgentEvent::{ToolCallStart { name, .. }, ToolCallEnd { name, args, .. }}` (already exist in `gray-core/src/event.rs`); `ProgressMsg` from Task 4 — so implement the `on_event` change INSIDE Task 4's edit. This task only locks the decision: `TextDelta` and `ToolResult` are NO LONGER forwarded to chat (final answer arrives once via `reply()`).

- [ ] **Step 1: Confirm no other `StreamMsg::Delta`/`Reset` producers exist.**

Run: `grep -rn "StreamMsg::" crates/gray-gateway/src/daemon.rs`
Expected: only the `on_event` closure and the `Streamer` internals (both replaced in Task 4). `run_cron_job` passes `None` as sink — untouched.

No commit (folds into Task 4).

---

### Task 4: `ProgressBubble` task replaces `Streamer`; delete-then-deliver finish

**Files:**
- Modify: `crates/gray-gateway/src/daemon.rs`:
  - replace `StreamMsg` enum (~line 655) with `ProgressMsg`
  - replace `Streamer` (~line 665) with `ProgressBubble`
  - delete `finalize_stream` (~line 726) — superseded by delete + `reply()`
  - replace `STREAM_EDIT_INTERVAL`/`STREAM_MIN_CHARS`/`STREAM_CURSOR` consts (~line 31) with `PUMP_MIN_INTERVAL`
  - update the spawn site (~line 421), `run_agent` sink type + `on_event` (~line 488/520), finish site (~line 443)
  - replace `finalize_stream_chunks_after_edit` test (~line 1100)
- Test: `cargo test -p gray-gateway` + new `progress_bubble_deletes_bubble_then_delivers` tokio test (stub path, no network).

**Interfaces:**
- Consumes: `crate::progress::{ProgressLines}` (Task 1); `adapter.delete_message` (Task 2); `Adapter` type alias, `SendOptions`, `SendResult` (existing).
- Produces: `ProgressBubble::spawn(adapter, chat, opts, max) -> Self` with public `tx: UnboundedSender<ProgressMsg>`; `async fn finish(self, text: String) -> String`.

Pump rules (Hermes `drain_progress_messages`, adapted to gray's live task):
- `ToolStart(name)` → `lines.push_start(name)`; pump.
- `ToolEnd(name, args)` → `lines.push_end(name, args)`; pump.
- `Done` → break; delete every tracked bubble id best-effort (log warn, ignore result); return.
- `pump()`: no-op when lines empty. First send is IMMEDIATE (no throttle — the bubble must exist before any edit). Edits throttled to `PUMP_MIN_INTERVAL` (1.5s, same value as the old `STREAM_EDIT_INTERVAL` — Discord/Telegram per-chat edit limits sit ~1/s). Overflow: `split_groups(max)`; group 0 edits the current bubble id (or sends when none); extra groups send as new bubbles; ALL sent ids tracked for later deletion. Permanent edit failure → send last group as a new bubble and keep editing that id (Hermes fallback); retryable failure → skip this tick. Send/edit/delete failures never fail the turn.

- [ ] **Step 1: Replace consts, enum, task, finalizer.** Delete `STREAM_EDIT_INTERVAL`, `STREAM_MIN_CHARS`, `STREAM_CURSOR`, `StreamMsg`, `Streamer`, `finalize_stream`; add:

```rust
/// Minimum interval between progress-bubble EDITS while working
/// (Discord/Telegram edit rate limits sit around 1/s per chat).
/// The first send is always immediate; only edits are throttled.
const PUMP_MIN_INTERVAL: Duration = Duration::from_millis(1500);

#[derive(Debug)]
pub enum ProgressMsg {
    ToolStart { name: String },
    ToolEnd { name: String, args: serde_json::Value },
    Done,
}

struct ProgressBubble {
    tx: tokio::sync::mpsc::UnboundedSender<ProgressMsg>,
    task: tokio::task::JoinHandle<()>,
}

impl ProgressBubble {
    fn spawn(adapter: Adapter, chat: String, opts: SendOptions, max: usize) -> Self {
        use crate::progress::ProgressLines;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProgressMsg>();
        let task = tokio::spawn(async move {
            let mut lines = ProgressLines::new();
            let mut bubble_ids: Vec<String> = Vec::new();
            let mut last_pump = Instant::now() - PUMP_MIN_INTERVAL;
            while let Some(m) = rx.recv().await {
                match m {
                    ProgressMsg::ToolStart { name } => lines.push_start(&name),
                    ProgressMsg::ToolEnd { name, args } => lines.push_end(&name, &args),
                    ProgressMsg::Done => break,
                }
                Self::pump(&adapter, &chat, &opts, max, &lines, &mut bubble_ids, &mut last_pump).await;
            }
            // Hermes cleanup_msg_ids: progress bubbles are ephemeral — the
            // chat ends with just the final answer.
            for id in &bubble_ids {
                if let Err(e) = adapter.delete_message(&chat, id).await.map(|r| {
                    if !r.success {
                        log::warn!("progress bubble delete failed: {:?}", r.error);
                    }
                }) {
                    log::warn!("progress bubble delete failed: {e:#}");
                }
            }
        });
        Self { tx, task }
    }

    /// Send the first bubble immediately; edit it (throttled) afterwards.
    /// Overflow rolls into extra bubbles; every sent id is tracked so the
    /// finish path can delete them all.
    async fn pump(
        adapter: &Adapter,
        chat: &str,
        opts: &SendOptions,
        max: usize,
        lines: &ProgressLines,
        bubble_ids: &mut Vec<String>,
        last_pump: &mut Instant,
    ) {
        if lines.is_empty() {
            return;
        }
        let groups = lines.split_groups(max);
        let Some(first) = groups.first() else { return };
        let current = bubble_ids.last().cloned();
        match current {
            None => {
                match adapter.send_ext(chat, first, opts).await {
                    Ok(r) if r.success => {
                        if let Some(id) = r.message_id {
                            bubble_ids.push(id);
                        }
                        *last_pump = Instant::now();
                    }
                    Ok(r) => log::warn!("progress bubble send failed: {:?}", r.error),
                    Err(_) => {}
                }
            }
            Some(id) => {
                if last_pump.elapsed() < PUMP_MIN_INTERVAL {
                    return;
                }
                match adapter.edit_message(chat, &id, first).await {
                    Ok(r) if r.success => *last_pump = Instant::now(),
                    Ok(r) if r.retryable => {}
                    Ok(r) => {
                        log::warn!("progress bubble edit failed permanently, sending fresh: {:?}", r.error);
                        Self::send_fresh(adapter, chat, opts, first, bubble_ids).await;
                        *last_pump = Instant::now();
                    }
                    Err(_) => {}
                }
            }
        }
        // Overflow groups always send as new bubbles (Hermes first-edits-rest-sends).
        for extra in groups.iter().skip(1) {
            Self::send_fresh(adapter, chat, opts, extra, bubble_ids).await;
        }
    }

    async fn send_fresh(
        adapter: &Adapter,
        chat: &str,
        opts: &SendOptions,
        text: &str,
        bubble_ids: &mut Vec<String>,
    ) {
        match adapter.send_ext(chat, text, opts).await {
            Ok(r) if r.success => {
                if let Some(id) = r.message_id {
                    bubble_ids.push(id);
                }
            }
            Ok(r) => log::warn!("progress bubble send failed: {:?}", r.error),
            Err(_) => {}
        }
    }

    /// Delete bubbles, then hand the final text back so the caller delivers
    /// it as fresh message(s) via the normal chunked path. Never fails —
    /// on task panic the text is still returned for delivery.
    async fn finish(self, text: String) -> String {
        let _ = self.tx.send(ProgressMsg::Done);
        drop(self.tx);
        if let Err(e) = self.task.await {
            log::warn!("progress bubble task failed: {e}");
        }
        text
    }
}
```

NOTE: `send_ext`/`edit_message`/`delete_message` return `SendResult` directly (not `Result`), NOT `Result<SendResult, E>` — the trait methods are `async fn ... -> SendResult`. So `adapter.send_ext(...).await` yields `SendResult`, and there is no `Err(_)` arm. Correct the match arms at implementation time to:

```rust
let r = adapter.send_ext(chat, first, opts).await;
if r.success {
    if let Some(id) = r.message_id { bubble_ids.push(id); }
    *last_pump = Instant::now();
} else {
    log::warn!("progress bubble send failed: {:?}", r.error);
}
```

and for delete:

```rust
for id in &bubble_ids {
    let r = adapter.delete_message(&chat, id).await;
    if !r.success {
        log::warn!("progress bubble delete failed: {:?}", r.error);
    }
}
```

(The sketch above was written with `Result` arms by mistake — the compiler will catch it; shape is as written here.)

- [ ] **Step 2: Update the three call sites.**

Spawn site (~line 421):

```rust
let progress = match &adapter {
    Some(a) if self.config.streaming && a.supports_edit() => {
        Some(ProgressBubble::spawn(Arc::clone(a), chat_id.clone(), Self::reply_opts(&ev), platform.max_message_len()))
    }
    _ => None,
};
let sink = progress.as_ref().map(|p| p.tx.clone());
```

`run_agent` signature: `sink: Option<tokio::sync::mpsc::UnboundedSender<ProgressMsg>>`. `on_event` body:

```rust
let mut on_event = |e: &AgentEvent| {
    if let Some(tx) = &sink {
        match e {
            AgentEvent::ToolCallStart { name, .. } => {
                let _ = tx.send(ProgressMsg::ToolStart { name: name.clone() });
            }
            AgentEvent::ToolCallEnd { name, args, .. } => {
                let _ = tx.send(ProgressMsg::ToolEnd { name: name.clone(), args: args.clone() });
            }
            _ => {}
        }
    }
};
```

Finish site (~line 443):

```rust
// 5. Deliver — progress bubbles are deleted, then the final answer
// goes out as fresh message(s) on the normal chunked path.
let res = match progress {
    Some(p) => {
        let final_text = p.finish(reply_text).await;
        self.reply(&ev, &final_text).await
    }
    None => self.reply(&ev, &reply_text).await,
};
```

- [ ] **Step 3: Replace the obsolete test.** Delete `finalize_stream_chunks_after_edit`; add (stub path runs with no features — TelegramAdapter stub send/edit/delete log + succeed):

```rust
#[tokio::test]
async fn progress_bubble_tracks_tool_lines_and_finishes() {
    use crate::progress::{tool_end_line, tool_start_line};
    // Pure composition sanity inside the IO test's world.
    assert_eq!(tool_start_line("terminal"), "⏳ terminal…");
    assert_eq!(
        tool_end_line("terminal", &serde_json::json!({"command": "ls"})),
        "🔧 terminal: \"{\"command\":\"ls\"}\""
    );
    // End-to-end through the bubble task on the stub adapter (no network):
    // tool events → bubble sends → Done deletes → final text returned.
    let a: Adapter = Arc::new(TelegramAdapter::new(PlatformConfig::with_token("123456:ABCDEFGHIJ1234567890")).unwrap());
    let b = ProgressBubble::spawn(a, "100".into(), SendOptions::default(), 4096);
    b.tx.send(ProgressMsg::ToolStart { name: "terminal".into() }).unwrap();
    b.tx.send(ProgressMsg::ToolEnd { name: "terminal".into(), args: serde_json::json!({"command": "ls"}) }).unwrap();
    let out = b.finish("done".into()).await;
    assert_eq!(out, "done");
}
```

- [ ] **Step 4: Run full suite (no features AND all features).**

Run: `cargo test -p gray-gateway` then `cargo check -p gray-gateway --features all-platforms`
Expected: PASS, including the 10 `progress::` tests + the new bubble test. `grep -rn "Streamer\|StreamMsg\|finalize_stream\|STREAM_CURSOR\|STREAM_MIN_CHARS\|STREAM_EDIT_INTERVAL" crates/gray-gateway/src/` must return NOTHING (dead code check).

- [ ] **Step 5: Commit.**

```bash
git add crates/gray-gateway/src/daemon.rs
git commit -m "feat(gateway): Hermes progress bubbles replace streaming answer edits"
```

---

## Self-review

- Spec coverage: Discord-only vs all-platforms → Task 2+4 apply the same path to all three `supports_edit` adapters (user chose all). Delete-after-turn → Task 4 Done path. Final-answer-only → Task 3/4 (no `TextDelta` forwarding, `reply()` delivery). Typing loop untouched in all tasks. Hermes source behaviors mapped: grouped edit (pump), `(×N)` dedup (Task 1), overflow roll (split_groups + extra sends), no-edit drain-silently (unchanged `supports_edit` gate), cleanup ids (bubble_ids + Done deletes), tool.started-only bubbles (on_event), permanent-edit-failure fallback (send_fresh).
- Placeholders: none — every step has exact code, exact commands, exact expected output. Two compiler-verified spots flagged inline (slack-morphism names, `SendResult`-not-`Result` arms).
- Type consistency: `ProgressLines::{push_start(&str), push_end(&str, &Value), text(), split_groups(usize)}` defined Task 1, used Task 4 identically. `delete_message(&self, chat: &str, message_id: &str) -> SendResult` defined Task 2, called Task 4 identically. `ProgressMsg::{ToolStart{name: String}, ToolEnd{name: String, args: Value}, Done}` defined and used in Task 4. `ProgressBubble::spawn(Adapter, String, SendOptions, usize)`, `tx` field, `finish(self, String) -> String` consistent across Steps 1–3.
- Gap fixed during review: initial `pump`/`finish` sketch used `Result` arms; corrected to direct `SendResult` with a compiler-backstop note.
