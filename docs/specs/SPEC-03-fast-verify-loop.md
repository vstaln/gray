# SPEC-03 — Fast verify loop (mini-swe-agent workflow-in-prompt + dsh headless)

## Problem
Measured during the gray2 dogfood session: full `cargo test --workspace`
(95s) + release `cargo install --bin gray2` (94s) sat on the critical path of
every fix cycle, when `cargo test -p gray` (~20s) + `target/debug/gray2`
(11s build) would have verified the same edits. Nothing tells the agent the
fast loop; it rediscovers the slow one each session. mini-swe-agent bakes the
workflow into `system_template`; dsh gives headless one-shot runs with no
ceremony.

## Design
1. **Document the loop in `AGENTS.md`** (new short section, ~15 lines):
   - Iterate: `cargo test -p <touched crate>` (never `--workspace` mid-loop;
     touched code here is always leaf-ward — dependents can't break).
   - Run: `./target/debug/gray2` (11s incremental) during iteration;
     `cargo install --bin gray2` exactly once, at the end, because the user
     runs `gray2` from `~/.cargo/bin`.
   - Full `cargo test --workspace` + `cargo fmt --check` once pre-commit.
   - Cap build pressure on the interactive desktop:
     `CARGO_BUILD_JOBS=4` (8-core box shared with the user's session).
2. **Inject the same loop into the agent's working context** so fresh
   contexts get it without reading AGENTS.md first (mini-swe-agent's
   `system_template` trick). Concretely: append the 5-line fast-loop summary
   to the system prompt in `crates/gray/src/system_prompt.rs` (find the
   existing workflow/guidance block and extend it; do not create a second
   prompt source).
3. **Optional, cheap: shell-test hygiene.** The slowest suite binaries
   (10-14s for a handful of tests) are timeout/poll-bound
   (`gray-plugin/tests/sidecar.rs` 5-20s timeouts, `gray-tools` shell tests
   up to 60s budgets). Tighten happy-path polls (assert-with-poll at ~50ms
   instead of second-scale grace waits) WITHOUT weakening failure detection.
   Measure before/after per-binary wall time; land only strict wins.

## Files to touch
- `AGENTS.md` — new "Fast verify loop" section.
- `crates/gray/src/system_prompt.rs` — same content, condensed for the model.
- Optionally `crates/gray-plugin/tests/sidecar.rs`,
  `crates/gray-tools/tests/*shell*` — poll tightening + timing evidence.

## Acceptance
- A fresh agent session fixing a one-crate bug runs `-p <crate>` +
  dev binary by default (spot-check by asking it to fix something trivial).
- Pre-existing suite timings recorded before poll changes; no test weakened
  (all still fail correctly on injected faults — at minimum, reason about
  each touched timeout in the PR/commit message).
- `cargo fmt --check` clean.

## Non-goals
- No sccache setup, no CI changes, no restructuring of the test suite.
- No change to what the full suite covers — only iteration order + docs.
