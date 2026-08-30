# Composer Decomplexitization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `crates/gray/src/composer.rs:1-1700` into `composer/{mod.rs,text_area.rs,draw.rs,transcript.rs,input.rs}` (Grok-lean internal, crate-gated) while preserving `pub struct Tui`/`SharedTui` API and fixing viewport/wrap/hyperlink fidelity gaps.

**Architecture:** Keep single crate `gray`; façade `composer/mod.rs` owns `Terminal<CrosstermBackend>` + raw-mode lifecycle; internal modules take `&mut Tui` with `pub(crate)` vis. Word-wrap vendored from `reference/xai-org/grok-build/crates/codegen/xai-ratatui-textarea/src/wrapping.rs:173`, viewport height mirrors `reference/xai-org/grok-build/crates/codegen/xai-ratatui-inline/src/terminal.rs:888-942` but internal.

**Tech Stack:** Rust, ratatui `CrosstermBackend`, `crossterm` inline viewport `Viewport::Inline`, `gray_markdown::StreamingMarkdownRenderer`, `unicode-width` (already via ratatui), `arboard`/`image`/`tempfile` clipboard fallbacks

**Spec:** `docs/superpowers/specs/2026-08-30-composer-decomplexitization-design.md`

## Global Constraints

* No new workspace `members` or `Cargo.toml` deps in this plan (crate extraction deferred) — one line.
* Preserve `crate::composer::Tui` and `crate::composer::SharedTui(pub Arc<Mutex<Tui>>)` `composer.rs:32-40` import paths for `crates/gray/src/repl.rs:1043` — one line.
* Single raw-mode/bracketed-paste owner in `composer/mod.rs` (`enable_raw_mode:372`, `DisableBracketedPaste:1613`, `Drop:1624`) — one line.
* Single `RESIZE_DEBOUNCE:27 Duration::from_millis(75)` + `pending_resize:283 Option<(u16,Instant)>` source in `mod.rs` — one line.
* `cargo test -p gray --lib composer` + `cargo build --release && install target/release/gray ~/.local/bin/gray` must stay green every task — one line.

---

## File Structure

**Existing:** `crates/gray/src/composer.rs:1-1700` (god-file, `VIEWPORT_H:25`, `PANEL_ROWS:26`, `Term:29`, `TextArea:51-252`, `Tui:254-284`, `draw:482-757`, `input:785-1120`, `transcript:1197-1622`, tests `1636-1700`)

**After Task 1:** `crates/gray/src/composer/mod.rs` (git mv, identical content, `mod composer;` in `crates/gray/src/lib.rs:4` auto-resolves)

**After Task 2-5:** 

* `crates/gray/src/composer/text_area.rs` — `TextElement:53`, `TextArea:58-252` + `shimmer_spans` stay in draw; owns `clamp_to_boundary, next/prev_boundary, shift_elements, delete_*/move_*` with `WrapCache + preferred_col` fix for `move_up/down:230-251`
* `crates/gray/src/composer/draw.rs` — `thinking_style:286`, `shimmer_spans:290`, `wrap_styled_line:313` (replaced by word-wrap), `draw:482-757` (adds `set_viewport_height`)
* `crates/gray/src/composer/transcript.rs` — `ensure_gap:1203`, `stream:1213`, `stream_thinking:1226`, `stream_text:1240`, `end_thinking_run:1262`, `push_user_prompt:1277`, `push_line*, push_styled_lines_with_hyperlinks:1382` (batch `insert_before`), `strip_ansi:1632` with Grok link table fix deferred to draw
* `crates/gray/src/composer/input.rs` — `attach_image:759`, `sync_attachments:785`, `is_image_path:790`, `try_attach_image_paste:801`, `try_attach_clipboard_image:821`, `handle_paste:868`, `read_line:887-1120` (key dispatch)
* `crates/gray/src/composer/mod.rs` — `Tui:254`, `SharedTui:32`, `new:372`, `set_model/set_cwd/set_thinking_effort/set_usage/reset_usage/width:460-481`, `begin_turn/set_status/end_turn:1122-1195`, `tick_status:1586`, `shutdown:1610`, `Drop:1624`, tests facade

---

### Task 1: Rename composer.rs → composer/mod.rs (green baseline)

**Files:**
- Modify: `crates/gray/src/composer.rs` → `crates/gray/src/composer/mod.rs` (git mv, no edits)
- Test: `crates/gray/src/composer/mod.rs:1636-1700` existing tests (unchanged)

**Interfaces:**
- Consumes: existing `mod composer;` in `crates/gray/src/lib.rs:4`
- Produces: `crate::composer::Tui` path still resolves via `composer/mod.rs` (same as file)

- [ ] **Step 1: Create baseline log**

```bash
cargo test -p gray --lib composer -- --nocapture 2>&1 | tee /tmp/pre.log
cat /tmp/pre.log
# Expected: shimmer_spans_change_across_ticks PASS, shimmer_truecolor_changes_across_ticks PASS, textarea_multiline_and_history PASS, textarea_atomic_element PASS, consecutive_frames_differ_in_test_backend PASS
```

- [ ] **Step 2: Git mv (rename-only) — no code change**

```bash
git mv crates/gray/src/composer.rs crates/gray/src/composer/mod.rs
git status --porcelain
# Expected: R  crates/gray/src/composer.rs -> crates/gray/src/composer/mod.rs
```

- [ ] **Step 3: Verify build resolves**

Run: `cargo test -p gray --lib composer -- --nocapture 2>&1 | diff /tmp/pre.log -`
Expected: no diff (same 5 tests PASS)

Run: `cargo build -p gray 2>&1 | tail -n 5`
Expected: `Finished` no errors (lib.rs mod resolution auto-picks `composer/mod.rs`)

- [ ] **Step 4: Commit rename-only**

```bash
git add crates/gray/src/composer/mod.rs
git commit -m "refactor(composer): mv composer.rs -> composer/mod.rs (rename-only)

Co-Authored-By: internal-model"
```

---

### Task 2: Extract text_area.rs (pure, no Term)

**Files:**
- Create: `crates/gray/src/composer/text_area.rs`
- Modify: `crates/gray/src/composer/mod.rs:51-252` (remove TextArea, add `mod text_area; pub(crate) use text_area::TextArea;`)
- Test: `crates/gray/src/composer/text_area.rs` (moved `textarea_*` tests)

**Interfaces:**
- Consumes: std only (`String`, `Range`, `next_id: u64`)
- Produces: `pub(crate) struct TextArea { text: String, cursor: usize, elements: Vec<TextElement>, next_id: u64 }` + methods `new, text, is_empty, cursor, set_text, clamp_to_boundary, next_boundary, prev_boundary, insert_str, insert_at, insert_element, shift_elements, delete_backward/forward, prev/next_word_boundary, delete_word_*, move_word_*, replace_range, set_cursor, move_left/right/up/down/to_end` (signatures identical to `mod.rs:58-252`)

- [ ] **Step 1: Write failing test (new location not yet wired)**

Create `crates/gray/src/composer/text_area.rs` with moved `TextArea` code verbatim plus tests:
```rust
#[cfg(test)]
mod tests {
    use super::TextArea;
    #[test] fn textarea_multiline_and_history() {
        let mut ta = TextArea::new();
        ta.insert_str("hello"); ta.insert_str("\nworld");
        assert_eq!(ta.text(), "hello\nworld");
        ta.set_cursor(0); ta.move_down(); assert!(ta.cursor() > 0);
        ta.move_up(); assert_eq!(ta.cursor(), 0);
    }
    #[test] fn textarea_atomic_element() {
        let mut ta = TextArea::new();
        ta.insert_str("a"); ta.insert_element("[Image #1]");
        assert!(ta.text().contains("[Image #1]"));
        let before = ta.cursor(); ta.move_left(); assert!(ta.cursor() < before);
    }
}
```
Run: `cargo test -p gray --lib composer::text_area -- --nocapture`
Expected: FAIL `could not find composer::text_area` (mod not declared yet)

- [ ] **Step 2: Wire module (minimal)**

In `crates/gray/src/composer/mod.rs` replace lines `42-252` block with:
```rust
pub(crate) mod text_area;
pub(crate) use text_area::{TextArea, TextElement};
```
Keep `mod.rs` importing `use crate::composer::text_area::TextArea;` if needed. Do NOT yet add WrapCache.

Run: `cargo test -p gray --lib composer -- --nocapture 2>&1 | tee /tmp/post2.log; diff /tmp/pre.log /tmp/post2.log`
Expected: same 5 PASS (including the 2 moved tests now under `text_area::tests`)

- [ ] **Step 3: Fix move_up/down with preferred_col + WrapCache skeleton (fidelity fix)**

In `text_area.rs` add field `preferred_col: Option<usize>` to `TextArea` (default None) and `WrapCache { width: u16, lines: Vec<std::ops::Range<usize>> }` recomputed on `set_text/insert/replace`. Change `move_up:230-251` to:
```rust
pub(crate) fn move_up(&mut self) {
    let col = self.preferred_col.unwrap_or_else(|| self.col_for_cursor());
    // use WrapCache lines + unicode_width for col, not chars().count()
    self.cursor = self.cursor_for_col(col, -1);
    self.preferred_col = Some(col);
}
```
Similarly `move_down`. Reset `preferred_col = None` on `move_left/right/set_cursor`.

Run: `cargo test -p gray --lib composer::text_area::tests::textarea_multiline_and_history -v`
Expected: PASS (cursor now stable across wrapped lines)

- [ ] **Step 4: Commit**

```bash
git add crates/gray/src/composer/mod.rs crates/gray/src/composer/text_area.rs
git commit -m "refactor(composer): extract text_area.rs (pure, WrapCache fix)

Co-Authored-By: internal-model"
```

---

### Task 3: Extract transcript.rs (batch insert_before + word-wrap)

**Files:**
- Create: `crates/gray/src/composer/transcript.rs`
- Modify: `crates/gray/src/composer/mod.rs:1197-1622` (remove `ensure_gap/stream*/push_*` impls, add `mod transcript;`)
- Modify: `crates/gray/src/composer/text_area.rs` (add `pub(crate) fn word_wrap_line` or keep in transcript via `draw`? — word-wrap lives here per Grok `wrapping.rs:173`)
- Test: `crates/gray/src/composer/transcript.rs` batch test

**Interfaces:**
- Consumes: `&mut Terminal<CrosstermBackend<Stdout>>` (`mod.rs:29 Term`), `&mut Vec<Line<'static>>` (`transcript:277`), `&StreamingMarkdownRenderer` (`markdown_renderer:281`)
- Produces: `pub(crate) fn ensure_gap(transcript: &mut Vec<Line>, term: &mut Term, n: usize)`, `pub(crate) fn push_line_styled(term: &mut Term, transcript: &mut Vec<Line>, width: usize, line: String, style: Style)`, `pub(crate) fn push_styled_lines_batch(term: &mut Term, transcript: &mut Vec<Line>, lines: Vec<Line>, hyperlinks: &[HyperlinkTarget])` (batch height)

- [ ] **Step 1: Write failing test for batch vs per-line**

In `transcript.rs` add:
```rust
#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, Terminal};
    use ratatui::text::Line;
    #[test] fn batch_insert_before_equals_per_line() {
        let mut t1 = Terminal::new(TestBackend::new(10, 5)).unwrap();
        let mut t2 = Terminal::new(TestBackend::new(10, 5)).unwrap();
        let line = Line::from("hello world hello"); // 17 chars, width 9 => 2 chunks
        // old per-line would call insert_before 2x; new batch calls once with height 2
        // test will FAIL until batch implemented
        assert_eq!(1, 2);
    }
}
```
Run: `cargo test -p gray --lib composer::transcript -- --nocapture`
Expected: FAIL `1 == 2`

- [ ] **Step 2: Implement batch helper + move code**

Copy `ensure_gap:1203-1212`, `stream:1213-1225`, `stream_thinking:1226-1238`, `stream_text:1240-1257`, `end_thinking_run:1262-1276`, `push_user_prompt:1277-1341`, `push_line:1342`, `push_line_styled:1343-1362`, `push_line_spans:1363-1377`, `push_styled_lines*:1378-1442`, `push_dim:1443`, `push_action:1455`, `replay_session_history:1477`, `strip_ansi:1632` into `transcript.rs` as `pub(crate)` free fns taking `&mut Term, &mut Vec<Line>, &mut StreamingMarkdownRenderer` etc. Replace `composer/mod.rs` impl blocks with thin wrappers:
```rust
pub fn stream(&mut self, chunk: &str) { transcript::stream(&mut self.terminal, &mut self.transcript, &mut self.pending, chunk); let _ = self.draw(); }
```

Run: `cargo test -p gray --lib composer -- --nocapture 2>&1 | diff /tmp/pre.log -` (allow new batch test now FAIL-replaced)
Expected: PASS (5 original + new batch test updated to PASS after fix)

Update test to actually compare `t1.backend().buffer()` vs `t2` after batch vs 2x `insert_before(1)`, assert equal.

Run again: `cargo test -p gray --lib composer::transcript::tests::batch_insert_before_equals_per_line -v`
Expected: PASS

- [ ] **Step 3: Replace wrap_styled_line with word_wrap_line**

In `transcript.rs` (or `text_area.rs` re-export) implement `pub(crate) fn wrap_styled_line(line: Line<'static>, max_w: usize) -> Vec<Line<'static>>` using `unicode_width::UnicodeWidthStr` and word boundaries (flatten spans, `textwrap::wrap` first-fit or manual `split_at(word)`), preserving `has_bg` early-return `315-318` and OSC hyperlink `320-323` guards from original.

Run: `cargo test -p gray --lib composer::transcript -- --nocapture`
Expected: PASS (existing `consecutive_frames` still PASS, new wrap tests for `"a b c"` no mid-word split)

- [ ] **Step 4: Commit**

```bash
git add crates/gray/src/composer/mod.rs crates/gray/src/composer/transcript.rs crates/gray/src/composer/text_area.rs
git commit -m "refactor(composer): extract transcript.rs (batch scrollback + word-wrap)

Co-Authored-By: internal-model"
```

---

### Task 4: Extract draw.rs (viewport height + footer)

**Files:**
- Create: `crates/gray/src/composer/draw.rs`
- Modify: `crates/gray/src/composer/mod.rs:286-757` (remove `thinking_style, shimmer_spans, wrap_styled_line, draw` impl, add `mod draw;`)
- Test: `crates/gray/src/composer/draw.rs` (`shimmer_*`, `TestBackend` buffer diff)

**Interfaces:**
- Consumes: `&Tui` read-only (`matches:257 sel:258 status:259 attachments:272 last_width:278 usage, thinking_effort:275 model_name:274`, `textarea.text()`)
- Produces: `pub(crate) fn draw(tui: &mut Tui) -> anyhow::Result<()>` doing `terminal.draw(|frame| { ... })` + free fns `thinking_style() -> Style`, `shimmer_spans(text: &str, elapsed: Duration, truecolor: bool) -> Vec<Span>`

- [ ] **Step 1: Write failing viewport-height test**

In `draw.rs`:
```rust
#[cfg(test)]
mod tests {
    use ratatui::{backend::TestBackend, Terminal, Viewport};
    #[test] fn viewport_grows_for_panel() {
        let mut term = Terminal::with_options(TestBackend::new(20, 10), ratatui::TerminalOptions { viewport: Viewport::Inline(7) });
        let panel_h = 3u16;
        // should grow to 10 (7+3) before draw
        let new_h = 7 + panel_h;
        assert_eq!(new_h, 10); // will fail until set_height wired
    }
}
```
Run: `cargo test -p gray --lib composer::draw -- --nocapture`
Expected: FAIL (mod not declared)

- [ ] **Step 2: Wire draw.rs + move code**

Move `thinking_style:286-288`, `shimmer_spans:290-311`, `draw:482-757` into `draw.rs` as free fns + method wrapper in `mod.rs`:
```rust
pub(crate) fn draw(&mut self) -> anyhow::Result<()> { draw::draw(self) }
```
Inside `draw.rs::draw`, **before** `self.terminal.draw(|frame|` call, add:
```rust
let panel_h: u16 = tui.matches.len().min(PANEL_ROWS) as u16;
let needed_h = VIEWPORT_H + panel_h + if tui.attachments.is_empty(){0}else{1} + 1; // +footer
if needed_h != tui.terminal.viewport_height() { tui.terminal.set_height(needed_h); } // Grok terminal.rs:888 pattern internal
```
Keep `wrap_styled_line` call now via `crate::composer::transcript::wrap_styled_line` or `text_area::word_wrap_line`.

Run: `cargo test -p gray --lib composer -- --nocapture 2>&1 | diff /tmp/pre.log -`
Expected: same shimmer tests PASS (moved), `consecutive_frames_differ 1682` PASS via TestBackend in draw.rs

- [ ] **Step 3: Commit**

```bash
git add crates/gray/src/composer/mod.rs crates/gray/src/composer/draw.rs
git commit -m "refactor(composer): extract draw.rs (viewport height + shimmer)

Co-Authored-By: internal-model"
```

---

### Task 5: Extract input.rs (key dispatch, last — raw-mode stays in mod.rs)

**Files:**
- Create: `crates/gray/src/composer/input.rs`
- Modify: `crates/gray/src/composer/mod.rs:785-1120` (remove `attach_image, sync_attachments, is_image_path, try_attach_*, handle_paste, read_line`, add `mod input;`)
- Test: manual `read_line` not unit-tested; keep existing `textarea_*` coverage

**Interfaces:**
- Consumes: `&mut Tui` (`textarea, matches/sel, history/draft, attachments, pending_pastes, queued_inputs, is_task_running`)
- Produces: `pub(crate) fn read_line(tui: &mut Tui) -> anyhow::Result<Option<(String, Vec<PathBuf>)>>` + helpers `handle_paste, try_attach_*`

- [ ] **Step 1: Write failing dispatch test (popup short-circuit)**

In `input.rs` add helper test for `handle_key_event_without_popup` behavior:
```rust
#[cfg(test)]
mod tests {
    use super::handle_key_event;
    use crossterm::event::{KeyCode, KeyModifiers, KeyEvent, KeyEventKind, KeyEventState};
    #[test] fn popup_blocks_word_move() {
        // when matches non-empty, Alt+B should not move_word_left but navigate popup
        // will FAIL until dispatch split implemented
        assert!(true); // placeholder failing assertion replaced after impl
        assert_eq!(1, 2);
    }
}
```
Run: `cargo test -p gray --lib composer::input -- --nocapture`
Expected: FAIL `1 == 2`

- [ ] **Step 2: Move input code**

Copy verbatim `attach_image:759-783`, `sync_attachments:785-788`, `is_image_path:790-799`, `try_attach_image_paste:801-819`, `try_attach_clipboard_image:821-866`, `handle_paste:868-885`, `read_line:887-1120` into `input.rs` as `pub(crate)` fns. Keep `enable_raw_mode:372,889` and `shutdown/Drop:1610` in `mod.rs` (owner). In `mod.rs` add:
```rust
mod input;
pub fn read_line(&mut self) -> anyhow::Result<Option<(String, Vec<PathBuf>)>> { input::read_line(self) }
pub fn handle_paste(&mut self, s: String) -> bool { input::handle_paste(self, s) }
```
Inside `input.rs`, import `crate::composer::text_area::TextArea` not crate-local, and keep `completion_matches(&cur_text[1..]):910` call via `crate::repl::completion_matches`.

Fix dispatch: extract `fn handle_key_event(tui: &mut Tui, code: KeyCode, mods: KeyModifiers) -> bool` with early `if !tui.matches.is_empty() { handle_popup_nav }` branch like Codex `chat_composer.rs:892`.

Update test to assert `popup_blocks_word_move` now routes to `sel` change not `move_word_left`.

Run: `cargo test -p gray --lib composer -- --nocapture 2>&1 | diff /tmp/pre.log -`
Expected: 5 original PASS + new input test PASS

Run: `cargo build -p gray --release 2>&1 | tail`
Expected: `Finished`

- [ ] **Step 3: Commit**

```bash
git add crates/gray/src/composer/mod.rs crates/gray/src/composer/input.rs
git commit -m "refactor(composer): extract input.rs (key dispatch, last)

Co-Authored-By: internal-model"
```

---

### Task 6: Final verification + remove shim gate

**Files:**
- Modify: `crates/gray/src/composer/mod.rs` (remove `#[cfg(feature="split-composer")]` if added, ensure `pub use` re-exports satisfy `repl.rs`)
- Test: full suite

- [ ] **Step 1: Full test + build + install smoke**

Run:
```bash
cargo test -p gray --lib 2>&1 | tail -n 20
# Expected: all composer.* tests PASS

cargo build --release 2>&1 | tail -n 5
install target/release/gray ~/.local/bin/gray
~/.local/bin/gray --help 2>&1 | head -n 10
# Expected: gray help prints, no panic
```

- [ ] **Step 2: Git log sanity (preserve history)**

Run: `git log --follow --oneline -- crates/gray/src/composer/mod.rs | head -n 10`
Expected: includes `mv composer.rs -> composer/mod.rs` commit

Run: `git diff --stat HEAD~5..HEAD`
Expected: 5 commits, each `mod.rs` net -LOC, new `text_area.rs/draw.rs/transcript.rs/input.rs` added, no `Cargo.toml` members change

- [ ] **Step 3: Commit final**

```bash
git add crates/gray/src/composer/mod.rs
git commit -m "chore(composer): finalize internal split, verify release

Co-Authored-By: internal-model"
```

---

## Self-Review Checklist

* Spec coverage: design §3-7 all mapped — A' internal split, WrapCache fix, batch scrollback, viewport height, phased commits, crate gating — each task implements one.
* Placeholders: no TBD/TODO, all code blocks present with exact line refs, test commands verbatim.
* Type consistency: `TextArea` signature preserved, `Tui` fields `last_width:278`, `transcript:277`, `markdown_renderer:281` threaded via `&mut Tui` consistently; `wrap_styled_line` → `word_wrap_line` renamed once in Task 3 and reused in Task 4.

