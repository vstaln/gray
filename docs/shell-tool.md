# shell tool — output examples (Phase 1D: foreground contract)

Spec: `docs/harness/shell-tool.md`. Every result is `header + "\n" + fence(body)`;
`is_error` is true only for harness failures (spawn error, guard deny, bad args).

## Success

```text
exit 0 · 0.0s · 1 lines · log ~/.gray/shell/nosession/t1.log
<untrusted-output task="t1">
hi
</untrusted-output>
```

## Non-zero exit is data, not an error (`is_error == false`)

```text
exit 3 · 0.0s · 0 lines · log ~/.gray/shell/nosession/t2.log · no output
```

## Benign exit (grep 1)

```text
exit 1 (no matches — not an error) · 0.0s · 0 lines · log ~/.gray/shell/nosession/t3.log · no output
```

## Signal death

```text
exit 137 (SIGKILL) (likely OOM-killed; check `dmesg | tail` or reduce parallelism) · 0.0s · 3 lines · log ~/.gray/shell/nosession/t4.log
<untrusted-output task="t4">
sigkill line 1
sigkill line 2
Killed
</untrusted-output>
```

## Truncation (30,000-line spew; body ≤ 50 KiB + marker, full log on disk)

```text
exit 0 · 0.4s · 30,000 lines · showing first 500 + last 1,500 · 28,000 lines / 1,200,000 chars omitted · log ~/.gray/shell/nosession/t5.log
<untrusted-output task="t5">
spew line 1 payload …
[… 28,000 lines / 1,200,000 chars omitted (bytes 12,800–1,212,800). grep the log path above, or shell_output(task_id="t5", from_offset=12800) to page …]
…spew line 30000 payload …
</untrusted-output>
```

(counts vary; shape is contractual)

## Timeout → promotion (2B: the Phase-1 kill arm is gone)

A foreground command that outlives `timeout` is NOT killed. It is promoted
to background; the result carries the tail captured so far:

```text
still running after 1s → promoted to background as t6 · pid 4242 · log ~/.gray/shell/nosession/t6.log
<untrusted-output task="t6">
tick 1
</untrusted-output>
shell_output(task_id="t6", from_offset=12) for the rest · next_offset=12
```

(`next_offset` is the log length when the result was built; the process is
still alive and the log keeps growing. Every spawn registers a task,
foreground included — a foreground log path (`…/t5.log`) can be paged later
with `shell_output(task_id="t5")`, 2C.)

## Cancel

```text
cancelled by user after 0.5s
exit 143 (SIGTERM) (terminated (SIGTERM)) · 0.5s · 1 lines · log ~/.gray/shell/nosession/t7.log
…
```

## Guard deny (`is_error == true`, message unchanged)

```text
Blocked by destructive-command guard (rm-rf-root): rm targeting a system root is unrecoverable. Safe alternative: delete a narrower path, preview with `ls`/`find … | wc -l` first. If the user explicitly asked for this, have them run it manually.
```

## Background

```text
started t7 · pid 4250 · log ~/.gray/shell/nosession/t7.log
shell_output(task_id="t7", from_offset=0) to read output
```

Returns as soon as the task is registered (well under a second); a waiter
reaps the child, drains the pump, and marks the task exited. Cancel (Ctrl-C)
still kills the foreground command; background tasks die on session shutdown
(2E) only.

## Kill (2D: `shell_kill(task_id=… | pid=… | port=…)` — exactly one)

Task kills signal the group gray created (SIGTERM → 2 s → SIGKILL);
foreign pid/port kills ask the user first (fail-closed) and signal the
single pid only — never a group, never pid ≤ 1 / self / own group.

```text
t8 (pid 4310) terminated after 0.2s
```

```text
t9 already exited (exit 0) — nothing signalled
```

```text
kill pid 9912 (python3) needs user approval (foreign process) — refusing
```

```text
port 38471 → pid 9912 (python3): foreign pid 9912 (python3) terminated after 0.1s
```

A task whose pid was reused (or is gone) is refused, never signalled:

```text
pid 4310 is gone or was reused; not signalling
```

## 2B decisions for the orchestrator

- Foreground never blocks longer than `MAX_TIMEOUT_SECS` (600 s): the REPL
  must set `Agent::with_tool_timeout` ≥ 610 s (2E owns; alternative: lower
  `MAX_TIMEOUT_SECS` to 110).
- Until 2E merges, REPL exit orphans running tasks (`shutdown_session`
  sweeps them once wired). In `-p` print mode there is no later turn, so
  `background=true` / promoted tasks would be orphaned — decision pending.

## shell_output — cursor reads, wait modes, list (2C)

Args: `task_id?` (e.g. `"t2"`), `from_offset=0`, `wait="none"|"output"|"exit"`,
`timeout=30` (1..=600), `max_bytes=16384` (≤51200).

No `task_id` → list, one line per task (id-sorted) plus a summary line:

```text
t1 · exit 0 · ran 2.1s · finished 5s ago · log ~/.gray/shell/nosession/t1.log
t2 · running · pid 4250 · 12s · log ~/.gray/shell/nosession/t2.log
2 tasks this session. shell_output(task_id="t2") to read output.
```

Empty registry: `no tasks this session. bash(background=true) starts one.`

Read (running):

```text
t2 · running · pid 4250 · 12s · bytes 0–128 of 128 · next_offset=128
<untrusted-output task="t2">
line 1
line 2
</untrusted-output>
```

Read (exited — same shape, verdict first):

```text
t1 · exit 0 · ran 2.1s · finished 5s ago · bytes 0–46 of 46 · next_offset=46
<untrusted-output task="t1">
bg-hi
</untrusted-output>
```

`wait="output"` blocks until the log grows past `from_offset`;
`wait="exit"` blocks until the task exits (both park on watch channels,
never poll). `wait="exit"` past `timeout` returns what is new plus:

```text
still running after 1s; call again with wait=exit
```

Nothing new with `wait="none"` (the anti-polling string — never silence):

```text
no new output for t2 (log is 128 bytes, next_offset=128). Do not poll: call again with wait="output" to block until the next write, wait="exit" to block until exit, or sleep(seconds) and continue working.
```

Offset past the end is a note, not an error (`is_error == false`):

```text
offset 999 is past the end (log is 128 bytes). Retry with from_offset=128 or 0.
```

Unknown id is an error that lists what exists (`is_error == true`):

```text
unknown task "t9" this session. Known tasks: t1, t2.
```

Windows over 50 KiB go through `middle_out` with the window's start as base
offset, so the marker's `from_offset` stays absolute and pages chain via
`next_offset`. A window that ends mid-line on a running task keeps the bytes
and adds `(last line incomplete — the task is still writing it)`.

## sleep + notify_on + usage guidelines (3B+3D)

`sleep(seconds, reason?)` waits without holding a process or spending turns:
`seconds` 1..=600 required, `reason` optional (shown in the UI). Never
`is_error`. Wakes early on this session's task exit / `notify_on` match, on
any user typing, or on cancel:

```text
slept 60s · no events
```

```text
slept 0.5s of 60s · woken early: t5 exit 0
```

```text
slept 0.3s of 60s · woken early: user typed
```

```text
sleep cancelled after 0.5s
```

Dropped wake events (lagged broadcast) wake with
`{n} task events were dropped; shell_output() to list` instead of sleeping
through them. For servers/watchers: `bash(background=true,
notify_on='error|ready')`, then `sleep` or keep working — never poll with
sleep+tail.

`notify_on` is a regex (size-limited, 1 MiB) matched against complete log
lines. Invalid → the tool fails with the regex error and an example.
Rate-limited: ≥ 10 s between wakes per task, max 5 per task, then disabled —
the log gains `[gray: notify_on disabled after 5 matches]` plus one final
`notify_on disabled for tN after 5 matches; read the log directly` wake.
Only meaningful with `background=true` or after promotion; a short
foreground command that exits before matching ignores it silently.

Usage guidelines (system prompt, ≤ 6 bullets — every word ships on every
request):

- bash output: first line is the verdict (exit, duration, size, log path).
  Non-zero exit is data, not a tool error; read the header.
- Long or server-like commands: background=true. You are woken when they
  exit; do not poll.
- Read new output with shell_output(task_id, from_offset=\<next_offset\>,
  wait='output'|'exit'). Never re-read the same bytes.
- Port in use: shell_kill(port=N). Stop a task: shell_kill(task_id).
- Waiting on something: sleep(seconds) — it ends early when anything happens.
- Truncated output names the log path; grep the log instead of rerunning.
