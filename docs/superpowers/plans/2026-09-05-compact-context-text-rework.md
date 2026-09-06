# Compact Context-Text Rework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the display-based token ruler with a billable-context ruler so compaction actually shrinks tool-heavy history.

**Architecture:** Add one sibling accessor `Message::context_text()` in gray-core that concatenates every billable block, then point the two budgeting call sites (`estimate_tokens`, `try_compact_once`) at it. No behavior change to rendering (`text_content()` untouched).

**Tech Stack:** Rust 2024, cargo test, gray-core / gray crates

**Spec:** `/tmp/opencode/audit3/index.html` sections 3-5 (root cause, 3-file diff, 2 tests, expected-results table). Empryo material in section 7 is concepts-only, never copied.

## Global Constraints

- MIT-only output: original code written against gray's own public API; no BUSL code, identifiers, or structure reproduced.
- Do NOT modify `Message::text_content()` — load-bearing for rendering and `summary_pair_envelope_is_byte_stable`.
- Keep the existing `chars / 4` heuristic; only change what is measured.
- Keep `estimate_tokens` signature unchanged: `pub fn estimate_tokens(msg: &Message) -> usize`.
- Branch `fix/compact-estimate-tool-blocks` targets `main`; PR #20 must end with only the 3-file diff.
- Verify with `cargo test -p gray --lib compact` and `cargo test -p gray-core --lib` before each commit.

---

### Task 1: Rebase branch onto main

**Files:**
- Modify: none (git history only)

**Interfaces:**
- Consumes: remote `origin/main`, local branch `fix/compact-estimate-tool-blocks`
- Produces: clean `git diff origin/main...HEAD --stat` showing only the 3 intended files

- [ ] **Step 1: Fetch and inspect drift**

```bash
git fetch origin
git diff origin/main...HEAD --stat
```

Run: `git fetch origin` then `git diff origin/main...HEAD --stat`
Expected: currently shows 8 files (compact.rs plus oauth/repl/setup/provider drift from main moving under PR #20). After rebase it must show exactly 3 files: `crates/gray-core/src/message.rs`, `crates/gray-core/src/agent.rs`, `crates/gray/src/compact.rs`.

- [ ] **Step 2: Rebase and resolve by keeping main for unrelated files**

```bash
git checkout fix/compact-estimate-tool-blocks
git rebase origin/main
```

Run: `git rebase origin/main`
Expected: conflict markers only in files you did not intend (oauth/repl/setup/provider). For each unrelated file accept main: `git checkout --theirs -- <file>` only if the file is NOT one of the 3 intended files. Never accept theirs for the 3 intended files. Then `git rebase --continue`. Final `git diff origin/main...HEAD --stat` lists at most the 3 intended files.

- [ ] **Step 3: Commit-free checkpoint**

```bash
git status --short
```

Run: `git status --short`
Expected: PASS when only intended files are modified. Do not commit in this task.

---

### Task 2: Add Message::context_text() accessor

**Files:**
- Modify: `crates/gray-core/src/message.rs:148-158` (after `text_content()`)
- Test: `crates/gray-core/src/message.rs` (no new test file; covered by Task 5 integration tests)

**Interfaces:**
- Consumes: `ContentBlock::{Text, ToolResult, ToolUse, Thinking, Image}` with fields `text`, `content`, `name`, `args: serde_json::Value`, `encrypted_content: Option<String>`, `data`
- Produces: `impl Message { pub fn context_text(&self) -> String }` used by Tasks 3 and 4

- [ ] **Step 1: Write the failing usage probe (no test file yet, compile check)**

```bash
grep -n "context_text" crates/gray-core/src/message.rs || echo MISSING
```

Run: `grep -n "context_text" crates/gray-core/src/message.rs || echo MISSING`
Expected: outputs `MISSING`.

- [ ] **Step 2: Implement the accessor**

```rust
/// Everything in this message that the provider will bill as context —
/// not just its prose.
///
/// [`text_content`](Self::text_content) is a *display* accessor: it keeps
/// `Text` blocks and drops tool results, tool arguments, replayed
/// reasoning blobs and image payloads. Those are precisely the blocks
/// that dominate a coding session, so any budgeting code that measures
/// with `text_content` scores a 50 KiB tool result as zero tokens.
/// Size-estimation callers must use this instead.
///
/// Concatenation order and separators are irrelevant to callers: only the
/// resulting length is meaningful. Kept deliberately allocation-simple —
/// it runs once per message per compaction, not per token.
pub fn context_text(&self) -> String {
    let mut out = String::new();
    for block in &self.content {
        let piece = match block {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::ToolResult { content, .. } => content.clone(),
            ContentBlock::ToolUse { name, args, .. } => {
                format!("{name}{args}")
            }
            ContentBlock::Thinking { text, encrypted_content, .. } => {
                // The encrypted blob is replayed verbatim next turn to
                // keep the provider's cache shard warm, and is billed
                // like any other input token. It must be counted.
                match encrypted_content {
                    Some(blob) => format!("{text}{blob}"),
                    None => text.clone(),
                }
            }
            // Base64 payload length is the only size signal available
            // here; providers re-encode, so this is an approximation in
            // the same spirit as the chars/4 heuristic downstream.
            ContentBlock::Image { data, .. } => data.clone(),
        };
        if !piece.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&piece);
        }
    }
    out
}
```

Place it immediately after `text_content()` inside `impl Message`, before the closing `}` of that impl block.

- [ ] **Step 3: Verify it compiles**

```bash
cargo check -p gray-core 2>&1 | tail -5
```

Run: `cargo check -p gray-core 2>&1 | tail -5`
Expected: `Finished` with no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/gray-core/src/message.rs
git commit -m "feat(core): add Message::context_text for billable context"
```

---

### Task 3: Point estimate_tokens at context_text

**Files:**
- Modify: `crates/gray/src/compact.rs:225-237` (`estimate_tokens`)
- Test: `crates/gray/src/compact.rs` existing `mod tests`

**Interfaces:**
- Consumes: `Message::context_text()` from Task 2
- Produces: corrected `estimate_tokens`, `tail_messages`, `estimate_context_tokens`, `should_compact` behavior with zero call-site changes

- [ ] **Step 1: Replace the ruler (replaces the PR #20 inline match)**

```rust
pub fn estimate_tokens(msg: &Message) -> usize {
    // Must measure billable context, not displayable prose: a message whose
    // only block is a 50 KiB tool result is ~12.8k tokens, not 0. See
    // `Message::context_text`.
    (msg.context_text().len() as f64 / 4.0).ceil() as usize
}
```

Delete the current multi-arm `match` over `msg.content` in `estimate_tokens` and substitute the 1-line body above. `ContentBlock` import stays (still used by tests); no new imports.

- [ ] **Step 2: Run existing compact tests**

```bash
cargo test -p gray --lib compact 2>&1 | tail -6
```

Run: `cargo test -p gray --lib compact 2>&1 | tail -6`
Expected: all existing tests PASS (text-only estimates shift by at most one sub-token from newline joins; `estimate_falls_back_to_chars` still 100; `tail_keeps_recent_within_budget` still passes).

- [ ] **Step 3: Commit**

```bash
git add crates/gray/src/compact.rs
git commit -m "fix(compact): meter billable context via context_text"
```

---

### Task 4: Point overflow recovery at context_text

**Files:**
- Modify: `crates/gray-core/src/agent.rs:312-318` (`try_compact_once` transcript)

**Interfaces:**
- Consumes: `Message::context_text()` from Task 2
- Produces: overflow-recovery summary that includes tool results instead of silently dropping them

- [ ] **Step 1: Swap the transcript line**

```rust
let transcript = self
    .messages
    .iter()
    // Overflow recovery previously summarized `text_content`, which
    // omits every tool result — the summary lost exactly the file
    // bodies and command output the run depended on, forcing the
    // model to re-read them after compaction.
    .map(|m| format!("{}: {}", m.role, m.context_text()))
    .collect::<Vec<_>>()
    .join("\n");
```

Change only the `.map(...)` line; keep the `is_empty` guard, `complete_prompt` call, and `summary_pair` assignment identical.

- [ ] **Step 2: Verify core tests still pass**

```bash
cargo test -p gray-core --lib 2>&1 | tail -6
```

Run: `cargo test -p gray-core --lib 2>&1 | tail -6`
Expected: PASS. `summary_pair_envelope_is_byte_stable` untouched and passing.

- [ ] **Step 3: Commit**

```bash
git add crates/gray-core/src/agent.rs
git commit -m "fix(core): include tool results in overflow-recovery transcript"
```

---

### Task 5: Add the two regression tests

**Files:**
- Modify: `crates/gray/src/compact.rs` `#[cfg(test)] mod tests` (append after `estimate_falls_back_to_chars`)
- Test: same file

**Interfaces:**
- Consumes: `estimate_tokens`, `tail_messages`, `estimate_context_tokens`, `should_compact`, `CompactionSettings`, `Message::new`, `Role::User`, `ContentBlock::tool_result`
- Produces: failing-on-main, passing-with-patch guards for both consequences

- [ ] **Step 1: Write the failing tests**

```rust
/// A message holding one capped tool result (`truncate.rs` allows 50 KiB)
/// must not measure as free. Regression guard for `estimate_tokens`
/// measuring with a display accessor.
#[test]
fn tail_budget_counts_tool_results() {
    // 4 KiB of tool output => 4096/4 = 1024 tokens.
    let big = Message::new(
        Role::User,
        vec![ContentBlock::tool_result("t1", "x".repeat(4096), false)],
    );
    assert_eq!(
        estimate_tokens(&big),
        1024,
        "a 4 KiB tool result is ~1024 tokens; measuring 0 means the \
         estimator is reading text blocks only"
    );

    // Ten of them against a 3000-token keep budget: the walk should stop
    // after two. Before the fix each scored 0, so all ten were retained
    // and `keep_recent_tokens` was a no-op for tool-heavy history.
    let msgs: Vec<Message> = (0..10).map(|_| big.clone()).collect();
    let tail = tail_messages(&msgs, 3000);
    assert_eq!(
        tail.len(),
        2,
        "tail must stop at the 3000-token budget, kept {}",
        tail.len()
    );
}

/// With no provider `Usage` (resumed session, or an OpenAI-compatible
/// endpoint that omits usage on streaming), the estimator is the only
/// signal deciding whether to compact. A tool-heavy history must cross
/// the reserve line.
#[test]
fn tool_heavy_history_trips_threshold_without_provider_usage() {
    let msgs: Vec<Message> = (0..10)
        .map(|i| {
            Message::new(
                Role::User,
                vec![ContentBlock::tool_result(
                    format!("t{i}"),
                    "y".repeat(8192),
                    false,
                )],
            )
        })
        .collect();

    // 10 x 8 KiB = 80 KiB => 20480 tokens. Before the fix: 0.
    let tokens = estimate_context_tokens(&msgs, None);
    assert_eq!(tokens, 20_480, "80 KiB of tool output measured as {tokens}");

    let s = CompactionSettings {
        enabled: true,
        reserve_tokens: 16_384,
        keep_recent_tokens: 20_000,
    };
    // 32k window, 16384 reserve => threshold 15616. 20480 is over it.
    assert!(
        should_compact(tokens, 32_000, &s),
        "tool-heavy history must auto-compact; before the fix it measured \
         zero and the session ran straight into a provider overflow"
    );
}
```

Requires in scope (already present via `use super::*` plus the file-level `use gray_core::message::{ContentBlock, Message, Role};`): `estimate_tokens`, `tail_messages`, `estimate_context_tokens`, `should_compact`, `CompactionSettings`. Do not add other tests.

- [ ] **Step 2: Run the new tests**

```bash
cargo test -p gray --lib compact::tests::tail_budget_counts_tool_results 2>&1 | tail -5
cargo test -p gray --lib compact::tests::tool_heavy_history_trips_threshold_without_provider_usage 2>&1 | tail -5
```

Run: both commands above.
Expected: each PASS with patch. Sanity: `git stash && cargo test -p gray --lib compact::tests::tail_budget_counts_tool_results` FAILs (0 vs 1024), then `git stash pop`.

- [ ] **Step 3: Run the full workspace gate**

```bash
cargo test --workspace 2>&1 | tail -8
```

Run: `cargo test --workspace 2>&1 | tail -8`
Expected: all suites PASS, including `summary_pair_envelope_is_byte_stable` and the PR #20 test `estimate_counts_tool_blocks_not_just_text` if still present (delete that single-case test in this task if it duplicates the new 4 KiB exact test — keep only the two new tests).

- [ ] **Step 4: Commit and push**

```bash
git add crates/gray/src/compact.rs
git commit -m "test(compact): pin tool-result budgeting and threshold trip"
git push origin fix/compact-estimate-tool-blocks
git diff origin/main...HEAD --stat
```

Run: commands above.
Expected: final stat shows exactly 3 files (`message.rs`, `agent.rs`, `compact.rs`), PR #20 updates in place, `test` check re-runs.
