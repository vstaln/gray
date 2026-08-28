# Port recon: tui_gateway / acp_adapter / cron

Source: `reference/NousResearch/hermes-agent` (Python). Labels: PROVEN = read directly; CONJECTURED = inferred from names/structure, not fully read.

## 1. tui_gateway (~32k LOC)

### Inventory (line counts PROVEN via wc -l)
| File | LOC | Role |
|---|---|---|
| server.py | 16,242 | Core: JSON-RPC dispatch table (`_methods: dict[str, callable]`, registered via `@method("name")` decorator), session registry/lifecycle (claim/release slots, teardown, orphan reaping with grace timers), agent subprocess management, clarify/approval pending-response maps |
| methods_session.py | 3,633 | session.* RPCs (create/list/resume/history/interrupt/branch/compress/undo/steer/usage…) |
| methods_tools.py | 2,579 | tools.*, process.*, rollback.* |
| methods_prompt.py | 1,626 | prompt.submit/background, input handling, file/image/pdf attach |
| methods_profiles.py | 1,079 | profiles.* (model personas + assets) |
| compute_host.py | 899 | remote compute host shell sessions |
| project_tree.py | 793 | projects.tree / repo discovery |
| host_supervisor.py | 577 | supervisor for compute hosts |
| methods_complete.py | 626 | complete.path, complete.slash (completions) |
| methods_config.py | 558 | config.get/set/show, reload.env/mcp, skills.manage |
| entry.py | 500 | CLI entrypoint: spawns gateway, wires transports |
| ws.py | 548 | WebSocket transport (starlette-style `/api/ws` app) |
| methods_browser_control.py | 382 | browser.controller.register/heartbeat/result/detach (remote browser control relay) |
| mcp_oauth_sessions.py | 339 | mcp.servers.oauth.start/poll |
| slash_worker.py | 196 | background slash-command worker thread |
| Others (<250 each): transport.py (Transport protocol: StdioTransport newline-framed JSON over stdout, TeeTransport fan-out), event_publisher.py (best-effort back-WS mirror to dashboard `/api/pub` — plain newline-framed JSON dicts, NOT JSON-RPC, queue max 256, drop-on-full), turn_marker.py, synthetic_turn.py, loop_noise.py (event filtering), render.py, git_probe.py, method_ctx.py (per-RPC context helper), _stdin_recovery.py |

### RPC surface (PROVEN — grepped all `@method(...)` registrations, ~150 methods)
Namespaces: `agents.list`, `approval.pending/received/respond`, `billing.*` (charge/state/auto_reload/step_up), `bot_relay.deliver/reply/outbox.drain/roster.sync`, `browser.*`, `clipboard.paste`, `command.dispatch/resolve`, `commands.catalog`, `complete.path/slash`, `config.*`, `cron.manage`, `delegation.pause/status`, `diagnostics.share_nous`, `file.attach`, `handoff.request/state/fail`, `image.*` (attach/attach_bytes/detach/generate), `input.detect_drop`, `insights.get`, `learning.*`, `llm.oneshot`, `mcp.*` (catalog/servers CRUD/oauth/test/setup.respond), `message.react`, `model.*` (options/save_key/disconnect), `paste.collapse`, `pdf.attach`, `pet.*` (16 pet-game methods), `plugins.*`, `preview.*`, `process.list/stop/kill`, `profiles.*`, `project.facts`, `projects.*`, `prompt.submit/background`, `reload.env/mcp`, `rollback.*`, `secret.respond`, `session.*` (~20), `setup.status/runtime_check`, `shell.exec`, `cli.exec`, `skills.*`, `slash.exec`, `spawn_tree.*`, `subagent.interrupt/steer`, `subscription.*`, `sudo.respond`, `system.battery`, `terminal.read.respond/resize`, `tools.configure`.

### Wire protocol (PROVEN)
- **JSON-RPC 2.0** over newline-framed JSON on stdio OR WebSocket (`ws.py`). Request: `{jsonrpc?, id, method, params}`; params must be object (else -32602). Unknown method → -32601. Errors: `{jsonrpc, id, error: {code, message, data?}}`. Responses carry same id.
- **Events to TUI**: outbound objects `{"jsonrpc":"2.0","method":"event","params":{...}}` (server.py:2003–2015). Inbound `"method":"event"` also recognized.
- Dispatch is **synchronous in-process** (`handle_request` → `_methods[method](rid, params)`); long ops use `_pending` maps + threading.Event for async resolution (clarify, approval, secret, sudo respond).
- event_publisher mirrors raw event dicts (no envelope) to dashboard.

### External deps
- `websockets` (sync client API only, in event_publisher). ws.py uses starlette websockets (server-side).
- stdlib-heavy: sqlite3, threading, concurrent.futures ThreadPoolExecutor, atexit.

## 2. acp_adapter (~6k LOC)

### Inventory
- server.py (2,640): ACP Agent-side implementation using the official `acp` PyPI package + its schema types. Handlers: initialize, authenticate, newSession/loadSession, prompt, setSessionModel/Mode/ConfigOption, listSessions, forkSession/resumeSession. Converts ACP content blocks (text/image/audio/resource/embedded) ↔ OpenAI-style user content. Sends updates: SessionInfoUpdate, UsageUpdate, AvailableCommandsUpdate, model/mode state.
- tools.py (1,348): maps hermes tool calls into ACP permission/tool-call events.
- session.py (695): SessionState, SessionManager bridging to the core agent loop.
- edit_approval.py (338), permissions.py (190): edit approval policy + permission request flow.
- entry.py (282) + __main__.py (5): stdio launch. events.py (279), provenance.py (127), auth.py (79).

### Wire protocol (PROVEN structure, CONJECTURED details)
- **Agent Client Protocol** over stdio JSON-RPC (the `acp` package's framing — bidirectional: client→agent requests like initialize/prompt, agent→client requests like session/update, requestPermission). Rust port needs an ACP crate or hand-rolled JSON-RPC stdio pair matching acp-python's framing; check Zed's `agent-client-protocol` Rust crate (same spec).
- Rich metadata extensions in `meta` fields (provenance, usage).

### External deps
- **`acp`** (PyPI) — the whole protocol layer. Rust: use `agent-client-protocol` crate or reimplement schema types (~40 schema structs imported).
- rich (display), starlette not needed here.

## 3. cron (~16k LOC)

### Inventory
- scheduler.py (7,641): main loop — due-job scan, inflight guard (running-job registration `try_register_running_job`, stale-inflight sweep, forced release), failure-streak nudging, prompt-injection-block detection (`CronPromptInjectionBlocked`), per-job toolset/reasoning config resolution, delivery summarization.
- jobs.py (3,879): job store — file-based JSON under hermes home with **flock-based locking** (`_jobs_lock`, per-job fire lock + claim fence `fire_claim_fence`), store-path swapping (`use_cron_store`), output dirs per job, skill field normalization. Optional `croniter` for cron expressions (lazy import, ~15ms regex cost noted).
- lifecycle_guard.py (1,271): guardrails around job lifecycle.
- scheduler_provider.py (703): in-process ticker provider (how scheduling is hosted).
- blueprint_catalog.py (799), suggestion_catalog.py (154), suggestions.py (269): catalogs of suggested jobs.
- executions.py (284): run history records. monitor.py (212): health monitoring. notepad.py (187).

### Schedule format (PROVEN from grep)
Jobs are dicts with `schedule` field supporting at least: cron expressions (validated via `croniter`) and interval minutes (`_cron_interval_minutes(expr)` parses interval-style exprs; `_job_interval_minutes(job)`). Oneshot runs exist (oneshot claim TTL). Exact full grammar CONJECTURED — read jobs.py:512 `_schedule_display_for_job` and scheduler due-scan before implementing.

### Wire protocol
None — internal scheduler. Persisted state: JSON job files + execution records + flock files.

### External deps
- `croniter` (optional). Everything else stdlib (threading, fcntl locks implied by "lock file", subprocess to spawn agent turns).

## 4. PyPI → Rust crates
| Python | Rust |
|---|---|
| websockets (sync) | tokio-tungstenite |
| ws.py server side (starlette) | axum (WebSocket upgrade) |
| acp (PyPI) | agent-client-protocol crate (Zed) or hand-rolled serde types |
| croniter | cron / cron_schedule crates |
| rich | ratatui/console reporting as needed (or skip — logging only) |
| threading/ThreadPoolExecutor/concurrent | tokio tasks + Semaphore |
| sqlite3 | rusqlite |
| flock | fs2 or nix::fcntl::flock |

## 5. Port order + risks
1. **cron first** — smallest external surface, no wire protocol, exercises job-store + locking patterns reusable elsewhere. Risk: flock semantics across processes; inflight-guard race logic (scheduler.py 7.6k lines of subtle concurrency — port the guard invariants, not line-by-line).
2. **tui_gateway core** — risk: it's the largest and most stateful (session registry, orphan reaper timers, pending-response maps). Recommend porting dispatch + Transport trait + a *subset* of methods first (session.*, prompt.submit); the ~150-method surface is mostly independent handlers — port lazily per namespace. Threading model must become tokio; the `_pending` Event pattern maps to oneshot channels.
3. **acp_adapter last** — thinnest layer, but depends on both the agent core AND a Rust ACP schema. Risk: `acp` PyPI schema drift vs Zed's Rust crate; verify framing compatibility early with a smoke test against Zed.

Cross-cutting risk: tui_gateway reaches deeply into core modules (tools.approval, db, platform plugins, billing, mcp, profiles...) — those are outside these three subsystems and are the real port dependency chain, not the subsystems' own code.

## 6. Load-bearing vs peripheral
- **Load-bearing**: tui_gateway server.py session-lifecycle + dispatch; transport abstraction; methods_session/methods_prompt (the actual TUI conversation path); acp server/session/tools; cron scheduler + jobs store.
- **Peripheral**: pet.* game (16 methods, pure UI toy), bot_relay, browser.controller relay, image.generate plumbing, git_probe, project_tree, suggestion/blueprint catalogs, monitor, notepad, loop_noise, render, _stdin_recovery. Port last or stub.
