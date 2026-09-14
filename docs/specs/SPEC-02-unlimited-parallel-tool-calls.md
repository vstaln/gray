# SPEC-02 — Unlimited parallel tool calls (dsh: no limits)

## Problem
`crates/gray-core/src/parallel.rs` already runs a turn's read-only calls
concurrently (`plan_segments` + `join_ordered`, wired into `agent_loop.rs`
lines ~639-730) — but the batchable set is `read | ls | find | grep` only.
`bash` is always a barrier (`Segment::Single`), so exploration sessions that
live in the shell (this session: ~20 sequential greps/seds, each a full
round-trip) serialize completely. dsh runs `maxParallelToolCalls: 100`;
user directive for gray: **no limits** — no semaphore, no size cap.

## Design
1. **Extend `is_batchable` to cover every read-only tool.**
   `crates/gray-core/src/parallel.rs`:
   - Add: `bash` (read-only by construction — see 2), `shell_output`,
     `sleep`, plus any other pure-read tools in the registry
     (audit `gray-tools` registry; default-deny stays: anything not
     explicitly listed remains a barrier).
   - Keep the existing test (`planner_groups_only_contiguous_batchable_runs`)
     updated: `bash` in the fixture must now land in a `Parallel` segment.
2. **Read-only-ness of `bash` is enforced by construction, not trust.**
   `bash` mutates only if the command mutates. Rule: a `bash` call is
   batchable iff it runs in the batchable position AND passes a cheap
   static screen — reject from the batch (demote to `Single` barrier) if the
   command string matches an obvious mutator pattern (redirections `>`, `>>`,
   `| … >`, `rm `, `mv `, `cp `, `mkdir`, `touch`, `sed -i`, `git ` except
   `git status|diff|log|show|blame`, `cargo ` except `cargo --version|metadata`,
   `sudo`, `chmod`, `chown`, `kill`, `tee`, `cargo install`, `npm i`, …).
   - Implement as `fn bash_is_batchable(command: &str) -> bool` in
     `parallel.rs`, unit-tested with allow/deny fixtures.
   - `plan_segments` currently sees `(id, name, args)` — extract
     `args["command"]` for `bash` and apply the screen there. Non-object args
     or unparsable command → `Single` (fail-safe, matches existing convention).
   - Document clearly: this is a heuristic, not a sandbox. A batchable `bash`
     that mutates anyway is still executed (same as today, just concurrently
     with siblings) — hooks/approval still see it in the ordered pre-pass.
3. **No cap, per directive.** `join_ordered` already spawns one task per input
   with no semaphore ("No cap: every call in the batch is in flight at once").
   Keep it that way. The `long_batchable_run_stays_one_segment` test (17-wide)
   is the precedent — extend it to a mixed read+bash run.
4. **Ordering/cancel/approval semantics unchanged.** Pre-pass (validation,
   `tool_before` hooks) stays on the loop thread in order; only
   `executor.execute` runs concurrently; post-pass reconciles by input index.
   Cancel still backfills `None`. `GRAY_PARALLEL_READS` kill-switch keeps
   working (all-`Single` fallback = today's loop verbatim).

## Files to touch
- `crates/gray-core/src/parallel.rs` — `is_batchable`, `bash_is_batchable`,
  `plan_segments` bash screen, tests.
- Possibly `crates/gray-core/src/agent_loop.rs` — only if the pre-pass needs
  the screened verdict surfaced; prefer keeping all logic in `parallel.rs`.
- Regression test: mixed turn `[read, bash(grep…), bash(sed -n…), grep]`
  plans one `Parallel` segment; `[read, bash(rm…)]` splits at the mutator.

## Acceptance
- `cargo test -p gray-core parallel` green, including new screen fixtures
  (allow: `grep -rn`, `sed -n '1,10p'`, `ls`, `cat`, `git diff`, `cargo metadata`;
  deny: `rm -rf`, `sed -i`, `cargo install`, `> file`, `| tee`, `sudo`).
- `cargo test -p gray-core` full green; `cargo fmt --check` clean.
- Live check: turn emitting 5 read-only calls completes them concurrently
  (wall ≈ slowest call, not sum).

## Non-goals
- No sandboxing, no new approval prompts, no changes to write-path tools.
- No cap/semaphore work — explicitly out per user directive.
