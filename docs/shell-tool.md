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

## 2B decisions for the orchestrator

- Foreground never blocks longer than `MAX_TIMEOUT_SECS` (600 s): the REPL
  must set `Agent::with_tool_timeout` ≥ 610 s (2E owns; alternative: lower
  `MAX_TIMEOUT_SECS` to 110).
- Until 2E merges, REPL exit orphans running tasks (`shutdown_session`
  sweeps them once wired). In `-p` print mode there is no later turn, so
  `background=true` / promoted tasks would be orphaned — decision pending.
