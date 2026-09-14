# WORKLOG — harness loop + background-task restore (2026-09-14)

## Done
- `eval/` loop (meta-harness, gray-sized): 10 tasks (`tasks/*/instruction.md` +
  `tests/test.sh`), `run.sh` (gray -p per task, ledger append), `harness-note.sh`
  (session↔harness join), `domain_spec.md` (ONBOARDING template), `report-template.md`.
  Zero Rust, zero deps. `eval/` is gray2-free (`GRAY_BIN=gray`).
- Baseline: `wordcount-report` PASS (session ca129ec6, ledger line ok). Full 10-task
  `run.sh` background run t112 finished 4m24s — see ledger tail for scores.
- Ledger: `~/.gray/harness-runs/summary.jsonl` (`{ts,task,pass,session_id,harness,workdir}`).
- Goose study prompt: `reference/aaif-goose/STUDY_PROMPT.md` (for another agent).

## Active: surgical background restore (user picked option 3)
- Problem: blocking-only refactor (commits 360af29, 1cd3614, 9f30e18 + ~30 dirty files)
  deleted the whole background circuit; `/gray` TUI shows no background tasks.
- Decision: restore `background=true` + timeout-promotion in `bash.rs` ONLY.
  No wake/drain/UI circuit, no registry resurrection beyond what bash needs,
  no `sleep`/`shell_kill` tools. Tasks run, logs on disk, header names the log.
- IMPLEMENTED 2026-09-14: `bash.rs` +background arg, detached `reap_detached`
  (wait + bounded pump drain, warn-only), timeout→promote (cancel still kills),
  `background_start`/`promoted` cards (pid + log path, tail/grep/kill hints).
  Registry-free: no ids, no wake, no new tools. Tests: 6 lib + contract suite
  updated (promotion replaces kill-on-timeout asserts); `cargo test -p gray-tools`
  all green (168+12+9+2), clippy clean (pre-existing dual-bin warning only),
  `cargo fmt --check` clean.
- Deletion inventory + options: see chat 2026-09-14 (options were full revert /
  keep blocking-only / surgical; picked surgical).

## Deferred
- `report.md` failure notes (add when baseline shows a real failure).
- Multi-trial ×3 on ties, per-task token rollup (spec marks unknown).
- `meta_harness.py`-style proposer automation (after 2+ manual loops).
- `crates/gray/Cargo.toml` binary still named `gray2`; installed `gray` is Sep-13
  build (pre-refactor). Rename/reinstall is user's call.

## 2026-09-14 — project context auto-load + /context honesty + backdrop pin
- Root cause (context): `collect_parts` hardcoded `project_context: 0` and
  nothing ever read project `AGENTS.md`/`CLAUDE.md` — the system prompt only
  told the model to `cat` them manually. `/home/vstaln/gray/AGENTS.md` (3.3k)
  was invisible to `/context`.
- Fix: `ProjectContextPlugin` (`skills_tool.rs`) serves `<project_context>`
  via the `prompt/context` hook (same seam as `SkillsPlugin`; system prefix
  stays byte-stable). Discovery: cwd → git root, `AGENTS.md`+`CLAUDE.md` per
  dir, root-first/nearest-last, `~/.gray/AGENTS.md` excluded (no double-bill),
  16k chars/file cap with truncation note. Wired into `extra_plugins`
  (`lib.rs`); guideline line updated (no more manual cat for rules);
  `collect_parts` estimates the exact block served, and the provider-total
  residual now subtracts sys+project+tools+skills (keeps `used()==total`).
- Modal/text area: all 8 modals already share `render_dimmed_background` +
  `TuiSession` (universal by construction); current tree dims input copy
  (your screenshot matches the Sep-13 installed binary, which predates the
  `e0357b1`/`9740148` dim fixes — reinstall at your convenience). Pinned with
  `backdrop_textarea_copy_is_dimmed_box_and_text`: textarea copy bg+text
  dimmed AND no full-brightness `surface_bg` anywhere in any backdrop.
- Verify: `cargo test -p gray --lib` 261 pass; `backdrop_dim` 3 pass;
  `cargo fmt --check` clean; clippy clean on touched files (one pre-existing
  `gray-core/compact.rs` collapsible-if untouched); `gray2 -p` smoke from repo
  root returns `hook-ok` (hook live, no turn breakage).
- Note: tree is shared live — `tool_fmt/mod.rs` breakage I hit mid-task was
  reverted by the other agent before I touched it; `fence.rs` protocol edit
  in progress by them, left alone. Backslash counts in tool logs lie (display
  collapsing); use `python chr(92) counts` for ground truth.
