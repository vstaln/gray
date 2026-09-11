# Pulse — a 24/7 standing goal

The pulse is a standing goal the agent works on a schedule, on its own,
around the clock. Each fire wakes the agent with your goal, lets it take the
next useful step with its tools, and reports back to chat — but only when there
is something worth saying. Wake-ups while nothing has changed stay quiet.

It is a core `gray pulse` subcommand, part of the default build.

State lives under `$GRAY_HOME` (default `~/.gray`):

| file | contents |
|---|---|
| `$GRAY_HOME/pulse/goal.md` | the standing goal (plain Markdown) |
| `$GRAY_HOME/pulse.json` | `enabled`, `schedule`, `deliver` |

The schedule is a cron job named `pulse` in the gateway's cron store; the
existing gateway cron ticker fires it.

## CLI

```bash
# Turn it on, every 30 minutes, reporting to a Telegram chat.
gray pulse on --every 30m --deliver telegram:123

gray pulse off                 # disable and remove the cron job
gray pulse status              # enabled? next run? (disabled | enabled, next run …)
gray pulse goal Ship the v1 launch post and keep the CI green.
gray pulse goal                # print the current goal
gray pulse sync                # re-render the cron job from goal + config
```

`on` defaults to `every 30m` and `local` delivery. `--deliver` accepts:

- `local` — run but don't send; the run doc is still saved.
- `origin` — reply in the chat the job was created from (falls back to every
  configured home channel for a CLI-made job).
- `<platform>[:chat[:thread]]` — an explicit target, e.g. `telegram:123`,
  `discord:456`, `slack:789:thread`. An unparseable target fails safe to
  `local`.

After editing the goal, run `gray pulse sync` to push it into the cron job.

## Quieting

The wake-up prompt tells the model to reply with exactly `[SILENT]` when there
is nothing to do, nothing changed, or nothing worth reporting. The gateway
suppresses delivery when `[SILENT]` is the whole output, its first line, or its
last line. The run doc is always saved either way.

## Requirements

- The **gateway daemon must be running** for the cron ticker to fire the job,
  and at least one platform must be configured for a non-`local` delivery
  target to resolve. See the Gateway section of the [README](../README.md).
- The pulse uses the gateway's own platform credentials; there is no
  separate bot or token.

## Running as a plugin

`gray pulse plugin` speaks the sidecar NDJSON protocol and exposes a
single `pulse` tool (`status`, `goal_get`, `goal_set`, `on`, `off`,
`sync`) so the agent can manage the pulse itself. Register it in
`gray.yml`:

```yaml
plugins:
  - tools-minimal
  - sidecar: [gray, pulse, plugin]
```

Register the sidecar as an argv list: a bare path form spawns the binary with
no arguments, so plugin mode (`pulse plugin`) never starts. The argv list form
does not expand `~` — spell the path out. For example, write
`- sidecar: [/home/you/.local/bin/gray, pulse, plugin]` with the absolute path
to the installed `gray` binary.

The agent can then read or set the goal, flip the pulse on/off, and check
status in conversation. See [`plugins.md`](plugins.md) for the sidecar wire
protocol.
