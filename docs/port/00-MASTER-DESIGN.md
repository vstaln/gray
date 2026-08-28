# Hermes-Agent → Rust: Master Port Design

Source: NousResearch/hermes-agent (~900k LOC Python core + ~195k TS apps + ~851k tests).
Target: **hermes-rs**, a fresh Cargo workspace at `~/hermes-rs` (gray stays independent).
Inputs: 8 subsystem recons in this directory (`root-modules.md`, `agent-core.md`, `tools.md`,
`gateway.md`, `hermes_cli.md`, `hermes_cli-web.md`, `plugins.md`, `tui-acp-cron.md`).
Labels carry over: PROVEN = verified by recon agents; CONJECTURED flagged inline.

---

## 1. Cross-cutting facts that shape everything

1. **Everything self-registers at import time.** Tools (`registry.register()`),
   platform adapters (`platform_registry.register(PlatformEntry(...))`), RPC methods
   (`@method("name")`), CLI subparsers (`add_parser(subparsers)`), plugins (`register(ctx)`).
   Rust has no import side effects → **every subsystem gets an explicit startup-time
   registry**: a builder function per module family that returns populated registry structs.
   No inventory/linkme magic — plain `fn register_all(reg: &mut Registry)` trees.
2. **The agent core is thread-based, not asyncio-first** (PROVEN: only 12 of ~199 agent/
   files contain `async def`; 52 use threading). But HTTP streaming, websockets, web
   server are async. Decision: **tokio everywhere**, with `tokio::task::spawn_blocking`
   standing in for `asyncio.to_thread`. One runtime, no blocking-client split.
3. **Loose dicts are the lingua franca** (messages, tool args, configs). The single
   biggest semantic risk of the whole port. Mitigation: **pin the message/tool schema as
   serde types in wave 0** — a `hermes-types` crate translated from
   `agent/message_content.py`, `agent/message_metadata.py`, `model_tools` coercion rules,
   and `tools/schema_sanitizer.py` behavior. Everything downstream depends on it.
4. **Lazy imports encode two things**: optional heavy deps (→ cargo feature flags) and
   circular-import workarounds (→ real shared modules, e.g. extract `session_context`
   into its own crate).
5. **Global/contextvar ambient state** (13+ files): home-dir overrides, session env,
   tool loops. → explicit context structs threaded through, `tokio::task_local!` only
   where ambient semantics are load-bearing (home override tokens).
6. **SQLite WAL hardening is a discipline, not a file**: state DB, kanban DB, cron store,
   delivery ledger, plugin storage all rely on WAL + flock + pid-liveness patterns against
   multiprocess access. One `hermes-db` crate owns rusqlite setup, pragma policy,
   repair-lock helpers; every other crate uses it. Re-validate WAL workarounds against
   libsqlite3-sys's bundled sqlite version (Python targeted CPython's build).
7. **The Python "plugin system" is manifest + register(ctx)** — no dylibs needed for a
   faithful port since all plugins ship in-tree. Bundled plugins become compile-time
   modules behind feature flags; hot-reload and third-party dynamic installs are DROPPED
   from v1 scope (restart to apply). This is the one deliberate infidelity; revisit only
   if out-of-tree plugins become a requirement.
8. **Registration-order routing** (Starlette) vs axum's most-specific-first: every route
   order dependency found in recon must be re-verified during the web wave.

## 2. Workspace layout

```
hermes-rs/crates/
  hermes-types      ← pinned serde schemas: messages, tool defs/results, usage, events
  hermes-paths      ← hermes_constants (home/profile resolution), path keys
  hermes-util       ← utils, hermes_time, hermes_logging (channel logger)
  hermes-config     ← yaml config + defaults + migrations + env_loader (from hermes_cli/config*)
  hermes-db         ← rusqlite policy layer (WAL/pragmas/repair locks); used by all DBs
  hermes-state      ← hermes_state{,_common,_schema,_search,_portability} → one SessionStore
  hermes-toolsets   ← toolsets, toolset_distributions, arg-coercion (model_tools logic)
  hermes-provider   ← transports/: chat_completions, anthropic, codex, bedrock, gemini,
                       model_metadata/models_dev/usage_pricing
  hermes-agent      ← conversation_loop, tool_executor, context engine, prompt_builder,
                       turn_*, errors/retry/deadline, credentials/secrets/proxy pool,
                       auxiliary_client + consumers
  hermes-tools      ← registry + file/terminal/process/env(de)/mcp/approval/delegation…
  hermes-plugins    ← PluginContext trait surface + bundled plugin modules (feature-gated):
                       providers/, memory/, image_gen/, video_gen/, web/, platforms/…
  hermes-gateway    ← TurnRunner/GatewayRunner (decomposed), platforms/base+registry,
                       relay/, session db, delivery/stream dispatch, lifecycle watchdogs
  hermes-cron       ← scheduler, jobs (flock store), executions
  hermes-tui-server ← tui_gateway: JSON-RPC dispatch (~150 methods), Transport trait,
                       event publisher
  hermes-acp        ← acp_adapter on agent-client-protocol crate
  hermes-web        ← axum web server + dashboard_auth + routers (replaces FastAPI cluster)
  bins/
    hermes          ← cli entrypoint: clap tree replacing main.py/_parser.py + REPL mixin
                       surface (cli.py/HermesCLI composition → traits over CliContext)
    hermes-gatewayd, hermes-tui, hermes-mcp-serve, hermes-batch, hermes-swe, hermes-cron
```

Dependency direction is strictly downward in the list above; no cycles. Where Python had
lazy-import back-edges (agent↔gateway.session_context), the shared piece moves DOWN a level.

## 3. Port DAG — translation waves

Each wave ends with `cargo build --workspace` green + wave-specific conformance checks
ported from the corresponding `tests/` subtree (851k lines of pytest is the oracle;
we port test *cases*, selectively, not line-by-line).

| Wave | Contents | Gate |
|---|---|---|
| **0** | Scaffold workspace; `hermes-types` schema pinning; `hermes-paths/util/db` | types round-trip fixtures from Python transcripts |
| **1** | `hermes-state` + `hermes-config` + `hermes-toolsets` | open/migrate/search a real `state.db` fixture |
| **2** | `hermes-provider` (all transports) + `hermes-agent` core loop | headless one-shot chat against live API reproduces run_agent behavior |
| **3** | `hermes-tools`: registry, file/terminal/process, environments(local,docker), approval, delegation, MCP client | tool-call transcript replay conformance |
| **4** | `hermes-plugins` machinery + model-providers (anthropic first, then REST-only batch) + memory(mem0) + web search | provider switch matrix smoke |
| **5** | `hermes-cron` + `hermes-tui-server` + ratatui TUI client (replaces ui-tui TS) + `hermes` bin REPL | interactive session over JSON-RPC, full slash-command set |
| **6** | `hermes-web` (axum + dashboard_auth + routers + ws tickets) | route inventory diff vs recon §1; auth middleware tests |
| **7** | `hermes-gateway` (run.py decomposition: TurnRunner first, GatewayRunner along mixin seams) + platforms (webhook → signal → whatsapp_cloud → qqbot…) + `plugins/platforms` big three (telegram/discord/slack) | end-to-end bot message → agent → reply |
| **8** | `hermes-acp` + long tail: voice stack, browser/computer_use, media gen, google_meet decision, pets/achievements/peripherals | feature-flagged, opt-in |

Waves 2–4 parallelize well once wave 0–1 land (types frozen). Waves 6–8 are independent
of each other and can run concurrently behind separate builders.

## 4. Monster-file decomposition contracts

Recon identified the god files; each gets a decomposition contract BEFORE translation:

- `gateway/run.py` (31.4k, GatewayRunner = 371 methods): keep data ownership seams —
  `TurnRunner` extracts cleanly (self-contained per-turn); GatewayRunner splits along
  its own mixins (authz / kanban_watchers / slash_commands) plus platform-adapter table,
  session-db handle, delivery ledger, shutdown supervisor as separate structs coordinated
  by an owning struct. Mid-file imports (L1765, L2073–2315) mark the hidden couplings —
  resolve those into named modules first.
- `agent/auxiliary_client.py` (10.8k) → side-channel LLM service trait + task catalog.
- `agent/conversation_loop.py` (8.6k) → turn state machine module + step functions.
- `agent/context_compressor.py` (8.2k) + `conversation_compression.py` (4.5k) →
  compression pipeline traits.
- `tools/mcp_tool.py` (8.2k) → mcp client conn pool + oauth + schema cache modules.
- `hermes_cli/main.py` (14.2k) → clap tree; each `cmd_*` becomes a module.
- `cli.py` (21.5k) → HermesCLI composition: `CliContext` + traits (Setup, Commands,
  Billing); worktree/git orchestration stays subprocess-to-git (faithful + lazy).
- `web_server.py` (19.6k) → axum Router assembly + middleware tower + app state;
  routers 1:1 from web_routers/.
- `tui_gateway/server.py` (16.2k) → dispatch table + session registry + pending-response
  maps (oneshot channels) + orphan reaper.

## 5. Deliberate divergences (documented infidelities)

1. No runtime plugin install/hot-reload (restart applies changes). All bundled plugins
   compile in behind features.
2. TS frontends not translated: ui-tui → ratatui client speaking the same JSON-RPC
   protocol (protocol preserved so the original TS UI could still attach later);
   Electron desktop app deferred entirely (wave 9+, undecided).
3. WAL workaround code re-validated rather than blind-copied (different sqlite build).
4. Self-updater (`update_cmd.py`, 8.5k) → replaced by installer-channel updates
   (curl script / release artifacts), matching gray's existing RELEASING pattern.
   Python's introspective relaunch flag-carrying becomes an explicit struct.
5. Third-party plugin discovery (pip entry points) dropped in v1.
6. google_meet keeps its Node component out-of-process or defers (open decision).

## 6. Open decisions needing owner input (blocking only their wave)

- **Wave 5**: ratatui TUI fidelity target — pi-style minimal vs hermes-TUI visual parity?
- **Wave 7**: platform priority order beyond webhook/signal/whatsapp/telegram/discord/slack
  (22 platform dirs exist; feishu/dingtalk/teams/matrix/irc/email…)?
- **Wave 8**: voice + browser/computer-use: port faithfully, stub, or skip?

## 7. Execution model

Builder subagents translate unit-by-unit (one monster-file slice or one module family per
dispatch), always against: (a) the relevant recon report, (b) the source file, (c) the
wave gate. Each builder must run `cargo check`/`cargo test -p <crate>` before committing.
Reviewer agents audit conformance against the Python source per completed wave.
Rate-limit discipline: max 4 concurrent builders; no duplicate dispatches — check
`docs/port/` + git log before launching.
