# Cron ticker (workstream B) — design

Date: 2026-09-14. Status: approved in chat (4/4 sections), pending spec review.

## Context

`gray-cron` is store + schedule math only (`crates/gray-cron`: `schedule.rs`,
`store.rs`); `gray cron` is a file-only CLI (`crates/gray/src/main.rs`). The
gateway that used to fire jobs was deleted (plan
`docs/superpowers/plans/2026-09-11-gateway-extraction-plan-a.md`), which names
this gap explicitly: "`gray cron` manages jobs that never run. B closes it."
Reference: `reference/NousResearch/hermes-agent/cron/` (~14k lines) and
`website/docs/developer-guide/cron-internals.md`. Guiding frame: timed gray —
a job fire behaves like a `gray -p` run, on a schedule.

## Goals / non-goals

Goals: a ticker that claims due jobs at-most-once and fires each through the
real headless agent with skills + pre-run scripts, local-file delivery,
`pause`/`resume`/`run` lifecycle, honest statuses. Non-goals (deferred):
`edit`, per-job model/provider overrides, `context_from` chaining,
multi-platform delivery (Discord/Telegram/…), incidents/monitor/suggestions,
managed-cron providers, NL weekday schedules, `no_agent` script-only jobs
(the wake gate covers the watchdog pattern without the extra flag).

## 1. Architecture & CLI

`gray-cron` stays sync store + math, no async, no agent edge. New async work
lives in the `gray` crate (`cron_fire.rs` + `cron_serve.rs`-shaped modules),
reusing `build_agent` (`crates/gray/src/lib.rs`) and the headless one-turn
path (`crates/gray/src/print.rs`). Only `gray-cron` change: additive
serde-defaulted `skills: Vec<String>` and `script: Option<PathBuf>` on
`CronJob`, so existing `jobs.json` files keep parsing.

CLI: `gray cron tick` (one claim→fire→record pass; nonzero exit only if the
pass itself broke — lock timeout, corrupt store — never for per-job
failures; also the OS-cron/runit entry point), `gray cron serve` (`tick` in a
60s loop until SIGINT/SIGTERM; supervision owns the process, no
daemonization), `pause`/`resume` (`pause` flips `state` to `Paused`, leaving `enabled`
untouched — `claim_due` already requires both; `resume` flips back to `Active`
and recomputes `next_run_at` so a long-paused job isn't instantly stale),
`run` (fire now through claim→run→`mark_done`, advancing recurring schedules
so the next tick doesn't double-fire; needs a single-job claim path in the
store — new method or equivalent, detail left to the implementation plan). `list`/`add`/`show`/`remove` unchanged, plus
`--skills`/`--script` on `add`.

Data flow per tick: `claim_due("pid:boot-uuid")` → for each claimed job in
order: resolve workdir → pre-script → assemble prompt → fresh headless agent
turn → deliver → `mark_done` → next job. Sequential: no pool, no overlapping
fires per ticker; a concurrent ticker gets zero claims (already-claimed).
Owner stamp: `std::process::id()` + one `Uuid::new_v4()` per `serve`/`tick`
process (dead processes never reuse a boot uuid — all the 300s TTL needs).

## 2. Fire pipeline

Prompt assembly is a pure function (unit-tested, no model): workdir (job's,
else ticker cwd) → optional pre-script → final prompt. Script runs as a
subprocess, cwd=workdir, piped output, killed on `SCRIPT_TIMEOUT_SECS = 300`
(pre-step stays inside one tick window; gray's bash tool self-bounds at 600).
Stdout capped at 8000 chars, appended as fenced `## script-output`. Wake gate
(hermes shape): last non-empty stdout line parsing as JSON
`{"wakeAgent": false}` skips the LLM run → success-silent. Script nonzero
exit/timeout → `error`, agent never runs. Script path must be absolute +
existing at `add` (same validation as `--in`), re-checked at fire; a deleted
script/skill at fire time is `error`, fail loud.

Skills are context-only in gray (no skill tool; model `cat`s SKILL.md): attach
= resolve each name via existing `resolve_skill_name(workdir, name)` (against
the job workdir, both at `add` validation and at fire)
(`crates/gray/src/skills_tool.rs`) and prepend a short `## skills` block with
exact `<location>` paths (≈ pasted `/skills <name>`). Unknown name rejected
at `add`; missing at fire → `error` naming it.

Agent run: always fresh session (no resume/history — hermes isolation),
`build_agent(config, workdir, …)`, output collected via `run_streaming` with
an accumulating callback (ticker stdout stays clean for tick logs). Recursion
guard is architectural: no `cronjob` tool exists, jobs cannot create jobs.
`[SILENT]` prefix (case-insensitive, trimmed) suppresses delivery, records
`ok`. Whole fire under `tokio::time::timeout(FIRE_TIMEOUT_SECS = 600`); on
timeout the run is abandoned, recorded `error` ("fire exceeded 600s"), claim
cleared via normal `mark_done` so the next tick isn't poisoned.

## 3. Delivery & errors

`local` (default) writes the transcript to
`$GRAY_HOME/cron/output/<job-id>/<unix-ts>.md` with header (name, id, fire
time, schedule), mode 0600, atomic tmp+rename like the store. `origin`/named
targets stay stored-but-uninterpreted (Plan A's opaque seam): run executes,
delivery records `DeliveryFailed` ("no delivery backend in this build").
`[SILENT]`/wake-gate silence skips the write, records `ok`.

Errors: agent failure → `error` + `last_error`, claim cleared, ticker
continues (one bad job never aborts the pass). Delivery write failure →
`delivery_failed` + `last_delivery_error`, run columns untouched. Missed
one-shots / stale-recurring fast-forward already in `claim_due`, unchanged.
Store failures (lock timeout, corrupt `jobs.json`) abort the pass nonzero —
at-most-once needs real exclusion. Job-boundary panic capture
(`AssertUnwindSafe` + explicit claim release) → `error`, ticker continues.
Statuses stay closed: `ok` / `error` / `delivery_failed`, no new literals.

## 4. Testing & rollout

Fire pipeline takes an injected async runner (`Fn(prompt) -> transcript`):
stubbed tests for assembly (injection, 8k cap, gate true/false/malformed,
missing script, unknown/missing skills), `[SILENT]`, timeout → `error` +
claim cleared, per-job failure isolation, `error` vs `delivery_failed`
split. Store tests unchanged; new CLI tests for `pause`/`resume`/`run`
transitions + resume `next_run_at` recompute. Loop per repo rules:
`cargo test -p <touched>`, `./target/debug/gray2`, full workspace +
`cargo fmt --check` pre-commit, `CARGO_BUILD_JOBS=4`.

Rollout: this branch + PR (no open PRs at write time), update the file-only
claim in `docs/plugins.md`, CHANGELOG entry. Live proof: one-shot job that
appends a marker via the agent → `tick` → output file + `show` status.
`serve` supervision (runit/systemd) is machine-local, out of the harness
diff. Serve supervision on this machine is explicitly out of scope.
