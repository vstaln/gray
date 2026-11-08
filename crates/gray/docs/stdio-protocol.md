# `gray --stdio` — persistent machine protocol

One long-lived `gray` process that drives the **same engine the interactive
REPL uses** — `run_prompt_turn` for turns and `dispatch_command` for slash
commands — over newline-delimited JSON (NDJSON) on stdin/stdout. Built for
embedding surfaces like gray-tui; one process per IDE thread/conversation.

```
gray --stdio [--model M] [--provider P] [--base-url U] [--api-key K] \
             [--session ID | -r [ID] | -c] [--bare] ...
```

`--stdio` conflicts with `-p/--print`, `--input-json`, `--json`,
`--dump-models`, `--dump-manifest`, `--skill`, and subcommands.

stdout carries **only** protocol rows (one JSON object per line). Diagnostics
go to stderr/log. No ANSI, no pickers — interactive-only surfaces
(`/models` with no arg, `/plugins`, connect/login modals, pagers) are
degraded to text or refused with an error, never opened.

## Envelope

Every emitted row:

```json
{"protocol": 2, "seq": 17, "session_id": "abc123",
 "req": 5, "turn_id": "turn-…", "type": "…", "...": "…"}
```

| field        | meaning                                                        |
|--------------|----------------------------------------------------------------|
| `protocol`   | protocol version (`2`)                                         |
| `seq`        | monotonically increasing sequence number                       |
| `session_id` | current session id, or `null` before one exists                |
| `req`        | the request `id` this row was produced for (`null` = ambient)  |
| `turn_id`    | the turn this row belongs to (`null` outside a turn)           |

Requests are `{"type": "…", "id": <any JSON value>, "...": "…"}`. Every
request gets **exactly one** `{"type":"result","id":<same>}` row — except
requests that were *queued* (below), which get a `queued:true` ack and a
second `result` when their deferred work completes: a turn result
(`{"turn_id": …}`) if they ran as their own turn, or `{"steered": true}`
if the running turn consumed them as a steer.

On startup the first row is `{"type":"hello", "gray_version", "cwd",
"model", "effort", "provider_id", "base_url", "unconfigured"}`.

## Requests

| type | fields | behavior |
|------|--------|----------|
| `prompt` | `text`, `attachments?` (paths) | Runs a turn exactly like typing in the REPL (`@mentions` resolve via the same attachment logic). Events stream until `turn_end` + `result`. Mid-turn it is **queued** like typed input (see below). |
| `command` | `line` | Runs any REPL slash command through the real dispatcher (`/help`, `/compact`, `/resume`, `/plugin …`, plugin commands). A leading `/` is added if missing. Output arrives as `say` events. |
| `steer` | `text`, `attachments?` | Queues input; the running turn consumes it as a steer (a `steered` event confirms) or it becomes the next turn. |
| `interrupt` | — | Cancels the running turn (same token SIGINT cancels). `{"interrupted": true|false}` in the result. |
| `approve` | `ask_id`, `answers` | Answers a pending `ask` event. `answers` maps each question `id` to an array of picked option labels. |
| `list_commands` | — | `{commands:[{name,description,aliases,scope,surface,source}]} — builtin registry + plugin commands. |
| `list_models` | `live?` | `{catalog: <same JSON as `gray --dump-models`>}` — provider rail with model rows in picker order, efforts, current/active flags. `live:true` refreshes every provider. |
| `set_model` | `provider?`, `model?`, `effort?` | `provider` uses the `--provider` resolution (scoped config + saved credentials), `model`/`effort` use `/models <id>` semantics incl. variant suffixes and clamping. Rebuilds the agent. |
| `status` | — | `{status:{session_id,cwd,model,effort,provider_id,base_url,context_window,compact_ceiling,context_used,last_turn_usage,totals:{turns,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost_usd,cache_hit_pct},turn_in_flight,queued_inputs,…}}` |
| `list_sessions` | `all?` | `{sessions:[SessionSummary]}` — saved sessions for this cwd (`all:true` = every directory). |
| `resume` | `session` (id or unique prefix), `last?`, `all?` | Dispatches `/resume` through the real handler (locking, history load, model adoption). |
| `shutdown` / `quit` | — | Graceful exit (hooks + shell tasks drained). |

## Events

Turn-local events carry `turn_id` + `req`; ambient events have `req:null`.

| type | fields | meaning |
|------|--------|---------|
| `hello` | version, cwd, model, effort, provider_id, unconfigured | first row after startup |
| `turn_start` | `prompt` | a turn begins |
| `text` | `delta` | assistant text delta |
| `thinking` | `delta` | reasoning delta |
| `tool_start` | `id`, `name` | tool call began |
| `tool_progress` | `id`, `name`, `args_so_far` | streamed partial args |
| `tool_call` | `id`, `name`, `args` | complete tool args |
| `tool_result` | `id`, `name`, `args?`, `output`, `is_error` | tool finished (output truncated ~8K, secrets redacted) |
| `usage` | `step:true`, `usage` | per-step token usage |
| `turn_usage` | `usage`, `stop_reason` | final usage for the turn |
| `steered` | `text` | a queued input was injected mid-turn |
| `compacted` | `tokens_before/after`, `messages_before/after`, `reason` | context compaction happened |
| `retract` | — | streamed text was withdrawn (provider reset) |
| `notice` | `kind` (`prefix_changed`), `cause` | structured provider notes |
| `say` | `level` (`info`/`warn`/`error`), `text`, `kind?` | anything the REPL would print: command output, warnings, retries, background-job/cron notices |
| `ask` | `ask_id`, `questions:[{id,header,question,detail,options:[{label,description}]}]` | approval/question pending — answer with `approve` |
| `turn_end` | `ok`, `interrupted`, `usage` | turn finished (errors surface as `say level:error` before this) |
| `result` | `id`, `ok`, `error?`, `queued?`, payload | request reply; always arrives |
| `error` | `code`, `message` | protocol-level problems (e.g. `bad_json`) |

## Concurrency rules

- One turn at a time per process (like the REPL). `interrupt`, `approve`,
  `steer` are answered immediately even mid-turn.
- A `prompt`/`command`/`steer` sent mid-turn gets `{"result":{"queued":true}}`
  and joins the input queue — the running turn may consume it as a steer;
  leftovers run after the turn, in order. Queued items keep their request
  `id` in `req` so clients can correlate the deferred `turn_start`.
- Background work (jobs finishing, cron, session sends/mailbox) produces
  `say` events and auto-turns between requests — same as the REPL's idle
  wake. Poll `status` if you need to distinguish idle vs busy.

## Phase-2 notes (IDE client)

- Spawn `gray --stdio` per thread; treat every line as `{"type":...}`.
- `result` is the only required correlation; `req` on events simplifies
  attribution without tracking turn ownership.
- Interactive-only commands: `/models` (no arg), `/plugins`, `/sessions`
  (no target), connect/login flows emit `say` guidance or an error result —
  route users to `list_models`/`set_model`/`resume` instead.
- `gray --dump-models [live]` and `gray --provider <id>` are the one-shot
  siblings of `list_models`/`set_model` (same code path).
