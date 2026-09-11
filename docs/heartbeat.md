# Heartbeat — a 24/7 standing goal

The heartbeat is a standing goal the agent works on a schedule, on its own,
around the clock. Each fire wakes the agent with your goal, lets it take the
next useful step with its tools, and reports back to chat — but only when there
is something worth saying. Wake-ups while nothing has changed stay quiet.

It is a **gated extra**, not part of the default build. Build it explicitly:

```bash
cargo build --release -p gray-heartbeat
```

The `gray-heartbeat` binary is a one-shot CLI (or a sidecar plugin with
`--plugin`). State lives under `$GRAY_HOME` (default `~/.gray`):

| file | contents |
|---|---|
| `$GRAY_HOME/heartbeat/goal.md` | the standing goal (plain Markdown) |
| `$GRAY_HOME/heartbeat.json` | `enabled`, `schedule`, `deliver` |

The schedule is a cron job named `heartbeat` in the gateway's cron store; the
existing gateway cron ticker fires it.

## CLI

```bash
# Turn it on, every 30 minutes, reporting to a Telegram chat.
gray-heartbeat on --every 30m --deliver telegram:123

gray-heartbeat off                 # disable and remove the cron job
gray-heartbeat status              # enabled? next run? (disabled | enabled, next run …)
gray-heartbeat goal set "Ship the v1 launch post and keep the CI green."
gray-heartbeat goal                # print the current goal
gray-heartbeat sync                # re-render the cron job from goal + config
```

`on` defaults to `every 30m` and `local` delivery. `--deliver` accepts:

- `local` — run but don't send; the run doc is still saved.
- `origin` — reply in the chat the job was created from (falls back to every
  configured home channel for a CLI-made job).
- `<platform>[:chat[:thread]]` — an explicit target, e.g. `telegram:123`,
  `discord:456`, `slack:789:thread`. An unparseable target fails safe to
  `local`.

After editing the goal, run `gray-heartbeat sync` to push it into the cron job.

## Quieting

The wake-up prompt tells the model to reply with exactly `[SILENT]` when there
is nothing to do, nothing changed, or nothing worth reporting. The gateway
suppresses delivery when `[SILENT]` is the whole output, its first line, or its
last line. The run doc is always saved either way.

## Requirements

- The **gateway daemon must be running** for the cron ticker to fire the job,
  and at least one platform must be configured for a non-`local` delivery
  target to resolve. See the Gateway section of the [README](../README.md).
- The heartbeat uses the gateway's own platform credentials; there is no
  separate bot or token.

## Running as a plugin

`gray-heartbeat --plugin` speaks the sidecar NDJSON protocol and exposes a
single `heartbeat` tool (`status`, `goal_get`, `goal_set`, `on`, `off`,
`sync`) so the agent can manage the heartbeat itself. Register it in
`gray.yml`:

```yaml
plugins:
  - tools-minimal
  - sidecar: [/abs/path/gray-heartbeat, --plugin]
```

Register the sidecar as an argv list: a bare path form spawns the binary with
no arguments, so the plugin mode (`--plugin`) never starts. `gray-heartbeat`
must be installed at (or copied to) the absolute path you list, and the argv
list form does not expand `~` — spell the path out. For example, after
`cargo build --release -p gray-heartbeat`, copy
`target/release/gray-heartbeat` to `~/.gray/plugins/gray-heartbeat` and write
`- sidecar: [/home/you/.gray/plugins/gray-heartbeat, --plugin]`.

The agent can then read or set the goal, flip the heartbeat on/off, and check
status in conversation. See [`plugins.md`](plugins.md) for the sidecar wire
protocol.
