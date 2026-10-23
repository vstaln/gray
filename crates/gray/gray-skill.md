---
name: gray
description: "Run the gray coding agent: one-shot prompts, JSON output, run budgets, session resume, scheduled jobs. Use when asked to start gray, hand a task to gray, or continue/resume a gray session. Do not use for editing files yourself, and do not use merely because a background task might help — gray is a separate agent with its own tools and its own context."
---

# gray

Gray is a coding agent CLI written in Rust: a provider loop, tools, sessions,
and auto-compaction. Callers that are not a human at a terminal should
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

## Bare runs

```bash
gray -p "..." --bare             # or GRAY_BARE=1
```

Mini-swe-agent shaped: a one-line system prompt (no workflow, no working
directory) and plain blocking `bash` (no background jobs); no compaction, so a
run that fills the window stops. No `~/.gray/AGENTS.md`, memory, skills,
project AGENTS.md/CLAUDE.md, plugins, cache warming or update check.
Model and provider settings still apply. For benchmarks and reproducible runs.

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

Cron lives in the gateway plugin (`gray plugin install gateway`); `gray cron`
and `/cron` then work as before. Do not run a scheduler yourself to make a job
fire — whatever ticks the home fires it.

## Other subcommands

`gray plugin list`, `gray update`.

## What gray can do

Bash is the main tool. Image viewing and background jobs arrive as
bash-claimed commands rather than extra tools. A long or silent command
becomes a background job: poll it with the job output/status actions and a
wait, never with a tight sleep loop, and a silent blocking command is turned
into a job after 600 s rather than hanging the turn.

## Rules when driving gray

- Prefer `-p`. Do not launch the bare REPL from a script or another agent.
- Do not probe a subcommand by omitting arguments: several accept defaults and
  will execute. `gray plugin install` with no name fails, `gray sessions prune`
  with no threshold does not.
- Output is redacted per line, but redaction does not stop a tool from sending
  an environment variable somewhere else. Never ask gray to print API keys.
- Gray has no approval prompt and runs with the user's privileges. Ask before
  destructive work; `gray --help` will not warn on gray's behalf.

## Extending gray

- `gray --skill` prints this file.
- `/skill` inside the REPL installs skills from disk.
- `gray --dump-manifest` prints the merged plugin manifest as JSON.
