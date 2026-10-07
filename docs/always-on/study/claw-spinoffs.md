# Study: The OpenClaw spin-offs — nanobot, nanoclaw, picoclaw, zeroclaw, tinyclaw

References (all read-only, local checkouts):

| Project | Lang | Path | One-line identity |
|---|---|---|---|
| nanobot | Python | `reference/HKUDS/nanobot` | The small readable re-implementation; the lineage root |
| nanoclaw | TS/Node | `reference/nanocoai/nanoclaw` | "Same core, but agents in Docker containers you can understand" |
| picoclaw | Go | `reference/sipeed/picoclaw` | nanobot ported to Go for $10/10 MB-RAM hardware |
| zeroclaw | Rust | `reference/zeroclaw-labs/zeroclaw` | Production-grade single-binary runtime; closest to Gray's stack |
| tinyclaw | TS/Bun | `reference/warengonzaga/tinyclaw` | Independent "AI companion" — tiny core + plugins, mood-roulette nudges |

Context anchors for comparisons: OpenClaw ≈ 500 KLOC Node monolith, application-level
security, config sprawl (nanoclaw README). Hermes ≈ mature Python gateway + SQLite
SessionDB + rich incident-driven hardening (see sibling study `hermes-memory.md`).

All five converge on the same always-on skeleton: **one long-lived process → a periodic
ticker → a `HEARTBEAT.md`-style checklist → an agent turn → a "say nothing" sentinel →
delivery to the last-used channel.** The interesting differences are *how cheaply the
silence decision is made* and *how durable the wake→deliver pipeline is*.

---

# 1. nanobot (HKUDS) — Python

~20K LOC readable core. `nanobot gateway` is the always-on process; also has WebUI, TUI
(native binary), and an OpenAI-compatible API.

## 1. Process model
- `GatewayRuntime` is a **managed subprocess**: `start_background`/`start_on_demand`/
  `foreground_instance` with `subprocess.Popen`, a FileLock transition lock, health
  endpoint probe, and a client "lease" so a CLI can auto-start the gateway on demand and
  mark it ephemeral (`nanobot/gateway/runtime.py:202`, `:233`, `:259`, `:419-465`).
- Inside the gateway everything is one asyncio loop: `MessageBus` with two unbounded
  `asyncio.Queue`s (inbound, outbound). Channels push inbound; core publishes outbound;
  "Local subscribers are awaited by `publish`; channel delivery is queued" — local state
  never waits on network sends (`nanobot/bus/queue.py:28-60`).
- Config hot-restart: `gateway.restart_mode` = `auto|exec|spawn|exit`
  (`nanobot/config/schema.py:355-358`).

## 2. Wake sources
- **Unified cron store** `jobs.json` (`CronStore`): schedule kinds `at` (one-shot ms),
  `every` (interval ms), `cron` (croniter expr + tz); payload kinds `agent_turn` or
  `system_event`; per-job `delete_after_run`, `run_history` ring (`nanobot/cron/types.py:23-199`).
- Timer: single asyncio task armed to `min(next_wake, max_sleep_ms=300_000)`; re-arms in
  `finally` so a bad tick can't kill the scheduler (`nanobot/cron/service.py:183,523-601`).
- **Heartbeat is just a system cron job** `id="heartbeat"` re-registered idempotently at
  startup from `gateway.heartbeat.interval_s` (default 30 min) (`nanobot/cli/gateway_runtime.py:838-855`,
  `config/schema.py:327-331`).
- **Dream** (memory consolidation) is the same shape: system cron job `id="dream"` from
  `agents.defaults.dream.build_schedule(tz)` (`cli/gateway_runtime.py:822-836`).
- **Local triggers** = a directory-based inbound queue: `triggers/{inbox,processing,failed,runs}`
  under the workspace; a 0.5 s poll claims deliveries (batch 20, max 10 attempts, recovery
  of interrupted `processing/` files at boot) and submits each as a session-bound turn
  (`nanobot/triggers/local_store.py:36-50`, `local_runner.py:22-50`, `local_types.py:44-78`).
  External scripts wake the agent by dropping a file — dead simple and durable.
- Self-scheduling: the agent gets a cron tool (`agent/tools/cron.py`); jobs are
  session-bound — unbound legacy jobs are disabled with a warning
  (`cron/service.py:109-110`, `cli/gateway_runtime.py:695-701`).

## 3. Proactivity policy
Two cheap gates *before* the model, one LLM gate *after*:
1. `HEARTBEAT.md` missing → skip. 2. `_heartbeat_has_active_tasks` parses for an
   `## Active tasks` section with non-comment content — none → **no LLM call at all**
   (`cli/gateway_runtime.py:180-202,635-637`).
3. If tasks exist: run a turn in the dedicated `heartbeat` session key with the prompt
   `_HEARTBEAT_PREAMBLE + "You are executing periodic heartbeat tasks..."` — the preamble
   says *"Output ONLY the final user-facing message… If nothing needs reporting, respond
   with just 'All clear.' and nothing else."* (`cli/gateway_runtime.py:171-178,644-646`).
   The message tool's delivery is suppressed for the whole turn so it can't leak
   (`:651-662`).
4. **Post-run evaluator**: a separate tiny LLM call with `evaluate_notification` tool
   (`should_notify: bool`) decides delivery — **fails closed** (`default_notify=False`):
   evaluator error → silence (`nanobot/utils/evaluator.py:60-131`).
   The evaluator system prompt (`templates/agent/evaluator.md`) is worth quoting: notify
   on "actionable information, errors, completed deliverables, scheduled reminder/timer
   completions"; suppress "routine status check", "confirmation that everything is
   normal", and "meta-reasoning about the task itself… The user should never see the
   agent reasoning about whether to speak." Workspace can override the prompt via
   `evaluator` workspace-prompt file with a max-char cap.
- Target channel picked from live sessions: prefers the unified session's
  `last_channel` metadata, else first non-cli session on an enabled channel, else `cli`
  (→ skip delivery entirely) (`cli/gateway_runtime.py:205-231,743-756`;
  `session/keys.py:30-40`).

## 4. Channels & delivery
- ~20 channel adapters (telegram, discord, slack, whatsapp, email, matrix, mattermost,
  msteams, linear, feishu, dingtalk, qq, signal, wecom, weixin, napcat, mochat,
  websocket…) behind `ChannelManager` + a registry (`nanobot/channels/`).
- Outbound goes through the bus's outbound queue; cron/heartbeat call
  `_deliver_to_channel(OutboundMessage(...), record=True)` (`cli/gateway_runtime.py:685-689`).
- No durable outbound retry queue — delivery is in-memory queues + per-channel adapters.
- DM sender approval via pairing codes in `~/.nanobot/pairing.json`
  (`nanobot/pairing/store.py`).

## 5. Sessions & long work
- Session key = `{channel}:{chat_id}`, or one `unified:default` session when
  `unified_session` is on; `last_channel` metadata is persisted so background jobs can
  route back to wherever the user last talked (`session/keys.py:8-40`).
- Internal sessions (`heartbeat`, `dream:*`) are namespaced out of the user surface
  (`session/keys.py:14-21`).
- AutoCompact: session expiry → archive via a `Consolidator` before reset
  (`agent/autocompact.py:27-112`).
- Subagents (`agent/subagent.py`) and a `/goal` sustained-objective mode (§7).

## 6. Memory & user model
- `MemoryStore` = plain files: `memory/MEMORY.md` (long-term), `memory/history.jsonl`
  (append-only, auto-increment cursor), `SOUL.md`, `USER.md`; `MEMORY.md` content is
  injected as `## Long-term Memory` when non-empty (`agent/memory.py:59-255`).
- **Dream** consolidates incrementally: `.dream_cursor` tracks how much of
  `history.jsonl` has been processed; each dream run builds a prompt + scoped tools,
  diffs content, advances the cursor only on completion, and **git-commits the memory
  dir** (`cli/gateway_runtime.py:151-166,567-624`; commit message
  `"dream: periodic memory consolidation"`). Failure leaves the cursor so it retries
  next run — the cursor is the idempotency mechanism.

## 7. Goals / standing orders
- `/goal` sets a sustained objective in session metadata (`goal_state` JSON, status
  `active|blocked|cancelled`); a `goal_runtime.md` host-instructions template steers the
  turn: register via `create_goal` early, write objectives that are "state-oriented,
  self-contained, safe under repetition, bounded, explicit about done-ness", call
  `update_goal action='complete'` only after verified (`session/goal_state.py:14-60`,
  `templates/agent/goal_runtime.md`). The guidance is explicitly written to survive
  compaction/replay — rare and worth copying.

## 8. Reliability
- Cron store: FileLock, atomic writes, `.corrupt-<ts>` backups of unreadable stores,
  in-memory store survives a corrupt reload; `_store_dirty` is persisted *before*
  reloading to avoid replaying a job whose side effects already ran
  (`cron/service.py:319-470,555-601`).
- Trigger inbox recovers interrupted `processing/` files; deliveries have
  `_MAX_DELIVERY_ATTEMPTS=10` then go to `failed/` (`triggers/local_store.py:23-25`,
  `local_runner.py:29-34`).
- Health: unauthenticated `/health`-style endpoint, warns loudly when bound beyond
  loopback (`cli/gateway_runtime.py:234-258`).

## 9. Safety for unattended action
- Pairing-code DM allowlist; `security/` package (sandbox/bwrap docker-compose variant in
  repo root); heartbeat evaluator *fails closed*; unbound cron jobs are refused rather
  than run untargeted.
- Message-tool delivery suppression is a real gate, not a prompt suggestion — the agent
  physically cannot send during the internal heartbeat turn (`cli/gateway_runtime.py:651-662`).

## 10. UX
- One-command installer that then opens `nanobot webui`; WebUI + native TUI +
  OpenAI-compatible API on `api.port=8900`; gateway port `18790`
  (`config/schema.py:333-359`). `jobs.json`, `triggers/inbox`, `HEARTBEAT.md`,
  `MEMORY.md` are all human-editable files — the whole control surface is files.
- "Feels alive": heartbeat "All clear." default, Dream git commits, webui sidebar shows
  cron/subagent state.

## 11. Mini-verdict
Steal: heartbeat-as-cron unification, active-tasks pre-filter, post-run notify evaluator
failing closed, dream cursor+git-commit, file-drop trigger inbox. Skip: the extra
evaluator LLM call is a cost center a rule-based sentinel could cover for most jobs.

---

# 2. nanoclaw (nanocoai) — TypeScript, Docker-isolated agents

One Node host process + **one container per session**. Uses Claude Agent SDK (or
Codex/OpenCode/Ollama providers) as the in-container harness. The anti-OpenClaw pitch:
small codebase, OS-level isolation, credentials never enter the container.

## 1. Process model
- Host = single Node process (orchestrator). Each **session** runs in its own Linux
  container sharing the agent *group's* filesystem (CLAUDE.md, skills, mounts)
  (`docs/architecture.md`).
- **The host↔container contract is two SQLite files and nothing else**: `inbound.db`
  (host writes `messages_in`, container reads RO) + `outbound.db` (container writes
  `messages_out` + `processing_ack`/`session_state`/`container_state`, host reads RO).
  One writer per file; `journal_mode=DELETE` explicitly — *not* WAL, because WAL's
  mmap'd `-shm` doesn't propagate across VirtioFS and would freeze readers on a stale
  snapshot (`docs/architecture.md`, "Two-Level DB").
- Agent-runner inside the container polls `inbound.db` every 1000 ms idle / 500 ms
  active; messages are `markProcessing` → provider `query()` → `markCompleted`;
  mid-turn arrivals are pushed into the live query via `provider.push()` (steering while
  busy). Continuation (Claude transcript / thread id) is persisted **per-provider** so
  switching providers can't resurrect a stale id
  (`container/agent-runner/src/poll-loop.ts:33-34,58-96,438-460`).
- Containers spawn on `wakeUpAgent`, idle-kill when their mailbox is empty. 10
  consecutive driver-classified failures → fresh runner
  (`poll-loop.ts:40`, `docs/architecture.md` "Container Lifecycle").

## 2. Wake sources
- **Scheduled tasks** (DB-backed): `--recurrence "0 9 * * 1-5"` (install-timezone cron)
  or `--process-after <ISO>` one-shots; each task runs in its own **system session**
  (`docs/scheduled-tasks.md`).
- **Script gates** — the best cheap-wake trick in the whole study: a Bash script runs
  *before* the agent is woken (30 s timeout, 1 MB output cap); last stdout line must be
  JSON. `{"wakeAgent": false}` completes the run **with zero model spend**;
  `{"wakeAgent": true, "data": {...}}` injects data into the prompt. Canonical example is
  a marker-file check (`docs/scheduled-tasks.md` "Script gates").
- Channel adapters decide forwarding: stateless (regex trigger) or stateful (mentioned
  earlier in thread → forward all). Two-level IDs — platform channel id + optional thread
  id — make "one session per Slack thread" vs "one session per channel" an adapter
  choice (`docs/architecture.md` "Channel Adapters").
- Webhooks: `src/webhook-server.ts` (202 LOC).
- No HEARTBEAT.md pattern. "Heartbeat" here is a liveness file, not a task loop
  (container touches `/workspace/.heartbeat`, host stats mtime —
  `container/agent-runner/src/heartbeat.ts:3-22`, `src/liveness.ts:17-33`).

## 3. Proactivity policy
- The policy is *architectural*: nothing reaches the model unless a message row exists,
  and rows only appear via trigger-filtered channels, due tasks (possibly script-gated),
  or webhooks. There is no NO_REPLY evaluator — silence = never waking the agent.
- Proactive sends are the agent's own act: `messages_out` rows naming a *destination*
  (`destinations` lookup table mounted in inbound.db); task prompts must say where to
  send ("send it to telegram") (`docs/scheduled-tasks.md`, `destinations.ts`).

## 4. Channels & delivery
- Adapters are install-on-demand skills (`/add-telegram`, `/add-slack`…) living on a
  `channels` git branch — trunk ships none (README "Skills over features").
- Host delivery poll: an *active* poll for hot sessions plus the 60 s sweep for
  everything; `delivered` table provides at-least-once + dedupe;
  `MAX_DELIVERY_ATTEMPTS=3` **persisted across restarts** so poison messages die
  permanently, not per-process (`src/delivery.ts:43,176-233,255-365`).
- Cross-session echo: a delivered reply can be echoed into sibling sessions of the same
  agent for shared awareness (`delivery.ts:303-334`).
- Failure semantics worth stealing: an errored *inbound* batch is acked `completed` —
  no redelivery, no poison-loop (`poll-loop.ts:289-300`).

## 5. Sessions & long work
- Session = `{agent_group, session}` folder + DB pair; modes: shared session across
  channels, per-thread, or per-channel-private (`docs/isolation-model.md`).
- Resume: provider continuation blob per session; container restart resumes the same
  underlying provider conversation.
- Stuck recovery: `CLAIM_STUCK_MS=60_000`; `decideStuckAction` treats missing heartbeat
  as "fresh spawn gets grace" keyed to spawn timestamp, and a durable "fence" (not an
  in-memory flag) prevents double-claim (`src/reconcile-session.ts:50-99,221-251`).
- Operator escape hatch: `rm -rf` a session folder re-provisions instead of killing the
  chat (`session-manager.ts:288-293`).

## 6. Memory & user model
- Plain-Markdown `memory/` per agent group (OKF v0.1: one concept per file, YAML
  frontmatter `type:`). `memory/index.md` (core memory + map) and
  `memory/system/definition.md` (how memory works — agent-editable) are injected at every
  fresh context — capped **16k chars each**; deeper files are followed by link
  (`docs/memory.md`). Migration-safe across providers because it's just files.

## 7. Goals / standing orders
- No formal goal system; standing orders live in the group's `CLAUDE.md` +
  `instructions.prepend.md` and per-task prompts. Task work-logs are appended per run
  (`ncl tasks get` shows them).

## 8. Reliability
- **Host sweep every 60 s** reconciles everything: egress re-heal → every active session
  (concurrency 8) → approvals scan → orphan-container stop; tick ends only when the
  enqueue queue drains (`src/host-sweep.ts:35,48,154-179`).
- Stale `processing` acks cleared at runner boot (`poll-loop.ts:118`);
  session-claim fencing survives process restarts (durable fence record).
- `launchd/` unit for macOS; `nanoclaw.sh` installer can invoke Claude Code to
  self-diagnose failed steps (README).

## 9. Safety for unattended action
- The strongest sandbox story of the five: filesystem-visible-only-what's-mounted
  containers; **credential gateway** (OneCLI Agent Vault) injects secrets at request
  time outside the container, with per-agent policy + rate limits — the agent never
  holds raw API keys (README).
- Allowlists, pairing, `gateway-approval-coordinator.ts` (821 LOC) for gated actions.

## 10. UX
- `ncl` CLI (`tasks`, `groups`, `sessions`); setup is `nanoclaw.sh` scripted + hands off
  to Claude Code for anything needing judgment ("AI-native, hybrid by design").
- Per-agent Slack apps provisioned for it — each agent gets own identity+avatar+memory.
- Feels alive via *separate agents as teammates*, not ambient chatter.

## 11. Mini-verdict
Steal: script gates (cheap pre-wake check), paired one-writer SQLite mailboxes,
delivered-ids + persisted attempt counters, durable claim fences, 60 s reconcile sweep.
Skip: container-per-session is heavyweight for Gray; the *shape* (mailbox files) matters
more than the isolation mechanism.

---

# 3. picoclaw (sipeed) — Go, embedded-class

Explicit Go port of nanobot ("Inspired by and based on nanobot" — file headers), tuned to
boot in ms and idle in ~10 MB on RISC-V/ARM SBCs. One static binary; `picoclaw gateway`
runs everything in-process as goroutines.

## 1. Process model
- Single binary, cobra CLI: `agent|gateway|cron|mcp|onboard|config|status|skills|model|
  auth|update|migrate` (`cmd/picoclaw/main.go:130-142`). No supervisor; `pkg/pid` +
  `pkg/updater` for self-update.
- `pkg/gateway/gateway.go` wires bus + channel manager + agent loop + cron service +
  heartbeat service, all in one process (`gateway.go:850-884`).

## 2. Wake sources
- `HeartbeatService`: `time.Ticker` every `interval` (default 30 min, floor 5 min);
  fires once 1 s after boot, then per tick (`pkg/heartbeat/service.go:24-26,129-155`).
- `CronService`: sleeps until `min(next_wake)` or 1 h cap, plus a `wakeChan` poked on
  any job add/update so new jobs reschedule the sleeper immediately
  (`pkg/cron/service.go:125-172,338-344`). Same schema as nanobot (`kind:
  at|every|cron`, `atMs/everyMs/expr/tz`, `deleteAfterRun`) — the JSON is
  cross-compatible (`cron/service.go:18-46`); exprs via `adhocore/gronx`.
- Agent-facing `cron` tool + `spawn` tool for long subagent work; cron security gating
  exists (`gateway.go:849-863`, README v0.2.3 notes).
- Channels can wake: MQTT channel, webhook channel, MaixCam device channel — hardware
  events are first-class wake sources here (`pkg/channels/`).

## 3. Proactivity policy
- Cheapest gate of all five: `HEARTBEAT.md` absent → **write a default template and
  return**; present but nothing below the `Add your heartbeat tasks below this line:`
  marker except headers/blanks → return, zero tokens (`pkg/heartbeat/service.go:215-316`).
- The tick prompt (verbatim, `service.go:236-247`):
  `# Heartbeat Check\nCurrent time: %s\n\nYou are a proactive AI assistant. This is a
  scheduled heartbeat check.\nReview the following tasks and execute any necessary
  actions using available skills.\nIf there is nothing that requires attention, respond
  ONLY with: HEARTBEAT_OK\n\n%s`
- `HEARTBEAT_OK` → `SilentResult`; everything runs through `AgentLoop.ProcessHeartbeat`
  with `SessionKey:"heartbeat"`, `NoHistory:true`, `SendResponse:false`,
  `SuppressToolFeedback:true` (`pkg/agent/agent_message.go:68-96`).
- Delivery goes to `state.json`'s `last_channel`/`last_chat_id` — recorded on every
  user turn except internal channels (`pkg/state/state.go:14-25`,
  `pkg/agent/agent.go:540-551`). Caveat found in code: `createHeartbeatHandler` wraps
  *every* response in `SilentResult`, so at the service layer nothing is forwarded —
  actual user-visible delivery must happen inside the turn (agent's own send tools);
  worth verifying behavior before copying (`pkg/gateway/gateway.go:867-884`).
- No evaluator, no dedupe, no quiet hours — silence is purely the sentinel string.

## 4. Channels & delivery
- ~25 channel packages incl. embedded-exotic ones: `maixcam`, `mqtt`, `onebot`, `pico`,
  `irc`, `matrix`, `deltachat`, `vk`, `wecom`, `whatsapp_native`, webhook + slack/teams
  webhook variants (`pkg/channels/`). `tool_feedback_animator.go` gives typing/progress
  UX on cheap channels.
- Outbound via `bus.PublishOutbound` with `OutboundContext{platform,userID}`; heartbeat
  reuses the recorded `lastChannel` (`pkg/heartbeat/service.go:354-390`).

## 5. Sessions & long work
- Sessions are append-only `.jsonl` + `.meta.json` per key; **truncation is logical** —
  meta records a `skip` offset; lines are never rewritten, so writes stay append-only
  and crash-safe; 64 fixed lock shards (bounded memory regardless of session count);
  10 MB max line (`pkg/memory/jsonl.go:24-56`).
- `ProcessHeartbeat` explicitly does not load history — each wake is context-free.
- `pkg/evolution/`: a self-improvement pipeline — turn "cases" recorded, pattern
  clustering, LLM-generated *skill drafts*, draft review/apply lifecycle
  (`pkg/evolution/{runtime,case_writer,pattern_clusterer,llm_draft_generator,draft_review,
  apply,skills_recall}.go`). Unique to picoclaw; closest thing to "the agent rewrites
  its own tools" among the five.

## 6. Memory & user model
- Workspace files: `SOUL.md`, `USER.md`, `AGENT.md`, `memory/MEMORY.md`, `skills/`
  (`workspace/`). Session JSONL doubles as conversation memory; MEMORY.md is the
  long-term store. `pkg/credential` + `.security.yml` handle secrets filtering.

## 7. Goals / standing orders
- None formal. HEARTBEAT.md checklist + cron prompts are the whole mechanism.

## 8. Reliability
- Atomic file writes everywhere (`pkg/fileutil.WriteFileAtomic`); state.json atomic
  with migration; cron recomputes `nextRunAtMs` on boot; lock-sharded memory store
  can't grow unboundedly; `heartbeat.log` is a dedicated append log separate from main
  logs (`pkg/heartbeat/service.go:393-415` — nice touch for a noisy subsystem).
- `pkg/health`, `pkg/pid` (single-instance), `pkg/isolation` (bubblewrap-ish),
  config migration pipeline (`pkg/config/migration.go`).

## 9. Safety for unattended action
- `.security.yml` sensitive-data filtering, isolation package, cron security gating
  (v0.2.3), pairing/allowlist per channel, `pkg/commands` guard. Self-described as
  pre-v1.0 "don't deploy to production" — treat its posture as reference-grade, not
  proven.

## 10. UX
- Web UI launcher (web/frontend + web/backend), system-tray UI on Windows/Linux,
  Android APK, MaixCam hardware integration, `picoclaw status`, self-update. Feels alive
  via tool-feedback animations and hardware presence more than ambient messaging.

## 11. Mini-verdict
Steal: marker-line heartbeat gate (zero-token skip), wakeChan-driven cron sleeper,
logical-truncation JSONL sessions, evolution skill-draft loop (aspirational), tiny
resource budget as a design constraint. Skip: silent-wrap ambiguity in the heartbeat
handler; no persistence of delivery attempts.

---

# 4. zeroclaw (zeroclaw-labs) — Rust, production-grade

The largest and most rigorous implementation; a Cargo workspace (23 `zeroclaw-*` crates)
behind one binary. `zeroclaw service install` → systemd --user / OpenRC / launchd /
Windows Service; `zeroclaw quickstart` writes a working TOML config. Deep always-on
machinery — this is the closest model to what Gray's gateway should grow into.

## 1. Process model
- **Daemon = a component supervisor.** `run` spawns a `JoinHandle` per subsystem —
  state writer, channels, gateway (HTTP/WS + dashboard), scheduler, heartbeat worker,
  tunnels — each wrapped in `spawn_component_supervisor` with exponential backoff
  (`initial_backoff`→`max_backoff`) that **resets to base after a "stable run" of
  5×initial** so a component that ran for hours then crashed retries fast instead of
  inheriting a huge stale backoff (`crates/zeroclaw-runtime/src/daemon/mod.rs:728-947,
  1752-1832`).
- Every component reports into a shared `crate::health` registry
  (`mark_component_ok`/`mark_component_error`) — the scheduler marks ok even on idle
  polls, so "alive but nothing due" is distinguishable from "wedged"
  (`cron/scheduler.rs:458,548-551`; `health/mod.rs`).
- Turns run in-process via `crate::agent::run` with a `TurnOrigin` (Daemon /
  channel ingress) and `InternalPrincipal` stamps (`daemon/mod.rs:2485-2510`).
- OS service integration is native: `service/mod.rs` handles `systemctl --user`,
  OpenRC, launchd (with stdout/stderr capture piping into the service log) — the
  supervised *runner* is zeroclaw itself re-exec'd, not a wrapper shell script
  (`service/mod.rs:1071-1313,1697-1738`).

## 2. Wake sources
- **Heartbeat worker** (daemon component): `interval_minutes` default 30;
  `[heartbeat] agent = "<alias>"` required — heartbeats run *as a configured agent*,
  not a special mode (`config/schema.rs:15278-15300`, `daemon/mod.rs:1867-1876`).
- **Two-phase heartbeat (default on)**: Phase 1 sends a *decision prompt* — "You are a
  heartbeat scheduler… Respond with ONLY one of: `run: 1,2,3` / `skip`… Be conservative"
  — and only Phase-2-executes the selected subset; Phase-1 failure falls back to running
  all tasks (`heartbeat/engine.rs:290-349`, `daemon/mod.rs:2455-2562`). Cheap-triage
  before expensive-work is the same shape as nanoclaw's script gates but expressed as a
  model call.
- **Structured HEARTBEAT.md**: `- [high|paused] task text` lines →
  `HeartbeatTask{priority,status}`; runnable = active only, sorted high→low;
  `heartbeat.message` config fallback when the file has no tasks
  (`heartbeat/engine.rs:14-57,225-290,381-400`).
- **Adaptive interval**: exponential backoff on consecutive failures
  (`base * 2^failures`, clamped `[min,max]` = [5,120] min defaults), and drops to
  `max(5,min)` minutes when a high-priority task exists
  (`heartbeat/engine.rs:130-156`; config `adaptive`, `min/max_interval_minutes`).
- **Dead-man's switch**: separate 60 s watcher — if `last_tick_at` ages past
  `deadman_timeout_minutes`, it *sends an alert* over the heartbeat (or dedicated)
  channel; the heartbeat monitoring itself is observable, not assumed
  (`daemon/mod.rs:2361-2417`).
- **Cron scheduler**: SQLite store; poll `reliability.scheduler_poll_secs` (floor 5 s);
  `tokio` interval with `MissedTickBehavior::Skip`; schedule kinds
  `{cron expr+tz, at, every_ms}`; job types `shell|agent`; `session_target:
  isolated|main`; declarative `[cron.<alias>]` config synced into DB at boot
  (`source="declarative"`, config wins on drift) plus a synthesized `__builtin_backup`
  job from `backup.schedule_cron` (`cron/scheduler.rs:26,442-575`, `types.rs:29-120`,
  `config/schema.rs:15400+`).
- **Catch-up policy is explicit**: `scheduler.catch_up_on_startup` — on: run all
  overdue jobs once; off: `skip_missed_run` advances `next_run` *without executing* and
  records the skip (`scheduler.rs:531-539,623-748`). Most projects get this wrong or
  silently; zeroclaw makes it a config decision.
- **Routines engine**: event-pattern → action automation with per-routine cooldowns —
  actions are `Sop{name}`, `Shell{command}`, `Message{channel,text}`, `CronJob{name}`
  (`routines/engine.rs:14-120`). SOPs are declarative multi-step procedures fired by
  MQTT/webhook/cron/peripheral triggers with approval gates and resumable runs
  (`src/sop/mod.rs`, README).
- Other wake edges: webhook channel, gmail_push, MQTT, calendar poller, filesystem
  channel, hooks system, voice-wake, peripherals (GPIO/I2C/SPI) — the broadest
  trigger surface of the five (`crates/zeroclaw-channels/src/`,
  `zeroclaw-runtime/src/calendar/poller.rs`, `hooks/`).

## 3. Proactivity policy
- **NO_REPLY sentinel with a kinded grammar** — the most nuanced silence contract seen:
  `NO_REPLY`, `NO_REPLY: …`, and `NO_REPLY[INFO]: …` are suppressed; `NO_REPLY[REFUSE]`
  and `NO_REPLY[FAIL]` **are delivered** because they carry operator meaning; a malformed
  `NO_REPLY[` with no bracket is delivered rather than guessed
  (`cron/scheduler.rs:37-86`). Same decision function gates heartbeat announcements
  (`daemon/mod.rs:2680-2700`).
- Phase-1 skip is a second silence layer (no tasks run, no delivery).
- Context on wake: optional `load_session_context` reloads the delivery channel's
  session history each tick so the agent sees fresh conversation; memory injection is
  origin-aware — `Conversation` entries are excluded for scheduled (Daemon) origins
  (`config/schema.rs:15331-15341`, `daemon/mod.rs:2570-2579,2588-2597`).
- Every agent run inside heartbeat gets `TurnOrigin::Daemon` +
  `InternalPrincipal::Daemon{task:"heartbeat:decision"|"heartbeat:execute"}` —
  provenance is machine-readable all the way down, which is what makes per-origin
  policy (memory injection, tool allowlists, receipts) possible (`daemon/mod.rs:2485-2510`).

## 4. Channels & delivery
- ~40 feature-gated channel crates — everything from Telegram/Discord/Matrix/Slack to
  nostr, bluesky, AMQP, MQTT, voice-call, voice-wake, filesystem, webhook, gmail_push,
  git, acp (`crates/zeroclaw-channels/src/lib.rs:1-98`).
- **Orchestrator** (55 KLOC): inbound → per-conversation **debounce buckets** (rapid
  messages merge into one turn; room-scoped keys; buckets are cancel-safe and carry
  their permits) → inbound hooks → per-session lanes → agent run → delivery
  (`orchestrator/mod.rs:700-840,1429-1610,1758-1764`).
- **`SessionActorQueue`**: semaphore per session id — `QueueFull{depth}` and
  `Timeout` errors instead of unbounded wait; idle slots evicted; per-session transcript
  generations let holders detect history changes
  (`crates/zeroclaw-infra/src/session_queue.rs:10-90`).
- **Durable plugin outbox**: SQLite outbox for plugin host work — leased rows
  (5 min), `MAX_ATTEMPTS=8`, backoff 1 s→15 min, 24 h give-up, **persisted idempotency
  keys**, and a `PublicOutboxData` type that structurally separates public payloads from
  `SecretPropertyRef`s so plaintext secrets can't enter the envelope
  (`cron/outbox.rs:1-90`). At-least-once by design, dedupe by key.
- `deliver_announcement` reuses the live channel registry — cron output and heartbeat
  alerts ride the same delivery path as interactive replies (`scheduler.rs`,
  `orchestrator/mod.rs:146`).

## 5. Sessions & long work
- `SqliteSessionBackend` (`zeroclaw-infra`), session lanes in the orchestrator,
  transcript generations for staleness. Cron jobs choose `isolated` (fresh session per
  run) vs `main` session. Sub-agents via `spawn_subagent` tool; `StallWatchdog` for
  wedged turns; debounce merges bursts.
- Cron-spawned agent runs default-exclude the cron-management tools
  (`CRON_AGENT_DEFAULT_EXCLUDED_TOOLS` = cron_add/update/remove/run/schedule) — a cron
  turn can't silently rewire the schedule (`scheduler.rs:27-32`).

## 6. Memory & user model
- An entire crate: markdown / sqlite / postgres / qdrant / lucid backends, embeddings,
  consolidation, decay, dedup, knowledge graph, rerank, hygiene passes, response cache,
  agent-scoped + principal-plane scoping, audit + threat scanning wrappers
  (`crates/zeroclaw-memory/src/lib.rs:1-60`).
- Origin-aware injection: `[Memory context]...[/Memory context]` markers; heartbeat
  outputs ≥50 chars auto-store as `MemoryCategory::Daily` for cross-session awareness
  (`daemon/mod.rs:2644-2675`, `memory/lib.rs:7-10`).

## 7. Goals / standing orders
- SOPs + routines are the standing-orders mechanism; `todo_write` tool exists;
  structured `[priority|status]` heartbeat tasks double as a standing checklist.
  No first-class "goal" object like nanobot's `/goal`.

## 8. Reliability
- The best of the five by a wide margin: durable job claiming (`claim_job` in-flight
  locks), `clear_stale_locks` at boot, explicit catch-up-vs-skip policy, per-component
  health registry, supervisor backoff with stable-run reset, heartbeat metrics with
  EMA durations + deadman alerting, run-history rings (100 records), persisted
  outbox idempotency.
- Cron runs carry `InternalPrincipal` and resolve the owning agent via
  `[agents.<x>].cron_jobs` — a job can't fire under the wrong agent after config
  changes (`scheduler.rs:578-618`).

## 9. Safety for unattended action
- `AutonomyLevel::{ReadOnly,Supervised,Full}` in named `[risk_profiles.<alias>]`
  bound per agent; `approval_route` options; `--dangerously-bypass-approvals-and-sandbox`
  (aka `--yolo`) disables two independent gates at once
  (`config/autonomy.rs:19-26`, `schema.rs:14493-14601,10626-10637`).
- OS sandboxes (Landlock/Bubblewrap/Seatbelt/Docker), workspace boundary policy,
  cryptographic **tool receipts** on actions, verifiable-intent issuance
  (`runtime/src/verifiable_intent/`), net_guard, pairing + token rotation for gateway
  clients.

## 10. UX
- `zeroclaw quickstart` (one-shot config), `agent -a <alias>` interactive,
  `service install|start`, web dashboard (chat, memory browser, config, cron, tools),
  gateway pairing flow, tunnels (cloudflare/ngrok/tailscale/pinggy/custom/openvpn) for
  remote access, i18n, `doctor`, evals, fuzz + benches in-repo. Feels engineered, not
  cute.

## 11. Mini-verdict
Steal nearly everything structural: component-supervisor daemon with stable-run
backoff reset, claimed-job SQLite cron + explicit startup catch-up policy, kinded
NO_REPLY, two-phase heartbeat, deadman switch, session actor queue, durable outbox,
TurnOrigin provenance. Do **not** copy the blast radius: 55 KLOC orchestrator and a
33-module memory crate are over-built for Gray today — take the patterns, not the scale.

---

# 5. tinyclaw (warengonzaga) — Bun, "companion" not butler

Independent product (not a fork): tiny core + plugin everything, personality-first
(Heartware SOUL.md), Ollama-Cloud built-in, Discord-like web UI. Explicitly optimizes
for *feeling alive* rather than throughput.

## 1. Process model
- `bun start` → `supervisor.ts` wraps the agent in a respawn loop: child exits
  **code 75** (`RESTART_EXIT_CODE`) → respawn (used for self-restart after config
  change); any other code passes through. **Crash-loop guard**: >N rapid restarts in a
  window → supervisor exits 1 (`src/cli/src/supervisor.ts:23-47,60-108`).
- Everything else is in-process: core agent loop (`core/src/loop.ts`, 1236 LOC),
  `SessionQueue` (per-session promise chain — 59 LOC total, `packages/queue/src/index.ts`),
  Pulse scheduler, nudge engine, gateway, delegation runner.
- Web UI served by `src/web/src/server.ts` (SSE streaming, nudge prefs API).

## 2. Wake sources
- **Pulse**: trivially simple — `setInterval` per job, schedule strings like `'30m'`,
  `'1h'`, `'24h'` only (no cron exprs, no persistence, no catch-up); `isRunning` flag
  skips overlap; `runOnStart` for boot jobs (`packages/pulse/src/index.ts:17-125`).
- The interesting wake content is the **companion nudge jobs** (§3), plus nudge `flush`
  driven by a pulse job (~1 min cadence, caller-configured).
- Intercom pub/sub wakes subsystems on lifecycle topics (`task:completed`,
  `agent:created`, `blackboard:proposal`, `nudge:scheduled`, …) with bounded history
  (`packages/intercom/src/index.ts:1-60`).

## 3. Proactivity policy — the reason to study this repo
- **Nudge engine**: a passive queue ordered by `deliverAfter`; `schedule()` inserts,
  `flush()` delivers due items through the gateway. Preferences: master `enabled`,
  **quiet hours** `quietHoursStart/End` ("HH:MM", handles overnight ranges),
  `maxPerHour` (default 5), `suppressedCategories` per-category opt-out
  (`packages/types/src/index.ts:894-950`, `nudge/src/index.ts:30-140`).
- **Categories** carry semantics + per-category mute: `task_complete`, `task_failed`,
  `reminder`, `check_in`, `insight`, `system`, `software_update`, `agent_initiated`,
  `companion` (`types/src/index.ts:896-905`).
- **Companion mood roulette** — the most original mechanism in the whole study. A Pulse
  job `companion-quick-checkin` (10 min) fires only if: companion enabled AND owner
  claimed AND owner idle ≥ `checkinInterval` (30 min default) AND ≥ that long since
  last companion nudge AND ≥1 prior conversation exists. Then `rollMood()` picks from a
  weighted pool — check_in 25, motivational 20, playful 15, random_thought 15,
  encouragement 10, reflection 10, philosophical 5 — and a mood-specific prompt goes
  through the agent loop *with Heartware personality already injected*, producing a
  1–3-sentence message scheduled as a `companion` nudge
  (`packages/nudge/src/companion.ts:56-160,280-320`).
- **Boot greeting**: `runOnStart` pulse job sends a warm "I'm back" message once per
  process lifetime — only to *returning* owners (history exists) (`companion.ts:322-375`).
- Anti-slop guard: generated text <5 chars or empty → skip quietly; response scrubbed of
  `[COMPANION…]` tags and quotes before scheduling (`companion.ts:225-250`).
- User activity is fed back via `touchActivity()` on every inbound message — the
  "don't interrupt an active user" signal is one timestamp (`companion.ts:200-210`).

## 4. Channels & delivery
- `OutboundGateway`: `userId` is `"<channel>:<id>"`; senders register by prefix —
  `gateway.send("discord:123", msg)` resolves the prefix. `broadcast` for
  all-channel sends; unregistered channels fail gracefully
  (`packages/gateway/src/index.ts:1-90`).
- Nudges map categories → `OutboundSource` (`check_in`→`pulse`, `reminder`→`reminder`,
  task results→`background_task`, etc.) (`nudge/src/index.ts:65-90`).
- Channels are plugins (`plugins/channel/` — Discord, "friends" push) — none in core.

## 5. Sessions & long work
- SQLite (`bun:sqlite`) for history/memory tables; per-session promise-chain queue
  serializes a user's turns while different users run parallel
  (`queue/src/index.ts:15-59`).
- **Background delegation**: `createBackgroundRunner` runs sub-agents async with
  AbortControllers, max 3 concurrent per user, ~120 s fallback timeout; results are
  delivered on the *next* turn via notification injection — the primary agent never
  blocks (`delegation/src/background.ts:14-60`). Blackboard + role templates +
  adaptive timeout estimator + lifecycle (suspend/revive) + intercom events.
- 4-layer **context compactor**: rule pre-compression → shingle dedup → LLM summary
  (L2 ~3000 tok) → derived L1 (~1000) + L0 (~200) tiers — only ONE LLM call; L1/L0 are
  deterministic trims with keyword-priority scoring (`compactor/src/tiers.ts:1-60`).

## 6. Memory & user model
- 3-layer adaptive memory, all local: episodic events (typed, importance-weighted —
  `correction` 0.9, `preference_learned` 0.8, `fact_stored` 0.6), FTS5/BM25 semantic
  index, Ebbinghaus temporal decay `e^(-0.05·days)·(1+0.02·access_count)`. Retrieval
  score = `0.4·fts + 0.3·temporal + 0.3·importance` — zero API dependencies
  (`packages/memory/src/index.ts:1-70`).
- Heartware = SOUL.md personality the user *cannot* override; learning package detects
  behavioral patterns to improve templates.

## 7. Goals / standing orders
- None formal; delegation tasks + nudges cover it. Templates give sub-agents standing
  roles (`delegation/templates.ts`, `handbook.ts`).

## 8. Reliability
- Supervisor restart code + crash-loop cap is the whole story. Pulse is in-memory —
  restart loses jobs (they're recreated at boot from code, so effectively fine).
  Nudge queue is in-memory too — pending nudges die on restart (acceptable at this
  scale; worth noting as the line where tinyclaw trades durability for simplicity).

## 9. Safety for unattended action
- SHIELD.md runtime anti-malware engine (parse + pattern-match + re-enforce),
  path sandbox, content validation, audit log, auto-backup, rate limiting — "5-layer";
  secrets engine AES-256-GCM; owner-claim auth (`core/owner-auth.ts`) gates who the
  companion nudges.

## 10. UX
- Zero-config onboarding ("Tiny Claw will walk you through the rest"), Discord-like
  dark web UI with typing indicators + delegation event cards + agents sidebar,
  Ollama Cloud free tier built in, self-configuring through conversation. **This is the
  only project of the five designed to proactively say hello** — quiet hours, moods,
  boot greeting.

## 11. Mini-verdict
Steal: nudge engine (categories + quiet hours + rate cap + per-category mute),
companion mood roulette, boot greeting, touchActivity idle signal, promise-chain
session queue (59 LOC), exit-75 supervisor restart + crash-loop cap, tiered one-LLM-call
compactor, episodic+decay memory scoring. Skip: in-memory pulse/nudge durability,
SHIELD.md (theater-leaning for our threat model).

---

# Cross-comparison

| Concern | nanobot | nanoclaw | picoclaw | zeroclaw | tinyclaw |
|---|---|---|---|---|---|
| Long-lived form | `nanobot gateway` managed subprocess w/ lease | host Node proc + container per session | single Go binary, all goroutines | Rust daemon, per-component supervisor | Bun proc under exit-75 supervisor |
| Turn execution | in-process asyncio | in-container (Claude SDK et al.) | in-process AgentLoop | in-process `agent::run` w/ TurnOrigin | in-process loop + bg sub-agents |
| Scheduler | jobs.json; at/every/cron; 5-min-max sleeper | DB tasks; cron expr or `--process-after` | jobs.json (nanobot-compatible); wakeChan | SQLite store; poll ≥5 s; claim locks | in-memory `setInterval`, `'30m'` strings |
| Missed-run policy | runs when next tick fires (no explicit catch-up) | next computed from cron | same as nanobot | **explicit**: `catch_up_on_startup` else `skip_missed_run` | none — jobs just re-register at boot |
| Cheap wake gate | `## Active tasks` section parse | **script gates** `{wakeAgent:false}` | marker-line content check | two-phase LLM `run:1,2,3`/`skip` decision | n/a (nudges are conditional by code) |
| Silence contract | "All clear." + **post-run evaluator LLM** (fail-closed) | silence = never wake | `HEARTBEAT_OK` sentinel | `NO_REPLY` grammar; `[REFUSE]`/`[FAIL]` still deliver | nudge prefs: quiet hours, max 5/hr, category mute |
| Ambient proactivity | heartbeat checklist | none | heartbeat checklist | heartbeat tasks + routines/SOPs | **companion mood roulette + boot greeting** |
| Delivery durability | in-memory bus queues | outbound.db poll; delivered-ids; 3 attempts persisted | bus publish, last_channel state | **SQLite outbox**: lease, 8 attempts, idempotency keys | nudge queue (in-memory) + gateway prefix routing |
| Session model | `channel:chat_id` or unified; internal keys namespaced | per-session folder+DB pair; shared/per-thread modes | JSONL+meta per key, logical truncation | SQLite backend; actor queue + debounce lanes | per-user queue; system `companion:`/`heartbeat:` keys |
| Memory | MEMORY.md + history.jsonl + **Dream cursor** + git commit | plain-md `memory/` OKF, 2 files injected ≤16k | MEMORY.md + JSONL; **evolution** skill drafts | full crate: backends, embeddings, decay, KG, hygiene | episodic FTS5 + Ebbinghaus scoring |
| Standing orders | `/goal` sustained objective + runtime template | CLAUDE.md + task prompts | HEARTBEAT.md | SOPs + routines + structured tasks | role templates |
| Crash/stuck recovery | dirty-store persist-before-reload; trigger inbox recovery | 60 s host sweep; claim fences; stale-ack clear | atomic writes; shard-locked store | stale-lock clear; stable-run backoff reset; stall watchdog; deadman | rapid-restart cap; in-memory loss acceptable |
| Isolation | pairing codes; bwrap variant | **Docker container per session + credential gateway** | `pkg/isolation`, `.security.yml` | Landlock/bwrap/seatbelt; risk profiles; tool receipts | Bun Worker sandbox; SHIELD.md; AES-GCM secrets |
| Quiet hours | no | no | no | no | **yes** (nudge prefs, overnight-aware) |
| Feels-alive factor | "All clear." heartbeats, dream commits | teammates-as-agents | hardware presence, tool animations | engineering signals (metrics, health) | highest — moods, greetings, idle-aware check-ins |

---

# What each does BETTER or SIMPLER than OpenClaw/Hermes

**nanobot** — *simpler*: one `jobs.json` for every timed thing (user crons, heartbeat,
dream) with system vs session-bound jobs distinguished by name; heartbeat skips the LLM
entirely when `## Active tasks` is empty; file-drop `triggers/inbox/` as the whole event
API. *better*: the post-run `evaluate_notification` gate that **fails closed** — OpenClaw
sends whatever the heartbeat prints; nanobot makes "should the user see this" an explicit
reviewed decision.

**nanoclaw** — *better*: OS-level isolation + credential gateway instead of
application-level allowlists (its founding critique of OpenClaw); **script gates** give
zero-token cron wakes — neither OpenClaw nor Hermes has a pre-wake deterministic gate
this clean. *simpler*: the whole host↔agent contract is two SQLite files, one writer
each; channel adapters are opt-in skills, not shipped surface.

**picoclaw** — *simpler*: the cheapest heartbeat gate (read a file, look below a marker
line, maybe call the model); one static binary vs Node runtime + 70 deps; append-only
JSONL sessions where truncation is a metadata offset — compaction can't corrupt history.
*better*: resource budget as a feature (10 MB RAM → runs where OpenClaw can't), and the
`evolution` skill-draft pipeline — a concrete self-improvement loop none of the others
ship.

**zeroclaw** — *better*: operational rigor OpenClaw lacks entirely — claimed-job
scheduler with stale-lock recovery and a *configurable* missed-run policy, kinded
NO_REPLY grammar, per-component health, deadman alerting on the alerter itself, durable
idempotent outbox, provenance (`TurnOrigin`/`InternalPrincipal`) on every turn.
*simpler*: honestly, nothing — it's the anti-simplicity proof that "do it right" costs
~10× the code; the win is that its mechanisms are each small and separable.

**tinyclaw** — *better*: it's the only one that treats proactive messaging as a
*product feature* with user controls — categories, quiet hours, hourly caps, per-category
mute, idle-aware check-ins, mood-weighted variety, boot greeting. OpenClaw's heartbeat
is a task runner; tinyclaw's is a personality. *simpler*: Pulse (50 LOC), SessionQueue
(59 LOC), gateway prefix routing, one-LLM-call 3-tier compaction — proof the skeleton
fits in hundreds of lines.

---

# Synthesis for Gray (ranked, across all five)

1. **Cheap pre-wake gate** (nanoclaw script gates + nanobot/picoclaw file parse): a
   deterministic check decides "is there work" before any model call; cron jobs should
   support an optional script/predicate whose `wakeAgent:false` means zero tokens.
2. **Kinded silence contract** (zeroclaw): `NO_REPLY` / `NO_REPLY[INFO]` suppressed,
   `[REFUSE]`/`[FAIL]` delivered — one `announce_delivery_decision()` shared by cron,
   heartbeat, and routines.
3. **Two-phase heartbeat** (zeroclaw): tiny decision prompt returns `run: 1,2,3` or
   `skip`, then real execution — triage cost ~1% of execution cost. Gray already has a
   cron ticker; this is a prompt + parse layer.
4. **Structured HEARTBEAT.md** (zeroclaw `- [high|paused] task` + adaptive interval):
   priority-aware checklist, faster ticks when high-priority work exists, exponential
   backoff on failures.
5. **Nudge engine with user-visible controls** (tinyclaw): categories, quiet hours,
   `maxPerHour`, per-category mute, `deliverAfter` queue — the "don't spam" policy as
   data, not prompt text.
6. **Companion/liveness layer** (tinyclaw): mood-roulette check-ins gated on owner idle
   + cooldown + history, boot greeting — this is what makes an always-on agent feel
   alive rather than merely awake.
7. **Durable wake→deliver pipeline** (zeroclaw outbox + nanoclaw outbound.db): claimed
   jobs, persisted attempt counters, idempotency keys, delivered-ids — retries survive
   restarts; poison messages die after N persisted attempts.
8. **Catch-up as a policy** (zeroclaw): `catch_up_on_startup` vs `skip_missed_run` —
   decide explicitly, record the decision; never silently replay or silently drop.
9. **Component-supervisor daemon** (zeroclaw): each subsystem under a supervisor with
   backoff that resets after a stable run + per-component health marks — matches Gray's
   "gateway + plugin processes" shape directly.
10. **Memory consolidation cursor** (nanobot Dream): append-only history.jsonl +
    `.dream_cursor` + git-committed memory dir — incremental, resumable, auditable
    "dreaming" with zero new infra.
11. **Provenance on every wake** (zeroclaw `TurnOrigin::Daemon`/`InternalPrincipal`):
    stamp turns with *why* they exist so memory injection, tool allowlists, and receipts
    can differ by origin — e.g. cron turns can't edit cron (zeroclaw's excluded-tools
    default).
12. **File-drop trigger inbox** (nanobot `triggers/inbox/`): the lowest-possible-ceremony
    event source — a file write becomes a session turn with claim/retry/recovery for
    free.

**Explicitly not to copy**: container-per-session (nanoclaw) — over-isolated for our
plugin model; post-run evaluator LLM as the *only* gate (nanobot) — use it behind the
sentinel, not instead of it; tinyclaw's in-memory-only scheduling — Gray needs durable
jobs; zeroclaw-scale memory abstraction — pick `history.jsonl + MEMORY.md + cursor`
first; SHIELD.md-style anti-malware prompt enforcement.

