# Tool output: squeeze, spill, recover

A tool result is text the model pays for twice — once when it enters the
context, and again on every later turn while it sits there. gray now spends it
once: compress what the command is, keep the rest recoverable, and measure what
that saved.

## What a result goes through

1. **Cap** (`crates/gray-core/src/tool_out.rs`) — 2000 lines / 50 KiB, head +
   tail, error outputs a hard 2 KiB. Unchanged, and still the floor.
2. **Squeeze** (`crates/gray-core/src/squeeze.rs`, applied in the bash tool) —
   over 2 KiB, runs of lines carrying no information collapse into a count.
   `cargo`, `npm`/`pnpm`/`yarn`/`bun`, `pip`/`pip3`/`uv` and `pytest` have their
   own rules; everything else gets the generic one. The rule only applies when
   *every* pipeline segment of the command is the same family — `cargo build |
   grep error` is two families, so grep's output is never squeezed as if it
   were cargo's. A rule that cannot shrink the text declines.
3. **Spill** (`crates/gray-core/src/spill.rs`) — if the cap still cut something,
   the full original is written to `$GRAY_HOME/spill/<handle>.txt` (0600) and
   the preview grows a `[spilled …]` footer naming the handle. The bash tool
   already wrote its full output to `$GRAY_HOME/shell/<session>/…log` before
   this; the store is what the *other* tools get, and it keeps a uniform,
   greppable handle for everything.

The squeeze runs on the inline body only. The shell log is the recovery path
and is never rewritten — the paging hints (`sed -n 'A,Bp'`, `dd bs=1`) address
line numbers in that log, so the window is chosen first and the line numbers
stay anchored to the log.

## Reading a spilled result back

```
gray spill grep <handle> <pattern> [-n CONTEXT] [-i] [-F] [--limit N]
gray spill head <handle> [-n N]
gray spill tail <handle> [-n N]
gray spill stats
```

A subcommand, not a tool: the model already has a shell, and a tool entry is
schema every turn pays for (and a prompt-cache miss on every call).

Handles are 16 hex characters of the SHA-256 of the content — every character
is checked before it becomes a path, the same content always yields the same
handle, and the store keeps the newest 256. An evicted or unknown handle is an
error that says what to do, never an empty success.

## What it saved

`gray spill stats` reports produced-vs-sent, broken down by rule, biggest saving
first. `/usage` shows the same counterfactual as one line. Tokens are estimated
the way the context gauge estimates them (`bytes / 4`) — the comparison is what
matters, the number is not an invoice.

## Trade-offs, stated

- **Spill files hold raw tool output**, exactly like the session transcripts
  beside them and the shell logs gray already writes: same `$GRAY_HOME`, same
  0600, same trust domain. Redaction happens on the way into the model's
  context, not on the way to disk — a scrubbed original would hand back text
  that never existed. `GRAY_NO_SPILL=1` turns the store off entirely and falls
  back to plain truncation.
- **The generic rule counts repeated lines.** Three identical lines become
  `line ×3`. That is a statement about the output, not a guess about intent, and
  the log holds the original.
- **`GRAY_NO_SQUEEZE=1`** disables compression only; spills and the cap stay.
- Background-job logs go through the job status path, which has its own resume
  hints, and are not squeezed.

## What is deliberately not here

Per-command compression for `docker`/`kubectl`/`git` (their output is already
tabular and a rule would buy nothing), an ML summariser (it can stall a turn and
regress silently), and any new tool in the tool schema.
