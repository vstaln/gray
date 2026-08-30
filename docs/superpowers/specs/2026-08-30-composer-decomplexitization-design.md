# Composer Decomplexitization — Design (Grok-lean internal split, crate-gated)

**Date:** 2026-08-30
**Path:** Bounded→Architectural (upgrade on choosing B faithful clone)
**Choice:** A' — Grok-lean internal split now, crates gated for later (addresses fidelity + migration critiques)
**File in scope:** `crates/gray/src/composer.rs:1-1700` (1700 LOC, 15% of `crates/gray/src`)

## 1. Context

`composer.rs` is a god-file: `TextArea:42-252` + `Tui:254-284` (28 fields) + `draw:482-757` + `read_line:887-1120` + `transcript/stream:1197-1540` share `&mut Tui`. Reference shows split:

* Codex `reference/openai/codex/codex-rs/tui/src/bottom_pane/` — 48,259 LOC / 42 files; `chat_composer.rs:12997` + `textarea.rs:4518` + `chat_composer/*.rs` (7 submodules) + `footer.rs:2075` etc. Demonstrates enterprise bloat to avoid copying.
* Grok `reference/xai-org/grok-build/crates/codegen/xai-ratatui-inline/src/{terminal.rs,scrollback.rs,resize.rs}` + `xai-ratatui-textarea/src/{textarea.rs,wrapping.rs,editor.rs}` + `xai-grok-pager/src/{input/,scrollback/,views/status_line/}` — lean crate boundaries to emulate.

Goal: decomplexitize without 2-crate over-engineering or big-bang breakage of `repl.rs:1043` + raw-mode lifecycle.

## 2. Architecture

No new crates now. Facade preserves public API:

```
crates/gray/src/composer/mod.rs      # re-exports pub struct Tui, SharedTui; owns Terminal + raw-mode Drop
crates/gray/src/composer/text_area.rs # TextArea + wrapping (was 42-252 + 313-369)
crates/gray/src/composer/draw.rs      # shimmer + draw + footer
crates/gray/src/composer/input.rs     # handle_paste/attach + read_line dispatch
crates/gray/src/composer/transcript.rs # ensure_gap + stream* + push_* + hyperlink batch
crates/gray/src/composer/state.rs     # optional: Tui field defs if mod.rs grows >300 LOC else inline
```

`Cargo.toml` workspace `members` unchanged. `crates/gray/src/composer.rs` becomes 5-line shim for one release:
```rust
pub use crate::composer::Tui;
```
via `mod composer;` auto-resolving to `composer/mod.rs`. Zero `install target/release/gray` change. Crate extraction (`gray-ratatui-textarea`, `gray-ratatui-inline`) gated behind second-consumer or 500-LOC threshold.

## 3. Components & Moves

* `text_area.rs` ← `52-252` `TextElement/TextArea` verbatim + `wrap_styled_line:313-369` replaced by Grok `xai-ratatui-textarea/src/wrapping.rs:173` `word_wrap_line` (word-wrap, `unicode_width`, `slice_line_spans` style preservation). Add `WrapCache + preferred_col` for `move_up/down:230-251` (fix `chars().count()` bug). Keep `O(n) scan, no grapheme crate` ceiling.
* `draw.rs` ← `286-312` `thinking_style/shimmer_spans` + `482-757` `draw()`. Change: compute `panel_h` then `viewport.set_height(base + panel_h)` before `terminal.draw()` (Grok `terminal.rs:888-942` pattern) instead of fixed `VIEWPORT_H=7:25` overflow. Footer `696-742` (`ctx/cache/model/effort`) stays here; hyperlinks rendered via viewport link table not `Cell.symbol:1431` OSC injection.
* `input.rs` ← `785-930` `is_image_path/try_attach_*/attach_image/sync_attachments/handle_paste` + `887-1120` `read_line`. Keep key priority order but split `handle_key_event_without_popup` (Codex `chat_composer.rs:892` pattern) so `matches:257 sel` popup short-circuits word moves. Owns `history:269/draft` navigation.
* `transcript.rs` ← `1197-1622` `ensure_gap, stream/stream_thinking/stream_text/end_thinking_run, push_user_prompt/box_lines, push_line_styled/push_styled_lines_with_hyperlinks, push_dim/push_action, replay_session_history`. Batch `insert_before(height, |buf| render)` via `scrollback.emit_to_scrollback` (one call per markdown batch, not per `chars.chunks` line: `1352`). Keep `markdown_renderer:281 + committed_markdown_lines:282` invariant.
* `mod.rs` / `state.rs` ← `254-284` `Tui` fields + `32-40` `SharedTui` + `372-383` `new() Viewport::Inline`, `372-374` raw-mode/bracketed-paste owner, `1610-1630` `shutdown/Drop`, `1586-1609` `tick_status` (single `pending_resize:283` source shared via `&mut`).

Visibility: `pub(crate)` only; `draw/input/transcript` take `&Tui`/`&mut Tui` not traits.

## 4. Data Flow

*Render:* `new()` → `InlineViewport` → `draw()` pre-computes `box_h/panel_h/attach_h/footer` → `set_height` → `terminal.draw(frame)` → `frame.set_cursor_position` `752-754`. Scrollback batches preserve ANSI.

*Input:* `read_line` poll 250ms → `Resize → pending_resize` → `Paste → handle_paste` → `Ctrl-C/D → Ok(None)` → `Esc → clear matches` → `Alt/Ctrl word moves` → `Enter` (slash-complete `983` vs `push_user_prompt` vs `queued_inputs:1005`) → `Tab/Char/Backspace/Delete/Left/Right/Up/Down` (popup-nav vs history vs `move_up/down`). `sync_attachments` after deletes.

*Streaming:* `begin_turn → stream_thinking (hide_thinking gate) → stream (pending \n split) / stream_text (StreamingMarkdownRenderer → frozen_len → batch) → end_thinking_run spacer → end_turn flush pending + markdown.finish_into_output + elapsed·tok dim line 1184`.

## 5. Error Handling & Lifecycle

* Raw-mode/bracketed-paste single owner in `mod.rs` (fixes brownfield Drop-order race). `input.rs` never calls `crossterm::terminal::*` directly.
* Resize debounce `RESIZE_DEBOUNCE 75ms:27` canonical in `mod.rs`, passed `&mut pending_resize` to `input` + `tick_status`.
* Clipboard fallbacks `821-865` (`arboard → image::RgbaImage → tempfile → wl-paste/xclip`) stay in `input.rs`, keep `workspace.dependencies: arboard 3, image 0.25, tempfile`.
* Markdown `committed_markdown_lines` + `transcript.len()>1000 drain 100` invariant stays in `transcript.rs`.

## 6. Testing

* Move tests with code: `textarea_multiline_and_history, textarea_atomic_element 1657` → `text_area.rs`; `shimmer_spans_change 1640` → `draw.rs`; `consecutive_frames_differ_in_test_backend 1682` → `draw.rs`.
* Integration: `cargo test -p gray --lib composer -- --nocapture` diff vs `/tmp/pre.log` after each phase; `cargo build --release` smoke.
* TDD per `writing-plans` tasks: each phase has failing `TestBackend` wrap/resize test before code move.

## 7. Rollout (green every commit)

1. `git mv crates/gray/src/composer.rs crates/gray/src/composer/mod.rs` (rename-only)
2. Extract `text_area.rs` (pure, no Term)
3. Extract `transcript.rs` (word-wrap + batch `insert_before`)
4. Extract `draw.rs` (viewport height)
5. Extract `input.rs` last (highest risk) — behind `#[cfg(feature="split-composer")]` until green, then remove shim.

Crate gating: only after 1 release green, move `text_area.rs` → `crates/gray-ratatui-textarea` with `pub use` shim if second consumer lands.

## 8. Non-goals

* No vim/kill-ring/unicode-segmentation vendor (`textarea.rs:4518` B2 deferred)
* No mentions/file-search/skill popups, `mentions_v2/*`
* No `scrolling-regions` tmux opt, no `textwrap` hyphen/OptimalFit
