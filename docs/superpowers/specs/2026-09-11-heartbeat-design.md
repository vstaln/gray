# Heartbeat — design

Status: approved direction (S1 of the gateway/24-7 discussion); being planned.

## Goal

Give gray a **24/7 heartbeat**: a standing goal the agent works on its own, on
a schedule, reaching the user on a chat platform only when there is something
to say. It is an **extra**, never `gray-core`.

## Non-goals

- No memory subsystem (rejected on philosophy grounds; the model keeps state in
  files it maintains).
- No new agent loop, no new scheduler, no core prompts.
- No new chat platforms.

## Architecture

Reuse everything that exists; add one gated crate.

- **Scheduler**: the gateway already runs a 60s cron ticker
  (`gray-gateway/src/daemon_boot.rs:183` → `daemon.rs::spawn_cron_ticker`) and
  fires due `gray_cron` jobs through `GatewayRunner::run_cron_job`, delivering
  via `deliver_cron_outcome` and honoring `[SILENT]`
  (`daemon.rs::is_silent`) — whole output or first/last line suppresses
  delivery. Heartbeat is therefore **a cron job**.
- **State**: one goal file `$GRAY_HOME/heartbeat/goal.md` and one config file
  `$GRAY_HOME/heartbeat.yaml` (`enabled`, `schedule`, `deliver`).
- **Job**: one `gray_cron::CronJob` named `heartbeat` in the shared
  `$GRAY_HOME/cron` store. Its prompt is the heartbeat template with the goal
  embedded; `deliver` is the configured `Deliver::Target("<platform>[:chat[:thread]]")`.
- **Control**: a standalone `gray-heartbeat` binary
  (`on|off|status|goal|sync`) so `gray` (the core binary) gains nothing.
- **Plugin configurability**: the same binary runs as a **sidecar plugin**
  (`gray-heartbeat --plugin`), registerable in `gray.yml`, exposing a
  `heartbeat` tool so the model can read/set its goal, cadence, and on/off from
  inside a turn. The agent run a heartbeat triggers already uses the normal
  `gray.yml` profile, so all plugins/hooks apply to it.

## Data / interfaces

- `HeartbeatConfig { enabled: bool, schedule: String, deliver: String }`
  (serde yaml) at `$GRAY_HOME/heartbeat.yaml`.
- Goal text at `$GRAY_HOME/heartbeat/goal.md`.
- Cron job `name == "heartbeat"`; prompt = [`render_prompt(goal, now)`].
- `render_prompt` instructs: take the next useful step, and reply with exactly
  `[SILENT]` when nothing is worth reporting (the delivery layer already
  suppresses `[SILENT]`).

## Quiet-by-default

No new mechanism: the prompt directs `[SILENT]`, and the gateway's existing
`is_silent` check suppresses delivery. (Verified: `daemon.rs` `is_silent` —
whole output or first/last line.)

## Packaging

- `crates/gray-heartbeat` — library + `[[bin]] gray-heartbeat`.
- Depends on `gray-cron` only (no `gray`, no `gray-core`, no cycle).
- In workspace `members` + `[workspace.dependencies]`, **absent** from
  `default-members` (like `gray-cron`/`gray-gateway`).
- Requires the gateway daemon to be running with a configured platform for
  delivery; without it the job still records output under `$GRAY_HOME/cron/output`.
