# Release fix loop (0.1.x line, no version bump)

Goal: TUI super fast, extremely high cache hits, usable by people. Source: audit ledgers 2026-09-07 (6 lanes) + dogfood RESULTS.

## Global Constraints (binding on every task)
- NEVER `cargo test` (kills X). Verify: `nice -n 19 ionice -c3 flock /tmp/cargo.lock cargo check -p <crate>` + `cargo clippy -p <crate> -- -D warnings`, then dogfood evidence where behavior changed.
- No commits, pushes, branch switches, stash, reset, clean. Shared checkout: `git status` first, touch ONLY listed files.
- New code ONLY under `dogfood/`. Never read/write real `~/.gray` (isolated `GRAY_HOME`). Redact secrets.
- Unit tests added for logic changes are marked UNRUN (ban) and flagged in the report.
- Live quota: tiny prompts; 429 = environmental (wait 60s, ×3, move on).
- Ponytail full: smallest diff, no new abstractions/deps, reuse existing helpers.

## Wave 1 (parallel, file-disjoint)
- T1 crashers: prompt_turn.rs, empty_turn.rs, key_watcher.rs, composer/question/session.rs, repl/handlers.rs (println sites only).
- T2 wire: gray-provider/src/openai.rs ONLY (preserve all other hunks byte-identical).
- T3 windows: gray-tools shell/kill.rs, shell/spawn.rs, shell/exit.rs, read/guard.rs + gray-gateway systemd.rs test gates.
- T4 release logistics: .github/workflows/ci.yml + CHANGELOG.md.
- T5 dogfood re-run: reuse dogfood/scenarios modal_* + screenshot picker/modals on fresh binary.

## Wave 2 (queued): guard bypass batch, session integrity, sid-threading, billing-429 distinction.
## Final: whole-branch review + finishing-a-development-branch.
