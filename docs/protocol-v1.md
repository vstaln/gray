# Gray sidecar plugin protocol — v1

Spec for the host↔sidecar wire as merged in PR #22, audited against
`origin/main @ cbaaeea` (2026-09-05). Conformance example:
[`plugins/echo/echo.sh`](../plugins/echo/echo.sh).
What adapters (`gray-pkg`) and the gateway repo build on.

## Wire spec (mirrors `sidecar.rs` — see sync note)

<!-- sidecar-doc-start -->
Sidecar plugin protocol (v1). See `docs/protocol-v1.md` for the
versioned spec (methods, TTLs, gating, host-emission audit).

Transport: newline-delimited JSON over child stdio. Requests the host
sends are `{"id", "method", "params?"}`; sidecars reply with
`{"id", "result"}` for request/response methods only:
- `plugin/manifest` (request): no params, reply
  `{"name","version","tools":[{"name","description","parameters","snippet"}],
  "commands":["/x"],"hooks":[...]}`. Pre-v1 `"tools":["name"]` still parses.
- `tool/call` (request): params `{"name","args"}`, reply `{"content","is_error?"}`.
- `prompt/context` (request): params `{"cwd"}`, reply `{"text"}`.
- `tool/before` (request): params `{"name","args"}`, reply allow/deny/modify.
- `command/run` (request): params `{"name":"/x","argv"}`, reply `{"text"}`.
- `event/notify` (notification): NO `id`, NO reply expected. Params carry a
  minimal tagged event `{"type", ...}` where type is one of
  `pre_step` | `pre_tool` | `post_tool` | `turn_end` with only the fields
  the sidecar needs (tool name/args, output content, usage totals).

Unknown methods/lines are ignored.

The three v1 request methods are only sent to sidecars claiming them in
`hooks`/`commands`, so pre-v1 sidecars (which ignore unknown lines, hence
would never reply) keep working.

Concurrency: one reader task per sidecar routes replies by `id` into
`pending`; writers take a short stdin lock only, so concurrent requests
resolve out of order instead of serializing on one mutex.
<!-- sidecar-doc-end -->

Sync note: the block above is byte-identical to the `//!` doc comment in
`crates/gray-plugin/src/sidecar.rs` (modulo the `//!` prefix). Verify with
(anchored markers — the check text itself quotes them, so an unanchored
range would match twice):

```sh
diff <(sed -n "/^\\/\\/!/p" crates/gray-plugin/src/sidecar.rs | sed "s#^//! \?##") \
     <(awk '/^<!-- sidecar-doc-start -->$/{f=1;next} /^<!-- sidecar-doc-end -->$/{f=0} f' docs/protocol-v1.md)
```

## Methods: TTLs and manifest gates

| Method | Kind | TTL | Sent only when | Code |
|---|---|---|---|---|
| `plugin/manifest` | request, no params | 30 s | always (spawn handshake) | `sidecar.rs:177` |
| `tool/call {name,args}` | request → `{content,is_error?}` | 30 s | always (tool invoked) | `sidecar.rs:311` |
| `prompt/context {cwd}` | request → `{text}` | 30 s | manifest `hooks` contains `prompt/context` | `sidecar.rs:206` |
| `tool/before {name,args}` | request → allow/deny/modify | 30 s | manifest `hooks` contains `tool/before` | `sidecar.rs:220` |
| `command/run {name:"/x",argv}` | request → `{text}` | 30 s | manifest `commands` contains the name | `sidecar.rs:236` |
| `event/notify {type,…}` | notification, no id, no reply | 5 s write | unconditional (pre-v1 sidecars ignore it) | `sidecar.rs:247` |

Reader routing: one reader task per child, replies matched by numeric `id`;
lines without a known `id` are DROPPED — there is no plugin→host channel
in v1. Child death respawns (`kill_on_drop(true)`); in-flight requests of a
dead generation fail fast. Teardown is SIGKILL only — no graceful shutdown
(gap, see Host emission).

## Manifest shape

From [`plugins/echo/echo.sh`](../plugins/echo/echo.sh) (reference):

```json
{"name":"echo","version":"0.1.0",
 "tools":[{"name":"echo","description":"Echo text back",
   "parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]},
   "snippet":"echo <text>"}],
 "commands":["/echo"],
 "hooks":["turn/end"]}
```

Notes: `hooks` entries used by the host are `prompt/context` and
`tool/before`; `turn/end` is informational (nothing gates on it).
`commands` entries are literal `/`-prefixed names. Pre-v1
`"tools":["name"]` (bare strings) still parses.

Fixtures (`crates/gray-plugin/testdata/`):
`hooks_plugin.sh` (claims `prompt/context`, `tool/before`),
`hang_plugin.sh` (never replies — timeout tests),
`crash_plugin.sh`, `reorder_plugin.sh`, `empty_name_plugin.sh`,
`echo_plugin.sh`, plus profile fixtures `gray.yml` / `gray_sidecar.yml`.

## Host emission (audited 2026-09-05 @ `cbaaeea`)

| Event / request | Emitter | Status |
|---|---|---|
| `plugin/manifest` | `SidecarPlugin::spawn` (`sidecar.rs:174`) | ✅ emitted |
| `tool/call` | `SidecarTool::execute` (`sidecar.rs:311`); `ToolContext` discarded (`_ctx`) | ✅ emitted, but sidecar never learns cwd/session (F10) |
| `prompt/context` | agent loop via `PluginHooks::prompt_context()`; adapter pins boot cwd (`lib.rs:169`) | ✅ emitted |
| `tool/before` | agent loop via `PluginHooks::tool_before()`; fail-open on RPC error | ✅ emitted |
| `command/run` | REPL `run_plugin_command()` (`repl/mod.rs:104`), reached from `ReplCommand::Unknown`; reply printed via `say()`, never submitted | ✅ emitted |
| `event/notify` (`pre_step`/`pre_tool`/`post_tool`/`turn_end`) | **none** — zero `CoreEvent` constructions outside the `sidecar.rs` serializer; `PluginHooks` (`gray-core/src/agent.rs:174`) has no event method; `PluginHookAdapter` forwards none | ❌ **gap → Task 2.0**: emit `PreTool`/`PostTool` (bubble content) and `TurnEnd` (bubble deletion) from the agent loop |
| `ToolContext` fields | `{cwd, cancel, questions}` (`gray-core/src/agent.rs:64`) — **no session id** | ❌ **gap → Task 2.0**: additive `Option` field; 6 construction sites (`gray/src/{print.rs, repl/mod.rs ×4}`, gateway `daemon.rs`/`daemon_agent.rs`); `Agent::run` signature untouched, so the gate STOP condition does not trigger |

Gate impact: gate row 5 ("bubble content") flips **yes → change** — it needs
the same Task 2.0 emission work as row 6. Row 6 (`turn_end`) stays **change**.
Rows 1 (`process-per-session` spawn inside `build_agent`), 3 (endpoint via
`prompt/context`), 7 (timeouts) stand as **yes**.

## Consumers

**REPL** (`crates/gray/src/lib.rs:280`): `profile_plugins()` →
`active_plugins()` → `Registry::from_plugins` +
`PluginHookAdapter::for_plugins(&plugins, cwd)`; sidecars spawn inside
`build_agent()` (once per agent construction); `Agent::with_hooks`.
`/new` and quit do not shut sidecars down (SIGKILL on drop only).

**Gateway** (`crates/gray-gateway/src/daemon_agent.rs:14`): `build_agent()`
uses `Registry::builtin()` and `Agent::new` with **no** `with_hooks`; zero
`gray_plugin` references anywhere in the crate (F11 confirmed). Session runs
live in `daemon_agent.rs:60`; streaming in `daemon_stream.rs` (`Streamer` —
PR #23's `ProgressBubble` unmerged at audit time). After extraction the
gateway must build agents with `gray_plugin::boot` (Task 2.1/3.1) to get
hooks at all.

## v1.1 (additive)

Everything in v1 keeps working: v1.1 only adds a notification, a reply
variant, a params object, and a manifest field. Pre-v1 sidecars (no
`protocol` field, ignore unknown lines) never see new traffic they must
answer, so no v1 behavior changes.

- `plugin/shutdown` (notification, no `id`, no reply) — **additive,
  gated**: params `{"reason": "session_end"}` (future: `host_exit`,
  `reload`). Sent only to sidecars whose manifest claims `protocol`
  (pre-v1 `protocol: None` never receives the line). Host waits a short
  grace for voluntary exit, then kills what remains. Reference
  (`plugins/echo/echo.sh`) exits 0 on receipt. Code: `sidecar.rs`
  `SidecarPlugin::shutdown`, `Plugin::shutdown` (default no-op).
- `session` object — **additive, ungated**: every v1.1 request/notification
  params carries `"session": {"id", "cwd"}` (`prompt/context`,
  `tool/before`, `command/run`, `tool/call`, `event/notify`). Extra field
  only — old sidecars ignore it. `tool/call` reads both from
  `ToolContext` (`cwd` + `session_id`); all other wire points use the
  pinned boot cwd and `""` (no `ToolContext` there to read).
- `command/run → {prompt}` — **additive, ungated**: reply variant
  alongside v1's `{"text"}`. `{"prompt"}` wins when both are present and
  non-empty; empty/missing stays unhandled (`None`). REPL routing
  (`repl/mod.rs` `run_plugin_command` → `ReplCommand::Unknown`):
  `Say(text)` prints via `say()` (v1 behavior); `Prompt(text)` is queued
  as `pending_command = ReplCommand::Prompt` — the same dispatch a typed
  prompt takes, no turn logic duplicated.
- `turn_end` emission — **additive, ungated**: the agent loop fans out
  `PluginHooks::turn_end(usage)` on every turn exit (ok plus all
  error/cancel/stall paths; best-effort, never fails the turn) →
  `event/notify {"type":"turn_end","usage":...,"session":...}`. Plain
  notification, so pre-v1 sidecars drop it harmlessly. Closes the v1
  "zero `turn_end` constructions" gap; `pre_step` stays unemitted.
- `manifest.protocol` — **additive**: `"protocol":"1.1"` in the
  `plugin/manifest` result (absent = pre-v1). Today it gates only
  `plugin/shutdown` delivery.

Gate verdict: **PASS** — teardown (`plugin/shutdown`) + `TurnEnd`
emission closed by this PR and proven green: `lifecycle.rs` 4/4
(`two_sidecars_produce_distinct_endpoints`,
`shutdown_one_leaves_other_alive`,
`pre_v1_fixture_survives_shutdown_without_hanging`,
`bubble_lines_then_cleared`) plus `turn_end_hook_called_once_on_end_and_on_error`,
`pre_post_hooks_emit_around_tool_execution`,
`plugin_command_prompt_reply_takes_prompt_path`. Note: gate row 5 flipped
to **change** during the ① audit (host never emitted `PreTool`/`PostTool`
either) and is closed by the same emission work. Streaming text +
plugin-initiated turns remain deferred to v2 as designed.

## Appendix: edges to cut + publishing path

Dependency edges (via `cargo tree`, audit date):
`gray → gray-gateway`, `gray → gray-cron`, `gray-tools → gray-cron`,
`gray-gateway → gray-cron` (F1: cron moves **with** the gateway in ③).
`gray → gray-gateway` pulls twilight into every workspace build today
(13 twilight/teloxide/slack-morphism nodes under `cargo tree -p gray`;
Task 3.0 cuts this edge first). `gray-plugin` has no `reqwest` —
networking stays out of the protocol crate (invariant for `gray-pkg`).

Publishing (`gray.alignment.id`, F2): `release.yml` pushes `main` →
beta tarballs under `/dl/gray-beta-*`, tags → stable + GitHub Release;
`dist/install.sh` is scp'd to `/var/www/gray/install.sh`
(`update.rs: BASE = https://gray.alignment.id/dl`, installer URL
`https://gray.alignment.id/install.sh`, asserted in `update.rs` tests).
The plugin index rides the same mechanism (`scripts/deploy.sh` pattern):
serve `index.json` at `gray.alignment.id/plugins/index.json` without moving
the installer URLs.
