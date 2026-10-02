---
name: gray
description: "Run the gray coding agent: one-shot prompts, JSON output, run budgets, session resume, scheduled jobs. Use when asked to start gray, hand a task to gray, or continue/resume a gray session. Do not use for editing files yourself, and do not use merely because a background task might help — gray is a separate agent with its own tools and its own context."
---

# gray

Gray is a coding agent CLI written in Rust: a provider loop, tools, sessions,
auto-compaction and cron. Callers that are not a human at a terminal should
always use `-p`. The bare `gray` REPL takes over the terminal and needs a TTY.

## Learn the current CLI

The installed binary is the authority for syntax. Run `gray --help` before
assuming a flag or subcommand exists; do not trust a remembered flag list.

## One-shot

```bash
gray -p "fix the failing test in crates/gray-tools"
gray -p "..." --json           # versioned NDJSON progress, then a final result
gray --json --input-json task.json    # structured envelope; path or "-"
```

`--json` requires `-p` or `--input-json`.

## Exit codes

- `0` the turn finished
- `1` the turn failed
- `3` provider or infrastructure failure (retryable)

The `--json` result carries a `code` (`auth_failed`, `rate_limited`,
`context_overflow`, `loop_detected`, ...), a `retryable` flag, and a `hint`
naming the command that fixes it. Read the hint instead of retrying blindly.
A failed turn may already have changed files.

## Budgets

```bash
gray -p "..." --json --max-turns 20
gray -p "..." --json --max-requests 40
gray -p "..." --max-cost-usd 2.00
gray -p "..." --max-wall-secs 900
```

## Sessions

```bash
gray -c                          # continue the most recent conversation
gray --session <id>              # resume a specific session (id prints on exit)
gray resume                      # interactive picker
gray sessions prune --older-than-days 90
```

Transcripts are JSONL under `$GRAY_HOME` (default `~/.gray`), written 0600,
with parent-id branching.

## Scheduled jobs

```bash
gray cron add "0 9 * * 1-5" "summarize yesterday's commits" --deliver local
gray cron list
gray cron show <id>
gray cron remove <id>
```

A schedule is `every 1h`, `30m`, `in 10m`, an RFC3339 timestamp, or a cron
expression. `add` runs validation inline; the supervised gateway daemon fires
the jobs. Delivery is a file by default; `--deliver origin` appends back to a
chat session.

## Other subcommands

`gray find`, `gray grep`, `gray plugin list`,
`gray gateway status`, `gray update`, `gray login`, `gray whoami`.

## What gray can do

Bash is the main tool. Image, search and background jobs arrive as
bash-claimed commands rather than extra tools. A long or silent command
becomes a background job: poll it with the job output/status actions and a
wait, never with a tight sleep loop, and a silent blocking command is turned
into a job after 600 s rather than hanging the turn.

## Rules when driving gray

- Prefer `-p`. Do not launch the bare REPL from a script or another agent.
- Do not probe a subcommand by omitting arguments: several accept defaults and
  will execute. `gray cron add` with no schedule fails, `gray sessions prune`
  with no threshold does not.
- Output is redacted per line, but redaction does not stop a tool from sending
  an environment variable somewhere else. Never ask gray to print API keys.
- Gray has no approval prompt and runs with the user's privileges. Ask before
  destructive work; `gray --help` will not warn on gray's behalf.

## Extending gray

- `gray --skill` prints this file.
- `/skill` inside the REPL installs skills from disk.
- `gray --dump-manifest` prints the merged plugin manifest as JSON.
