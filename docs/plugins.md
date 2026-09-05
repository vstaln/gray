# gray plugins — authoring guide

Sidecars are child processes speaking newline-delimited JSON over stdio.
Frozen wire spec: [`protocol-v1.md`](protocol-v1.md) (v1.1).
Machine schemas: [`schema/manifest.v1.json`](schema/manifest.v1.json),
[`schema/protocol.v1.json`](schema/protocol.v1.json).
Reference implementation: [`plugins/echo/echo.sh`](../plugins/echo/echo.sh)
(copy it as your starting point).

## Manifest (`plugin/manifest` → result)

```json
{"name":"echo","version":"0.1.0","protocol":"1.1",
 "tools":[{"name":"echo","description":"Echo text back",
   "parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]},
   "snippet":"echo <text>"}],
 "commands":["/echo"],
 "hooks":["turn/end"],
 "capabilities":[],
 "subcommands":[]}
```

- `name` (non-empty, else boot bails), `version`, `tools` are required.
  Pre-v1 `"tools":["name"]` bare strings still parse.
- `commands` are literal `/`-prefixed names routed from the REPL.
- `hooks`: `prompt/context`, `tool/before` gate requests; `turn/end` is
  informational (events arrive as `event/notify` regardless).
- `protocol: "1.1"` opts into `plugin/shutdown` + `session` params.
  Absent = pre-v1: unknown lines ignored, never sent shutdown.
- `capabilities`: advisory sandbox declaration (`exec`, `http`,
  `session`, `ui`). Parsed, surfaced in `--dump-manifest`, schemad —
  **not enforced yet**.
- `subcommands` (e.g. `/cron`): host-owned namespaces the plugin extends.
  Argv forwards over the same `command/run` wire as `commands`.

## Methods and TTLs

| Method | Direction | Kind | TTL | Params → result |
|---|---|---|---|---|
| `plugin/manifest` | host→sidecar | request | 30 s | — → manifest |
| `tool/call` | host→sidecar | request | 30 s | `{name,args,session}` → `{content,is_error?}` |
| `prompt/context` | host→sidecar | request, gated on `hooks` | 30 s | `{cwd,session}` → `{text}` |
| `tool/before` | host→sidecar | request, gated on `hooks` | 30 s | `{name,args,session}` → `{decision: allow/deny/modify,…}` |
| `command/run` | host→sidecar | request, gated on `commands`/`subcommands` | 30 s | `{name:"/x",argv,session}` → `{text}` **or** `{prompt}` |
| `event/notify` | host→sidecar | notification (no `id`, no reply) | 5 s write | `{type: pre_step/pre_tool/post_tool/turn_end,…}` |
| `plugin/shutdown` | host→sidecar | notification, v1.1 only | 5 s write | `{reason: session_end}` — exit promptly |
| `host/run` | sidecar→host | request (**string** `id`) | 30 s | `{session,prompt}` → `{text}` or `{error}` |
| `host/say` | sidecar→host | request (**string** `id`) | 30 s | `{text}` → `{ok:true}` or `{error}` |

Rules: host ids are numbers, sidecar ids are strings — the namespaces
never collide. Gated methods are only sent to sidecars claiming them, so
pre-v1 plugins (ignore unknown lines) keep working. `command/run`:
`{"prompt"}` wins over `{"text"}` — prompt makes the host run a turn,
text just prints. Without a host handler, `host/*` replies `{"error":…}`
(loud, never a hang). Both hosts install a real runner
(`gray::host::default_handler`, gateway `cron_host_handler`): `host/say`
queues for display (REPL-loop drain) or logs + saves under
`cron/output` (gateway); `host/run` replays the prompt through a fresh
`gray -p` child of the running binary and returns its stdout as
`{"text"}` (shared core: `gray_plugin::host::run_prompt_child`).
Ceiling: the 30 s per-request TTL still applies — a longer turn reports
a loud timeout (its side effects already happened).

Every v1.1 request/notification carries
`"session": {"id": <id or "">, "cwd": <cwd>}`.

## Check your plugin

```sh
gray plugin check ./my-plugin   # spawn + manifest + tool/call + notify + shutdown
python3 docs/schema/validate.py # reference-plugin vs schema (also in CI)
```

`check` resolves the argv from a directory (the dir itself when
executable, else `plugin.sh`, else the single executable inside) and
fails nonzero with per-check PASS/FAIL lines. Test the hang/crash/
reorder/empty-name modes against the fixtures in
`crates/gray-plugin/testdata/`.

## Publish

Ship a directory with an executable (see `plugins/echo/`); users enable
it via `gray.yml` sidecar entries. Keep the manifest honest (only claim
hooks/commands you answer) and exit 0 on `plugin/shutdown`.

## Links

- Cron ([`plugins/cron/cron.sh`](../plugins/cron/cron.sh), exec wrapper
  over the `gray-cron-sidecar` binary): the scheduler lives in the
  sidecar — same store/parser as in-process `gray-cron` (no
  reimplementation), manifest `capabilities:["session"]` +
  `subcommands:["/cron"]`, real `cron.add`/`cron.list`/`cron.remove` +
  `/cron` argv, due jobs fire via `host/run` and report via `host/say`
  (first scan waits a full 60 s tick so `plugin check`/`-p`/manifest
  dumps exit clean). The gateway keeps its claim-guarded in-process
  ticker until the sidecar goes persistent (owed); every ticker claims
  atomically (`store::claim_job_run`), so concurrent firers never
  double-run.
- Skills (prompt-time context, not sidecars): `crates/gray/src/skills/`.
- Gateway (chat delivery, shares the agent builder): `crates/gray-gateway/`.
