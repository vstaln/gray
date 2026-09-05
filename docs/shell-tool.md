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

## Timeout, Phase 1 (kill arm; 2B replaces with promotion)

```text
killed after 1s (timeout) — output preserved
exit 143 (SIGTERM) (terminated (SIGTERM)) · 1.0s · 2 lines · log ~/.gray/shell/nosession/t6.log
<untrusted-output task="t6">
tick 1
tick 2
</untrusted-output>
```

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

## Background flag (until 2B)

Appends `\n(background not yet available — ran in foreground)` and runs foreground.
