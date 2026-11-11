# `gray --stdio`

One long-lived `gray` driving the same engine the REPL uses —
`run_prompt_turn` for turns, `dispatch_command` for slash commands — over
newline-delimited JSON on stdin/stdout. Built for gray-gui: one process per
IDE thread.

```
gray --stdio [--model M] [--provider P] [--base-url U] [--api-key K] \
             [--session ID | -r [ID] | -c] [--bare]
```

stdout carries protocol rows only; diagnostics go to stderr. Interactive-only
surfaces (`/models` with no arg, `/plugins`, login flows, pagers) degrade to
`say` text or an error result.

## Envelope

```json
{"protocol": 2, "seq": 17, "session_id": "abc123",
 "req": 5, "turn_id": "turn-…", "type": "…", "...": "…"}
```

`seq` is monotonic; `session_id` is null before a session exists; `req` is the
request id that produced the row (null = ambient); `turn_id` is null outside a
turn. First row is `hello`: `gray_version`, `cwd`, `model`, `effort`,
`provider_id`, `base_url`, `unconfigured`.

Requests are `{"type": "…", "id": <any JSON>}`. Every request gets exactly one
`result`. Requests *queued* mid-turn first get `{"queued": true}`, then a
second `result` when their work completes: `{"turn_id": …}` if they ran as
their own turn, `{"steered": true}` if the running turn consumed them.

## Requests

| type | fields | behavior |
|------|--------|----------|
| `prompt` | `text`, `attachments?` | a turn, like typing in the REPL (`@mentions` resolve the same way) |
| `command` | `line` | any slash command through the real dispatcher; `/` added if missing; output as `say` |
| `steer` | `text`, `attachments?` | queue input for the running turn |
| `interrupt` | — | cancel the running turn, as SIGINT does |
| `approve` | `ask_id`, `answers` | answer an `ask`; `answers` = `{question_id: [label, …]}` |
| `list_commands` | — | builtin registry + plugin commands |
| `list_models` | `live?` | same JSON as `gray --dump-models` |
| `set_model` | `provider?`, `model?`, `effort?` | `/models` semantics, incl. variant suffixes and clamping |
| `status` | — | session/model/context/usage snapshot, `turn_in_flight`, `queued_inputs` |
| `list_sessions` | `all?` | saved sessions for this cwd |
| `resume` | `session`, `last?`, `all?` | `/resume` through the real handler |
| `shutdown` / `quit` | — | graceful exit: hooks then shell tasks |

## Events

Turn-local events carry `turn_id` + `req`; ambient ones carry `req: null`.

| type | fields |
|------|--------|
| `turn_start` | `prompt` |
| `text`, `thinking` | `delta` |
| `tool_start` | `id`, `name` |
| `tool_progress` | `id`, `name`, `args_so_far` |
| `tool_call` | `id`, `name`, `args` |
| `tool_result` | `id`, `name`, `output`, `is_error` (output ~8K, secrets redacted) |
| `usage` | `step: true`, `usage` |
| `turn_usage` | `usage`, `stop_reason` |
| `steered` | `text` |
| `compacted` | `tokens_before/after`, `messages_before/after`, `reason` |
| `retract` | — streamed text withdrawn by the provider |
| `notice` | `kind`, `cause` |
| `say` | `level`, `text`, `kind?` — anything the REPL would print |
| `ask` | `ask_id`, `questions` — answer with `approve` |
| `turn_end` | `ok`, `interrupted`, `usage` (errors arrive as `say` before it) |
| `result` | `id`, `ok`, `error?`, payload |
| `error` | `code`, `message` |

## Concurrency

One turn at a time per process, like the REPL. `interrupt`, `approve` and
`steer` are answered immediately even mid-turn. `prompt`/`command`/`steer`
sent mid-turn are queued: the running turn may consume them as a steer,
leftovers run in order after it, and the deferred `turn_start` carries their
original `req`. Background work (jobs, cron, mailbox) emits `say` and may run
auto-turns between requests — poll `status` to tell idle from busy.

## Client notes

Spawn one process per thread and correlate on `result`; `req` on events saves
tracking turn ownership. Don't drive no-arg `/models`, `/plugins` or
`/sessions` — use `list_models` / `set_model` / `resume`.
