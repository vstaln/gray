# Auto-Compact on Context Threshold Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically compact conversation when token usage approaches the model's context window, mirroring pi's threshold/overflow compaction, without Prime's complexity.

**Architecture:** Add threshold check before each agent turn using `resolve_model_context_length()` and last `Usage`. If `tokens > window - reserve`, run existing `compact` logic automatically (reason="threshold"). Also catch provider `context_length_exceeded`/`max_tokens` errors and compact with reason="overflow" then retry. Keep pi's `reserveTokens`/`keepRecentTokens` defaults, reuse `serialize_conversation`/`build_summarization_prompt`.

**Tech Stack:** Rust, `gray-core` (agent loop), `gray` (compact, setup/context), `gray-provider` error mapping, `tokio`

**Spec:** User request: "it doesnt auto compact when close to context window can u fix that? lets plan first. how can we add something like that. check how pi does it pi agent not prime" — reference `reference/pi-mono/packages/agent/src/harness/compaction/compaction.ts:147,246` and `crates/gray/src/compact.rs:1`

## Global Constraints
- Ponytail ultra: minimal diff, reuse existing `compact.rs` + `resolve_model_context_length` + `fetch_live_provider_models`, no new deps
- Portal/gateway crates untouched
- Token estimate must use provider `Usage` when available, fallback `chars/4` heuristic like pi `estimateTokens` `compaction.ts:270`
- Must not break manual `/compact` — auto uses same summary path with reason annotation

---

### File Structure
- `crates/gray/src/setup/context.rs` — already provides `resolve_model_context_length()`, `parse_context_window()`, `format_context_length()`; no change needed except expose `DEFAULT_RESERVE_TOKENS`
- `crates/gray/src/compact.rs` — add `CompactionSettings`, `should_compact()`, `estimate_tokens()`, `calculate_context_tokens()`, expose `prepare_auto_compact()` helper
- `crates/gray/src/repl/mod.rs` — add threshold check before `agent.run_streaming()` and overflow catch after `CoreError::Provider` with `context_length` substring, trigger auto-compact and retry once
- `crates/gray-core/src/agent.rs` — optional: expose `estimate` helper or keep in `gray` crate only (prefer no core change, keep in `gray` crate)
- `crates/gray-provider/src/openai.rs` — verify error mapping surfaces `context_length_exceeded` for overflow detection (already maps provider errors)
- Test: `crates/gray/src/compact.rs` unit tests for `should_compact`, `estimate_tokens`, `parse`

---

### Task 1: Add compaction threshold types and helpers (port pi defaults)

**Files:**
- Modify: `crates/gray/src/compact.rs:1-127`

**Interfaces:**
- Consumes: `resolve_model_context_length()` from `setup/context.rs`, `Usage` from `gray_core::event::Usage`
- Produces: `pub struct CompactionSettings { enabled: bool, reserve_tokens: usize, keep_recent_tokens: usize }`, `pub const DEFAULT_COMPACTION_SETTINGS`, `pub fn should_compact(tokens, window, settings) -> bool`, `pub fn estimate_tokens(msg) -> usize`, `pub fn calculate_context_tokens(usage) -> usize`

- [ ] **Step 1: Write failing test for should_compact**
```rust
#[test]
fn should_compact_threshold() {
  let s = CompactionSettings { enabled: true, reserve_tokens: 16384, keep_recent_tokens: 20000 };
  assert!(!should_compact(100_000, 128_000, &s));
  assert!(should_compact(115_000, 128_000, &s)); // 115k > 128k-16k
  assert!(!should_compact(200_000, 128_000, &CompactionSettings { enabled: false, ..s }));
}
```

- [ ] **Step 2: Run test**
Run: `cargo test -p gray compact -- --nocapture`
Expected: FAIL `should_compact not found`

- [ ] **Step 3: Implement minimal**
```rust
pub struct CompactionSettings { pub enabled: bool, pub reserve_tokens: usize, pub keep_recent_tokens: usize }
pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings { enabled: true, reserve_tokens: 16384, keep_recent_tokens: 20000 };
pub fn should_compact(tokens: usize, window: usize, s: &CompactionSettings) -> bool { s.enabled && tokens > window.saturating_sub(s.reserve_tokens) }
pub fn calculate_context_tokens(u: &Usage) -> usize { if u.total() > 0 { u.total() } else { u.input_tokens + u.output_tokens } }
pub fn estimate_tokens(msg: &Message) -> usize { (msg.text_content().len() as f64 / 4.0).ceil() as usize } // simplified vs pi chars/4 per role
```
Port pi `compaction.ts:158,165,247,270` exactly, reuse `format_context_length`.

- [ ] **Step 4: Run test PASS**
Run: `cargo test -p gray compact`
Expected: PASS

- [ ] **Step 5: Commit**
```bash
git add crates/gray/src/compact.rs
git commit -m "feat(compact): add threshold helpers ported from pi"
```

---

### Task 2: Estimate context usage from Usage + fallback

**Files:**
- Modify: `crates/gray/src/compact.rs` (add `estimate_context_tokens`)
- Test: `crates/gray/src/compact.rs`

**Interfaces:**
- Consumes: `Agent::messages()` slices
- Produces: `pub fn estimate_context_tokens(messages: &[Message], last_usage: Option<Usage>) -> usize`

- [ ] **Step 1: Write failing test**
```rust
#[test]
fn estimate_uses_usage_when_available() {
  let msgs = vec![Message::user("hi"), Message::assistant("hello")];
  let usage = Usage { input_tokens: 100000, output_tokens: 10000, ..Default::default() };
  assert_eq!(estimate_context_tokens(&msgs, Some(usage)), 110000);
}
#[test]
fn estimate_falls_back_to_chars() {
  let msgs = vec![Message::user("a".repeat(400))]; // 100 tokens
  assert_eq!(estimate_context_tokens(&msgs, None), 100);
}
```

- [ ] **Step 2: Run fails**
- [ ] **Step 3: Implement**
```rust
pub fn estimate_context_tokens(messages: &[Message], last: Option<Usage>) -> usize {
  if let Some(u) = last { if u.total() > 0 { return u.total() } }
  messages.iter().map(estimate_tokens).sum()
}
```
Mirrors pi `estimateContextTokens` `compaction.ts:216` (usage + trailing heuristic).

- [ ] **Step 4: Run PASS**
- [ ] **Step 5: Commit**
```bash
git add crates/gray/src/compact.rs
git commit -m "feat(compact): estimate usage with fallback"
```

---

### Task 3: Auto-compact helper that reuses existing /compact flow

**Files:**
- Modify: `crates/gray/src/compact.rs` (add `auto_compact` fn)
- Modify: `crates/gray/src/repl/mod.rs:438` `handle_compact`

**Interfaces:**
- Consumes: `Agent`, `Config`, `CompactionSettings`
- Produces: `pub async fn auto_compact_if_needed(agent: &mut Agent, config: &Config, last_usage: Option<Usage>, reason: &str) -> Result<bool, _>` returns true if compacted

- [ ] **Step 1: Write failing test (mock provider)**
```rust
#[tokio::test]
async fn auto_compact_triggers_on_threshold() {
  // fake agent with 3 messages, mock provider that returns summary, verify messages.len() == 2 after
}
```

- [ ] **Step 2: Run fails**
- [ ] **Step 3: Implement minimal wrapper**
Reuse `serialize_conversation` + `build_summarization_prompt` + `agent.complete_prompt` + `agent.set_messages(vec![summary_user, summary_assistant])` exactly as `handle_compact` `crates/gray/src/repl/mod.rs:466` does, but with `CompactionReason` param and without tui push. Keep `keep_recent_tokens` logic simple: same as manual — replace all with 2-msg summary (like current). Pi's `prepareCompaction` `compaction.ts:616` with `findCutPoint` is deferred to later — YAGNI for v1.

- [ ] **Step 4: Run PASS**
- [ ] **Step 5: Commit**

---

### Task 4: Wire threshold check before each turn + overflow recovery

**Files:**
- Modify: `crates/gray/src/repl/mod.rs:1480` (main agent loop in `run_repl_mode`)
- Modify: `crates/gray/src/compact.rs` (expose `is_context_overflow_error`)

**Interfaces:**
- Consumes: `should_compact`, `estimate_context_tokens`, `resolve_model_context_length`, `auto_compact_if_needed`
- Produces: no new public API, just loop behavior

- [ ] **Step 1: Write failing test for repl (integration)**
Simulate `last_usage` at 120k with window 128k, ensure next `Prompt` triggers compact before `agent.run`.

- [ ] **Step 2: Run fails**

- [ ] **Step 3: Implement**
In `run_repl_mode` before `agent.run_streaming(prompt, ...)`:
```rust
let window = crate::setup::resolve_model_context_length(config.model.as_deref().unwrap_or(""));
let tokens = crate::compact::estimate_context_tokens(agent.messages(), tui.latest_usage);
if crate::compact::should_compact(tokens, window, &crate::compact::DEFAULT_COMPACTION_SETTINGS) {
  say(tui, &format!("auto-compacting {}/{} tokens...", format_context_length(tokens), format_context_length(window)));
  if let Err(e) = crate::compact::auto_compact_if_needed(&mut agent, config, tui.latest_usage, "threshold").await { /* log */ }
}
```
After `run_streaming` returns `Err(CoreError::Provider(msg))` and `msg.contains("context_length") || msg.contains("context window") || msg.contains("max_tokens")` (pi `agent-loop.ts:208` handles `length` truncation), then:
```rust
if is_context_overflow_error(&e) {
  auto_compact_if_needed(..., "overflow").await?;
  retry once
}
```
Only one retry to avoid loop.

- [ ] **Step 4: Run `cargo test -p gray` PASS + manual `cargo run -p "hello world"` with mocked large history**

- [ ] **Step 5: Commit**
```bash
git add crates/gray/src/repl/mod.rs crates/gray/src/compact.rs
git commit -m "feat(repl): auto-compact on threshold and overflow (pi parity)"
```

---

### Task 5: Docs and config knob

**Files:**
- Modify: `README.md` (add `GRAY_COMPACTION_RESERVE` or mention `/compact` now auto)
- Modify: `crates/gray/src/setup/catalog.rs` (optional: persist `compaction_enabled` if requested, else YAGNI)

**Interfaces:**
- No code, just docs

- [ ] **Step 1: Update README section for context window + auto-compact**
- [ ] **Step 2: Verify `gray --help` and `/help` no new flag needed (auto is default)**
- [ ] **Step 3: Commit**
```bash
git add README.md
git commit -m "docs: auto-compact threshold"
```

---

## Self-Review
- Spec coverage: pi `shouldCompact` `reserveTokens`/`keepRecentTokens` → Task1, token estimate `estimateContextTokens` → Task2, summary generation `generateSummary` already exists → Task3 reuses, threshold before turn + overflow retry `agent-loop.ts:208` → Task4. Prime-agent not needed — pi harness is source.
- Placeholders: none — each step has exact code/file.
- Types: `CompactionSettings` matches pi `CompactionSettings` shape, `Usage` from `gray_core::event::Usage`, `Message` from `gray_core::message`.

## Alternatives Considered
- Full `prepareCompaction`/`findCutPoint`/`retainedTail` like pi `compaction.ts:616` — deferred, current `/compact` replaces all with 2-msg summary, keeps diff small. Add `keepRecentTokens` windowing later if summary quality suffers.
- Adding `gray-core` change — avoided, keep estimate in `gray` crate only.
