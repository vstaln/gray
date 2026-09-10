# Ponytail Cut Batches — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Delete ~6,000 lines of dead/over-built code and ~12 unused deps from this workspace without changing behavior.

**Architecture:** Deletion-first. Each task removes one coherent verified-dead unit (or merges a verified duplicate), then runs that crate's tests. Phases are ordered safest → riskiest so execution can stop at any phase boundary with the tree green. Four product decisions are gated in Task 0; if unanswered, skip those items.

**Tech Stack:** Rust 2024 workspace (`crates/gray*`), cargo, gh CLI.

**Spec:** ponytail audit report, 2026-09-10 (this conversation). Supersedes the untracked stale `docs/superpowers/plans/ponytail-cuts.md` where they conflict — notably: `crates/gray-core/src/redaction.rs` is LIVE (`gray-tools/src/shell/pump.rs`, `gray/src/print.rs`, `gray-gateway/src/daemon_agent.rs`); never delete it. Delete that stale plan file once this one is accepted.

## Global Constraints

- **Branch:** PR #54 (`fix/batch-release-readiness`) is open. Check `gh pr list --state open` first. If open → all cuts land there. If merged/closed → create ONE branch `feat/batch-ponytail-cuts` from `main` only after `git branch -a` shows no other unmerged work branch. Never a second PR.
- **Delete means delete.** No stubs, no `pub use` shims, no `#[allow(dead_code)]` re-exports, no compat.
- **Caller check before every cut:** grep the symbol workspace-wide (`crates/ plugins/ examples/ dogfood/`). A production caller outside tests → skip the item, report `DONE_WITH_CONCERNS` with the caller path, continue with the rest of the task.
- **Tests first, then delete:** run the covering test once green BEFORE the cut; after the cut the same command must be green. A deletion that turns a test red is a behavior change — revert that item.
- Per task: `cargo fmt --check` + `cargo clippy -p <crates> -- -D warnings` + `cargo test -p <crates>`. Phase gate: `cargo test --workspace --quiet` + `cargo clippy --workspace -- -D warnings`.
- Untracked WIP files (`read/image.rs`, `read/notebook.rs`, `bash.rs`, `gray-plugin/src/boot.rs`): `rm` them; nothing to commit.
- Commit style: `chore(cut): <what>`. One commit per task.
- `ponytail:` comment on any deliberately deferred simplification (e.g. kept LRU) naming the ceiling + upgrade path.

---

## Task 0: Product decision gates (ask before executing)

Four items are DELIBERATE forward seams or parked product work. Ask the user; default = SKIP.

- [ ] `gray-extras` orphan crate (1,695 lines): docs/cli-architecture-audit.md says "migrate OAuth, delete proxy + unwired webfetch, then remove crate" (001 product-cut). Ask: delete now, migrate OAuth first, or keep parked?
- [ ] `gray-core/src/turn_queue.rs` admission seam (~140): docs/superpowers/plans/2026-09-07-codex-turn-queue.md is staged work. Delete only if that wave is dead.
- [ ] `gray-markdown` `CodeBlockSpan`/`code_blocks` (~240 + ~390 test): looks like deliberately vendored upstream pager API. Zero consumers today. Cut or keep as public surface?
- [ ] `gray-plugin/src/boot.rs` (untracked, uncompiled): intended to land (wire it) or abandoned (rm)? Default rm per Phase 1.

Record answers in this file before starting Phase 1.

---

## Phase 1: Stale / uncompiled files

### Task 1: Delete files that never compile

**Files:**
- Delete (tracked): `crates/gray-core/src/compact_v2.rs`
- Delete (untracked): `crates/gray-tools/src/read/image.rs`, `crates/gray-tools/src/read/notebook.rs`, `crates/gray-tools/src/bash.rs`, `crates/gray-plugin/src/boot.rs`, `crates/gray-plugin/tests/boot.rs`

- [ ] Verify stale: `rg -n 'compact_v2' crates/` → 0 hits. `rg -n 'mod bash' crates/gray-tools/src/lib.rs` → 0 hits. `rg -n 'pub mod boot|mod boot' crates/gray-plugin/src/lib.rs` → 0 hits. `rg -n '^pub mod (image|notebook)' crates/gray-tools/src/read/mod.rs` → 0 hits.
- [ ] `git rm crates/gray-core/src/compact_v2.rs`
- [ ] `rm crates/gray-tools/src/read/image.rs crates/gray-tools/src/read/notebook.rs crates/gray-tools/src/bash.rs crates/gray-plugin/src/boot.rs crates/gray-plugin/tests/boot.rs`
- [ ] Run: `cargo test -p gray-core -p gray-tools -p gray-plugin`
- [ ] Commit: `chore(cut): delete stale uncompiled files (compact_v2, unwired read WIP, boot harness)`

---

## Phase 2: Unused deps and no-op config (all verified 0 refs)

### Task 2: Drop unused dependency edges

**Files:** `crates/{gray-tools,gray-provider,gray-session,gray-supervise,gray-acp,gray-markdown,gray,gray-gateway,gray-pkg}/Cargo.toml`, root `Cargo.toml`

- [ ] Verify each with `rg -c '\b<dep>\b' crates/<crate>/src crates/<crate>/tests` → 0:
  - gray-tools: `anyhow`, `thiserror`
  - gray-provider: `thiserror`
  - gray-session: `futures`
  - gray-supervise: `tokio`
  - gray-acp: `log`
  - gray-markdown: `anstyle_syntect`
  - gray → `gray-provider`, gray-gateway → `gray-provider` (path edges; `gray_provider` refs = 0)
- [ ] Remove those `[dependencies]` lines (and the two workspace path edges). Do NOT remove shared `[workspace.dependencies]` entries still used elsewhere.
- [ ] Remove gray's duplicated `[dev-dependencies]` lines already present in `[dependencies]`: `async-trait`, `reqwest`, `tempfile`.
- [ ] No-op specs: root `log` `features = ["std"]` (default feature); root `gray-gateway` `default-features = false` (its default is `[]`).
- [ ] gray-pkg: delete the `adapter-gray-native` feature and `pub mod adapter;` line (module deleted in Task 19).
- [ ] Run: `cargo check --workspace --all-features` then `cargo test --workspace --quiet`
- [ ] Commit: `chore(cut): drop 8 unused deps, 2 path edges, no-op feature specs`

### Task 3: fs2 → std file locks

**Files:** `crates/gray-gateway/src/lock.rs:49,65,71`, `crates/gray-cron/src/store.rs:17,173`, both Cargo.toml

- [ ] `rg -n 'fs2' crates/` — replace only gateway + cron sites; leave other crates for their own task.
- [ ] Swap `fs2::FileExt::{lock_exclusive,try_lock_exclusive,unlock}` → `std::fs::File::{lock,try_lock,unlock}`; `try_lock` error is `std::fs::TryLockError::WouldBlock`. Requires stable ≥1.89 (CI tracks stable).
- [ ] Run: `cargo test -p gray-gateway --features all-platforms -p gray-cron`
- [ ] Commit: `chore(cut): replace fs2 with std file locks`

---

## Phase 3: gray-markdown cuts (except gated code_blocks)

### Task 4: Delete dead markdown subsystems + wrapper layers

**Files:** `crates/gray-markdown/src/{colors,checkpoint,streaming,output,buffers,render,parse,style,latex_delimiters,url_scan,table_records,lib}.rs`

- [ ] `delete` polarity-safe color subsystem + color cap: `POLARITY_SAFE_SYNTAX`, `COLOR_LEVEL_CAP`, `set_polarity_safe_syntax`, `polarity_safe_syntax`, `set_color_level_cap`, `color_level_cap`, `adapt_color_polarity_safe`, `polarity_safe_syntax_ansi`, `ansi256_to_rgb` (only that path); `get_color_level` → `detect_color_level()`. [colors.rs:132-307, lib.rs:49-52] (-145)
- [ ] `delete` forced-checkpoint policy: `FORCED_CHECKPOINT_*`, `should_force_checkpoint`, `frozen_at` field + updates, the empty-bodied `else if` in `rerender_tail`. [checkpoint.rs:60-151, streaming.rs] (-65)
- [ ] `delete` dead `StreamingMarkdownRenderer` methods: `source`, `frozen_bytes`, `frozen_lines_count` (dup of `frozen_lines_len`), `set_style`, `set_pretty`, `pretty`, `into_output`, `clear`, `push`. [streaming.rs] (-70)
- [ ] `yagni` drop `collapse_soft_breaks` plumbing (always true): keep collapse inline in `Event::SoftBreak`. [parse.rs, streaming.rs, lib.rs] (-35)
- [ ] `delete` `try_push_mermaid` stub (always false) + caller guard. [parse.rs:539-551] (-19)
- [ ] `shrink` `MarkdownRenderView` + `as_view` + unused `line_count` → return `&MarkdownRenderOutput`. [output.rs:112-148] (-30)
- [ ] `delete` `MarkdownBuffers.render_events` field; merge one-caller `build_render_events_into` into `build_render_events`. [buffers.rs, render.rs] (-12)
- [ ] `delete` `render_markdown_ratatui` simple API (tests use `_full`). [lib.rs:150-159] (-10)
- [ ] `yagni` `TableBorders::new` + `Default` (only `BOX` used). [style.rs:33-77] (-10)
- [ ] `shrink` `StyleInto` trait → free `fn anstyle_to_ratatui_style`. [colors.rs:340-381] (-6)
- [ ] `delete` `LatexDelimiterNormalizer::{reset,Default}` (only reachable from deleted `clear`). [latex_delimiters.rs] (-11)
- [ ] `shrink` `patch_lines_with_link_style` → `apply_link_styling` (single caller passes `link_style()`). [url_scan.rs] (-4)
- [ ] `shrink` `ColumnMetrics` → `Vec<ColumnKind>`; drop no-op `metrics.get(col)`. [table_records.rs] (-8)
- [ ] `shrink` `unicode_display_width` LRU cache → `s.width()` with `// ponytail: dropped LRU; re-add if profiling shows width() hot`. [buffers.rs:239-295] (-55)
- [ ] If Task 0 approved: also delete `code_blocks` machinery (`PendingCodeBlock`, `build_code_block_spans`, `output.code_blocks`, `view.code_blocks`, rebase, re-export) + its test module. [output.rs, parse.rs, streaming.rs, buffers.rs, render.rs, lib.rs] (-240, -390 tests)
- [ ] Run: `cargo test -p gray-markdown`
- [ ] Commit: `chore(cut): strip dead markdown subsystems and wrappers`

---

## Phase 4: gray-tools cuts

### Task 5: Delete test-only and dead tool surface

**Files:** `crates/gray-tools/src/read/{window,stream,hygiene,notices,args}.rs`, `src/find.rs`, `src/edit_diff.rs`, `src/stats.rs`, `src/shell/contract.rs`, `src/shell/tools/sleep.rs`

- [ ] `delete` test-only production surface: `window::{clamp_line,clamp_lines,prefix_lines}`, `stream::count_rest`, `stream::{cancelled_note,count_skipped_total}`, `hygiene::{prepare,normalize_newlines}`, `notices::read_failed`. Prod call sites use `notices::*`/`clamp_counted` directly. Move nothing to cfg(test) that is already covered by tests/read_zoo — just delete. (-170)
- [ ] `delete` `GlobTool` + its `Registry::builtin()` registration (production registers Find/Ls/Grep only). [find.rs:51,116, lib.rs:69] (-90)
- [ ] `delete` `READ_ARG_ALIASES`, `canonical_name`, `is_limit_zero` (never wired). [read/args.rs] (-45)
- [ ] `delete` `generate_unified_patch_default` and test-only `Edit::{new,with_line_hint,with_occurrence,with_replace_all}`. [edit_diff.rs] (-35)
- [ ] `delete` write-only fields masked by `#![allow(dead_code)]`: `WaitMode`, `Spawned.start_ticks`, `Task.pattern_strikes`, `KillReport.pid/method`, `ExitReport.code/signal/benign`, `RawLine.overflow_bytes/had_newline`, `LineStream::display()` + field. [shell/contract.rs, spawn.rs, kill.rs, exit.rs, read/stream.rs] (-60)
- [ ] `yagni` drop env overrides `GRAY_READ_MAX_LINES/BYTES/LINE_CHARS` + `env_usize`/`max_*` indirection. [read/window.rs] (-20)
- [ ] `delete` `sleep` `reason` arg (parsed then discarded). [shell/tools/sleep.rs] (-14)
- [ ] `delete` `CUT_CLAMP`; `ToolStats.notice` (single caller sets "none"). [stats.rs] (-8)
- [ ] `shrink` duplicate private `get_opt_str` in bash + shell_output → hoist beside `get_str`. [shell/tools] (-8)
- [ ] `shrink` `find.rs` error nest (3× code/empty checks) into one `is_some_and`. (-12)
- [ ] `shrink` drop `stream::MAX_LINE_CHARS` test-only re-export; inline `window::MAX_LINE_CHARS`. (-3)
- [ ] Run: `cargo test -p gray-tools`
- [ ] Commit: `chore(cut): remove dead tool surface and test-only exports`

### Task 6: Dedupe hand-rolled tool machinery

**Files:** `crates/gray-tools/src/shell/split.rs`, `src/read/bulk.rs`, `src/grep.rs`, `src/{find,grep,ls,write,edit}.rs`, `src/lib.rs`

- [ ] `shrink` merge three quote-aware scanners `subshell_inners`, `process_subst_inners`, `backtick_inners` → one parameterized scanner (trigger-token list). [shell/split.rs] (-100)
- [ ] `native` `bulk.rs` hand-rolled glob/gitignore (`matches_segment`, `matches_segs`, `matches_pattern`, `read_gitignore`, `is_gitignored`, `walk_files`) → `globset::GlobBuilder` + `ignore::WalkBuilder` as `find.rs::fallback_walk` already does (both workspace deps). Verify byte-parity for trailing-slash + basename rules with existing tests. (-90)
- [ ] `shrink` `grep.rs` context mode manual whole-file cache → pass `--context N` to the already-spawned `rg` and format its context events. (-45)
- [ ] `shrink` 4× notice-join block → `append_notices(&mut output, &notices)` in `truncate.rs`. [find.rs ×2, grep.rs, ls.rs] (-30)
- [ ] `shrink` write/edit top-level alias fallback chains → rely on central `ALIASES` in `lib.rs`; drop duplicated schema alias props (keep `edits[]` handling). [write.rs:183-208, edit.rs:170-199] (-40)
- [ ] Run: `cargo test -p gray-tools`
- [ ] Commit: `chore(cut): dedupe scanners, glob matching, grep context, aliases`

---

## Phase 5: gray-core / gray-session / gray-provider

### Task 7: Delete staged-but-unused core surface

**Files:** `crates/gray-core/src/{approvals,agent,agent_loop,compact}.rs`, `crates/gray-session/src/lib.rs`, `crates/gray-provider/src/{lib,openai,testutil}.rs`

- [ ] `delete` session-title subsystem: `set_user_title`, `set_auto_title`, `set_title_inner`, `next_title_in_lineage`, `sanitize_title`, `derive_title`, `title`/`title_source` fields. Callers only in this file's tests (verified). (-235)
- [ ] `delete` `resolve_session_id` + its tests (no workspace caller; `load(SessionId::new(raw))` used instead). If a session picker is staged → skip with `DONE_WITH_CONCERNS`. (-90)
- [ ] `delete` `gray-provider/src/testutil.rs` (0 consumers; openai tests use local helpers). (-94)
- [ ] If Task 0 approved: `delete` turn-admission seam (`SubmitMode`, `Submission`, `RejectReason`, `Agent::submit`, `current_turn`, `next_turn_id`, `TurnState::Busy`); fold empty-input check into `run`/`run_streaming`. (-140)
- [ ] `delete` `Decision::AcceptAlways` + unreachable arm (never constructed). [approvals.rs:53,293-308] (-20)
- [ ] `shrink` `agent_loop.rs`: extract one `preflight()` + one error-result pusher shared by `Segment::Parallel` and `Segment::Single`. (-60)
- [ ] `yagni` provider builder knobs set only from tests (`http`, `initial_backoff`, `request_max_retries`, `stream_max_retries`, `stream_idle_timeout`, `prompt_cache_retention`) → hardcode defaults. (-55)
- [ ] `shrink` gray-session locked read/tail/scan duplicated in `append_with_usage_and_duration` + `append_compaction_replacement` → one locked reader. (-40)
- [ ] `stdlib` `split_head_tail` char-boundary loops → `str::{floor_char_boundary,ceil_char_boundary}`. [core/compact.rs] (-12)
- [ ] `delete` dead accessors/ctors: provider `base_url/api_key/model/request_max_retries/stream_max_retries/stream_idle_timeout`, `Agent::{provider,clear_messages,with_max_rounds}`, `OpenAiProvider::new`. (-34)
- [ ] `yagni` drop 10 unused `gray_provider` re-exports (only `OpenAiProvider` used). [lib.rs] (-11)
- [ ] `delete` `append_with_usage` wrapper (only `append` calls it). (-8)
- [ ] Run: `cargo test -p gray-core -p gray-session -p gray-provider`
- [ ] Commit: `chore(cut): remove dead core/session/provider seams and accessors`

---

## Phase 6: gray TUI / REPL cuts

### Task 8: Merge empty_turn into prompt_turn + delete dead composer APIs

**Files:** `crates/gray/src/repl/{empty_turn,mod,prompt_turn,handlers,dispatch,plugin_cmds,session,commands,key_watcher}.rs`, `src/composer/{mod,terminal,text_area}.rs`, `src/composer/question/{panel,session}.rs`, `src/composer/draw/widgets.rs`, `src/composer/transcript/{boxes,rows}.rs`

- [ ] `shrink` `empty_turn.rs` fork: verify `run_prompt_turn` handles empty text + `pending_images` (read both files), then route `ReplCommand::Empty` to `run_prompt_turn(String::new(), …)`, delete `empty_turn.rs` and the redundant minimal `spawn_key_watcher`. If parity fails → skip, `DONE_WITH_CONCERNS`. [repl/mod.rs:90,724] (-350)
- [ ] `shrink` dismissed-modal draft reset copy-pasted 5× → `Tui::clear_draft()`. [handlers.rs:44,349,467, dispatch.rs:274, plugin_cmds.rs:100,149] (-40)
- [ ] `delete` dead `CustomTerminal` accessors: `backend`, `backend_mut`, `current_buffer`, `current_buffer_mut`, `previous_buffer`, `get_cursor_position`, `last_known_cursor_pos` (already `#[allow(dead_code)]`). (-29)
- [ ] `shrink` `option_row` → `option_rows` desc-less path. [question/panel.rs:205-230] (-26)
- [ ] `shrink` `persist_turn_messages` session-mint block → call `ensure_session_state` then append. [repl/session.rs:199-218] (-18)
- [ ] `delete` `input_scroll` + its test (nothing in draw path). [draw/widgets.rs] (-18)
- [ ] `stdlib` `text_area::char_width` CJK range table → `crate::text_width::char_width` (unicode-width). (-12)
- [ ] `delete` `QueuedRequest.resolved` `Arc<AtomicBool>` + `current_resolved` (never read in prod). [question/session.rs] (-12)
- [ ] `delete` `/gateway connect|pairing` redaction arms (gateway left the TUI). [transcript/rows.rs:25-36] (-12)
- [ ] `delete` `Tui::{push_line,push_styled_lines,attach_image}`, `push_tool_box_no_gap`, `cycle_permission_mode`, `format_help_all`. (-42)
- [ ] `delete` `truecolor` field + unreachable 256-color branch; `needs_cron_tick` + dead conditionals. [composer/mod.rs] (-15)
- [ ] `shrink` `option_label_for_index_for` → call `option_label_for_index`; key_watcher Ctrl+w identical if/else arms. (-16)
- [ ] `delete` `TextElement.id`/`next_id`; `push_result_summary._auto` param + computed arg. (-9)
- [ ] Run: `cargo test -p gray --lib` and `cargo check -p gray --all-features`
- [ ] Commit: `chore(cut): merge empty-turn path and strip dead TUI APIs`

### Task 9 (stretch, refactor-risk): Merge skills/plugins manager modals

**Files:** `crates/gray/src/setup/{skills_modal,plugins_modal}.rs`

- [ ] Extract shared chrome/render/keys into one parameterized install-manager (remove-only vs toggle+remove; error verb). ~85% identical. Verify key-by-key parity and that existing modal tests cover both flows; if coverage is thin, add a focused test per flow first. (-300)
- [ ] Run: `cargo test -p gray --lib`
- [ ] Commit: `refactor(cut): unify skills/plugins manager modals`

---

## Phase 7: gray setup / skills / system prompt

### Task 10: Delete dead setup and prompt surface

**Files:** `crates/gray/src/{system_prompt,resume,update,skills_tool,shell_drain}.rs`, `src/skills/mod.rs`, `src/setup/{catalog,effort,context/providers.rs,marketplace_modal,tabs}.rs`, `src/compact/{mod,policy}.rs`, `src/tool_fmt/{mod,plain}.rs`

- [ ] `delete` system-prompt default "pi" branch + `default_selected_tools` + `get_readme_path/get_docs_path/get_examples_path` + `append_system_prompt` field; prod always passes `custom_prompt: Some` — port its tests to that shape. [system_prompt.rs:77,86-93,145-255] (-120)
- [ ] `delete` `LoadSkillsOptions`/`load_skills` explicit-path machinery (`agent_dir`, `skill_paths`, `include_defaults:false`, `user_skill_roots`, `is_under` + loop); `discover_skills` is the only caller. [skills/mod.rs:61-67,549-648] (-100)
- [ ] `shrink` resume filter predicate 3× → one `matches(s, query, cwd_filter)`. [resume.rs:353-377,651-705] (-55)
- [ ] `delete` catalog dead fields: `CatalogProvider.featured`, `models`+`CatalogModel`, `env_key`+`env_hint`, `ConnectItem.category/env_key/oauth_capable`, `OAUTH_CAPABLE`, `normalize_auth_mode`. [setup/catalog.rs] (-55)
- [ ] `delete` update receipts (`write_update_receipt*` + tests) — write-only. If ops wants telemetry → keep and add a reader, else delete. [update.rs] (-50)
- [ ] `delete` dead fns: `resume_command_hint`, `format_skill_invocation`, `IgnoreMatcher::default`, `tab_bar`, `format_tool_result_lines`, `format_tool_result_plain`, test-only `format_market_row` + tests. (-45)
- [ ] `shrink` `resolve_skill_name` → reuse `discover_skills(cwd)` match by name. [skills_tool.rs] (-40)
- [ ] `stdlib` `format_relative` hand-rolled epoch→Y/M/D → `chrono::DateTime::from_timestamp(...).format("%Y-%m-%d")`. [resume.rs:36-72] (-35)
- [ ] `shrink` `SearchFlight`/`SearchRx`/`SearchPoll` → `fn poll(&mut self)`. [marketplace_modal.rs:1056-1095] (-25)
- [ ] `shrink` `Tab` + `MarketTab` duplicated wrapped-index machinery → one enum/index pair. [tabs.rs, marketplace_modal.rs] (-25)
- [ ] `delete` `compaction_settings()`, `user_reserve_tokens()`, `DEFAULT_COMPACTION_SETTINGS`, `CompactionSettings.enabled`. [compact/policy.rs:42-60] (-20)
- [ ] `shrink` `get_provider_models_with_live` delegate → call `fetch_live_provider_models` at 5 sites. (-10)
- [ ] `delete` `auto_compact_if_needed` ignored `_config/_last_usage/_reason` params. (-8)
- [ ] `delete` `get_provider_models` `Vec::new()` stub (0 callers). (-6)
- [ ] `shrink` `sweep_old_shell_logs` → `crate::setup::gray_home()`. (-7)
- [ ] `delete` effort modal unreachable empty-levels fallback. [setup/effort.rs:43-49] (-5)
- [ ] `delete` unused re-exports `has_nerd_font`, `dim_color/dim_line/dim_style`. [setup/mod.rs] (-3)
- [ ] Run: `cargo test -p gray --lib`
- [ ] Commit: `chore(cut): remove dead setup, skills, and prompt surface`

---

## Phase 8: gateway / acp / cron / supervise

### Task 11: Delete dead daemon and channel surface

**Files:** `crates/gray-gateway/src/{status,platform,lock,progress,config,daemon_boot,authz,delivery}.rs`, per-channel adapters, `crates/gray-acp/src/{session,events,error,registry}.rs`, `crates/gray-cron/src/{lib,store}.rs`, `crates/gray-supervise/src/exit.rs`

- [ ] `delete` status board REPL/stage facade: `gateway_boot_rows`, `mark_stage`, `set_status_board` (+3 overrides, `stage()` helpers, `board` fields), `notified`+Notify, `all_terminal`+`terminal`, `fail_unresolved`, `probe_board_healthy`, `Platform::label`; keep `snapshot`/`save_snapshot`/`mark_connected`/`mark_failed`/`read_board_healthy`. (-140)
- [ ] `delete` test-only helpers: `platform::{truncate_message,split_message}` (prod uses `split_message_smart`), `telegram::drive_heartbeat`, `slack::has_socket_mode`, `lock::gateway_locked_elsewhere`, `progress::ProgressLines::text`. (-90)
- [ ] `delete` acp one-impl `PermissionPrompt`/`DenyAllPrompt` + option threading (both callers pass `DenyAllPrompt`) → inline always-deny. (-45)
- [ ] `delete` `run_gateway_shutdown` + `_with_board` + `Option<board>` param (only `run_gateway` with `None` called). (-35)
- [ ] `delete` acp `SessionInfo`/`info()`/`mode`, `usage_text` + `status_line` param (never written). (-30)
- [ ] `yagni` `GatedExecutor` (`tool_call_allowed` always Err; 3/4 params ignored) → unit deny executor. [authz.rs:161-202] (-25)
- [ ] `delete` acp dead code: empty `AcpSession::shutdown()`, `EventMapper::finish`, unconstructed `AcpError::{UnknownAgent,Spawn,Busy}`. (-20)
- [ ] `delete` `DeliveryLedger::sweep_marked`, `CronStore::add` wrapper. (-20)
- [ ] `delete` `MessageEvent.media_urls` (never read); `GatewayConfig.denied_tools` (gate hardcodes `&[]`); `BasePlatformAdapter::is_alive` default. (-28)
- [ ] `stdlib` `preview_80` backtrack loop → `floor_char_boundary(80)`. (-8)
- [ ] `shrink` `DeliveryLedger::sweep` ↔ `sweep_all_claimable` one filter; `progress::utf16_len` dup → `platform::utf16_len`. (-14)
- [ ] `delete` gray-cron 10 unused re-exports; `exit::EXIT_CLEAN`. (-9)
- [ ] Run: `cargo test -p gray-gateway --features all-platforms -p gray-acp -p gray-cron -p gray-supervise`
- [ ] Commit: `chore(cut): remove dead gateway/acp/cron/supervise surface`

### Task 12: gray-extras (only if Task 0 approved)

- [ ] Migrate the useful OAuth impl into its retained home (per docs/cli-architecture-audit.md), delete proxy + unwired webfetch if no consumer, then `git rm -r crates/gray-extras` + workspace member + CI refs.
- [ ] Run: `cargo check --workspace --all-features && cargo test --workspace --quiet`
- [ ] Commit: `chore(cut): remove gray-extras catch-all crate`

---

## Phase 9: gray-pkg / gray-plugin

### Task 13: Dedupe package/plugin plumbing

**Files:** `crates/gray-pkg/src/{ops,sources,skills_ops,index,adapter}.rs`, `crates/gray-plugin/src/{lock,lib,builder}.rs`, `crates/gray/src/plugin_check.rs`

- [ ] `delete` pkg's duplicate `LockFile`/`LockEntry`/`default_true`/`Default`/`read_lock`/`write_lock` (`TODO(2.4)` admits it) → `gray_plugin::lock` load/save. [ops.rs:11-60,257-274] (-66)
- [ ] `shrink` `ops::clone_git_repo`, `sources::{clone_into_tmp,clone_plugin_repo}` → one helper with extra-flags param. (-50)
- [ ] `shrink` hoist one `resolve_argv`: gray's private copy vs `builder::resolve_install_argv` mirror → make one `pub fn` in gray-plugin, call from gray (gray already depends on gray-plugin). [plugin_check.rs:32-80] (-49)
- [ ] `shrink` ClawHub stage pipeline (`ops::install_clawhub`, `skills_ops::stage_clawhub`) → one `sources` helper returning `(root, verified)`. (-30)
- [ ] `delete` `lock::load_disabled_sidecar_argvs` (only tests; `builder::load_lock_files` reimplements). (-24)
- [ ] `shrink` duplicated `LockEntry{...}` + `write_lock` blocks in `install_index`/`install_url` → `record_install`. (-20)
- [ ] `yagni` merge `SearchSource`/`Source` enums (same labels; prod uses `SearchSource::label`). (-15)
- [ ] `shrink` `skills_ops` dup helpers: `now_secs` (3rd copy) → `ops::now_secs`; `validate_slug` → `ops::validate_install_key` made `pub(crate)`. (-15)
- [ ] `yagni` index fields never read: `Index.schema/generated`, `Entry.caps/adapter_min/requires`, `Source.subdir`. (-15)
- [ ] `shrink` builtin `Manifest` literals → `Manifest::default()` + `..`. [builder.rs:39-89] (-14)
- [ ] `delete` empty `adapter` module + feature (Task 2 removed the feature; remove `pub mod adapter;` + dir). (-10)
- [ ] `delete` `npm_tarball_url` `#[allow(dead_code)]` wrapper (tests call `npm_resolve`). (-10)
- [ ] `delete` `CoreEvent::PreStep` (never constructed) + match arm + `Message` import. (-8)
- [ ] `delete` never-read parsed fields: `ClawHubEntry.display_name`, `ClawHubDetail.display_name/summary`, `ClaudeEntry.marketplace`. (-8)
- [ ] `yagni` `builder::default_plugins()` (no prod caller; gray has `gray_defaults()`) → inline in tests. (-7)
- [ ] `delete` `impl FromStr for NameOrUrl` (all callers use `parse_spec`). (-7)
- [ ] `yagni` `Report.unverified`/`pi_summary` readers? If none outside gray-pkg → delete fields. (-6)
- [ ] `delete` `redact_url` one-line delegate. (-4)
- [ ] `yagni` `Manifest.provider` (parsed, never read). (-4)
- [ ] Run: `cargo test -p gray-pkg -p gray-plugin`
- [ ] Commit: `chore(cut): dedupe pkg/plugin plumbing and remove dead fields`

---

## Phase 10: Workspace, CI, assets

### Task 14: CI/repo hygiene

**Files:** `.github/workflows/{ci,release}.yml`, `assets/space/*`, `scripts/deploy.sh`

- [ ] `delete` 3 unreferenced PNGs (andromeda/eclipse/moon-dither) — confirm no external website consumption first (ask user). (-3 files)
- [ ] `shrink` release.yml 4× deploy-key bootstrap → one composite action; dedupe installer sync between `build-deploy` and `publish` on tag pushes.
- [ ] `shrink` ci.yml: drop `cargo check -p gray-gateway --features all-platforms` (subsumed by next line's test); gate OS-independent `fmt` + `rustsec` to ubuntu only.
- [ ] `yagni` `scripts/deploy.sh` re-implements release.yml staging without atomicity — delete unless manual single-leg deploys are real.
- [ ] Run: `cargo test --workspace --quiet` (final gate) + push branch; CI runs the rest.
- [ ] Commit: `chore(cut): dedupe CI/release steps, drop unreferenced assets`

---

## Final verification (after last executed task)

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test --workspace --quiet`
- [ ] `cargo test -p gray-gateway --features all-platforms --quiet`
- [ ] `cargo check -p gray --all-features`
- [ ] `git diff --stat main...HEAD` — confirm net removal; report per-task line counts.
- [ ] Land on the open PR (#54) or its successor branch; never a second PR.
