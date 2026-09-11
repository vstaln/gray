# Pulse — design

Status: approved direction (S1 of the gateway/24-7 discussion); being planned.

## Goal

Give gray a **24/7 pulse**: a standing goal the agent works on its own, on
a schedule, reaching the user on a chat platform only when there is something
to say. It is an **extra**, never `gray-core`.

## Non-goals

- No memory subsystem (rejected on philosophy grounds; the model keeps state in
  files it maintains).
- No new agent loop, no new scheduler, no core prompts.
- No new chat platforms.

## Architecture

Reuse everything that exists; add one core module.

- **Scheduler**: the gateway already runs a 60s cron ticker
  (`gray-gateway/src/daemon_boot.rs:183` → `daemon.rs::spawn_cron_ticker`) and
  fires due `gray_cron` jobs through `GatewayRunner::run_cron_job`, delivering
  via `deliver_cron_outcome` and honoring `[SILENT]`
  (`daemon.rs::is_silent`) — whole output or first/last line suppresses
  delivery. Pulse is therefore **a cron job**.
- **State**: one goal file `$GRAY_HOME/pulse/goal.md` and one config file
  `$GRAY_HOME/pulse.json` (`enabled`, `schedule`, `deliver`).
- **Job**: one `gray_cron::CronJob` named `pulse` in the shared
  `$GRAY_HOME/cron` store. Its prompt is the pulse template with the goal
  embedded; `deliver` is the configured `Deliver::Target("<platform>[:chat[:thread]]")`.
- **Control**: a core `gray pulse` subcommand
  (`on|off|status|goal|sync`), living in `crates/gray/src/pulse`.
- **Plugin configurability**: the same subcommand runs as a **sidecar plugin**
  (`gray pulse plugin`), registerable in `gray.yml`, exposing a
  `pulse` tool so the model can read/set its goal, cadence, and on/off from
  inside a turn. The agent run a pulse triggers already uses the normal
  `gray.yml` profile, so all plugins/hooks apply to it.

## Data / interfaces

- `PulseConfig { enabled: bool, schedule: String, deliver: String }`
  (serde JSON) at `$GRAY_HOME/pulse.json`.
- Goal text at `$GRAY_HOME/pulse/goal.md`.
- Cron job `name == "pulse"`; prompt = [`render_prompt(goal)`].
- `render_prompt` instructs: take the next useful step, and reply with exactly
  `[SILENT]` when nothing is worth reporting (the delivery layer already
  suppresses `[SILENT]`).

## Quiet-by-default

No new mechanism: the prompt directs `[SILENT]`, and the gateway's existing
`is_silent` check suppresses delivery. (Verified: `daemon.rs` `is_silent` —
whole output or first/last line.)

## Packaging

- `crates/gray/src/pulse` — core module of the `gray` crate, exposed as
  `gray pulse`.
- Depends on `gray-cron` and `gray-gateway` (existing gray deps, no cycle).
- Part of the default build.
- Requires the gateway daemon to be running with a configured platform for
  delivery; without it the job still records output under `$GRAY_HOME/cron/output`.
