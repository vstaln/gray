# SPEC-01 — Bash observation budget (mini-swe-agent 10k rule)

## Problem
Every `bash` tool result is injected into model context in full. Inline shell
views currently carry up to `MEM_HEAD_BYTES (16 KiB) + MEM_TAIL_BYTES (48 KiB)
= 64 KiB` (~16k tokens) per call (`crates/gray-tools/src/shell/contract.rs`).
A few such calls bloat every subsequent inference turn for the rest of the
session. mini-swe-agent caps every observation at 10,000 chars
(head 5000 + tail 5000 + elided count) and pays bounded tokens per step.
Measured symptom: full `cargo test` outputs and multi-hundred-line greps
returned verbatim into context during the gray2 dogfood session.

The full output already persists on disk (`~/.gray/shell/<session>/<id>.log`)
and the header already points at it ("grep the log instead of rerunning" is in
the bash tool description). Nothing is lost by shrinking the inline view.

## Design
1. **Shrink the inline view, keep the disk log.** Retune in
   `crates/gray-tools/src/shell/contract.rs`:
   - `MEM_HEAD_BYTES`: 16 KiB → 6 KiB
   - `MEM_TAIL_BYTES`: 48 KiB → 6 KiB
   - Inline budget ≈ 12 KiB total (~3k tokens), head+tail with elision notice.
   - `shell_output(task_id, from_offset=…)` paging stays unlimited — the agent
     can still page the full log in bounded chunks. No other code path changes.
2. **Header must state the budget.** `header()` in
   `crates/gray-tools/src/shell/view.rs` already renders
   "showing first X + last Y · N lines / M chars omitted · log <path>".
   Keep that exact shape; add the log-grep hint when omitted > 0, e.g.
   `· grep the log for more (shell_output pages it)`. The bash tool
   description (`shell/tools/bash.rs` lines ~40-45) already says this — keep
   both consistent.
3. **Pager/env hygiene (mini-swe-agent's `PAGER=cat` rule).** In
   `crates/gray-tools/src/shell/spawn.rs`, force on the child env:
   `PAGER=cat`, `MANPAGER=cat`, `GIT_PAGER=cat`, `SYSTEMD_PAGER=cat`.
   Rationale: kills interactive-pager hangs and progress-bar token spam at the
   source. Do NOT set `NO_COLOR` (may break golden tests that assert ANSI).
4. **No change to** `truncate.rs` (2000 lines / 50 KiB head-only for
   find/grep) — out of scope; shell inline views are the per-turn tax.

## Files to touch
- `crates/gray-tools/src/shell/contract.rs` — the two constants.
- `crates/gray-tools/src/shell/view.rs` — `header()` elision hint (only if
  missing after the retune; check goldens at `view.rs:416+`).
- `crates/gray-tools/src/shell/spawn.rs` — child env pager vars.
- Tests colocated in the same files (`view.rs` goldens, new unit test for
  head+tail elision at 12 KiB boundary).

## Acceptance
- `cargo test -p gray-tools shell` green; `header_goldens` updated if the
  hint text changes.
- Manual: run a command emitting > 100 KiB; inline result ≤ ~12 KiB, names
  the log path, and `shell_output` pages the rest.
- `cargo test -p gray-tools` full green (pager env must not break goldens).
- `cargo fmt --check` clean.

## Non-goals
- No change to background/promotion, timeouts, or `shell_output` paging.
- No change to find/grep/read truncation budgets.
