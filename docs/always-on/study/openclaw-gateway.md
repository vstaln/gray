# Study: OpenClaw — Gateway daemon, heartbeat, cron, hooks, delivery

Reference: `/home/vstaln/gray/reference/openclaw` (TypeScript, Node, SQLite).
Read alongside: `VISION.md`, `docs/gateway/*`, `docs/automation/*`,
`docs/concepts/{main-session,queue,standing-intents}.md`, `src/{daemon,cron,infra,hooks,auto-reply}`.

High-level framing from `VISION.md`: OpenClaw is "the AI that actually does
things — runs on your devices, in your channels, with your rules." One
long-lived **Gateway** process per state dir is the product; channels and the
agent runtime live inside it. Design pressure runs toward "secure defaults,
explicit power," thin core + fat plugin surface, and an obsessive amount of
restart/idempotency hardening. Most of what follows is one theme: *every piece
of in-flight work has a durable owner and a resume story.*

---

## 1. Process model

**One always-on Gateway process** owns routing, the control plane, the
scheduler, and all channel connections on a single multiplexed port (default
`18789`, loopback bind) serving WebSocket control/RPC, OpenAI-compatible HTTP
(`/v1/chat/completions`, `/v1/responses`, `/v1/embeddings`, `/v1/models`),
plugin HTTP routes, hooks ingress, and the Control UI (`docs/gateway/index.md`
"Runtime model", ~line 67). Auth required by default
(`gateway.auth.token/password`).

**Channels are plugins inside the gateway process**, not separate daemons.
They are auto-started side services and can be suppressed independently of the
control plane (see crash-loop breaker, §8).

**Agent turns run in-process** via `runEmbeddedAgent`, serialized by session
key through a lane-aware FIFO queue (`docs/concepts/queue.md`):

- Per-session lane guarantees one active run per session key.
- A global `main` lane caps overall parallelism at
  `min(16, max(8, CPU parallelism))`; `subagent` lane defaults to 8; other
  lanes default to 1 (`docs/concepts/queue.md` ~lines 25-31).
- Queue mode while a session is busy — `steer` (default: inject into the
  active run at the next tool/model boundary), `followup`, `collect`
  (coalesce into one turn after a quiet window), `interrupt` (abort + run
  newest). Built-in 500 ms debounce, `cap: 20`, `drop: "summarize"` which
  injects compact summaries of dropped messages as a synthetic followup
  (`docs/concepts/queue.md` ~lines 36-95).
- Typing indicators fire on enqueue so UX doesn't degrade while queued.

**Supervision is delegated to the OS service manager**, installed by
`openclaw gateway install`:

- **macOS**: per-user LaunchAgent `ai.openclaw.gateway` (or
  `ai.openclaw.<profile>`). Plist sets `RunAtLoad`, `KeepAlive=true`,
  `ThrottleInterval=10`, `ExitTimeOut=20`, `ProcessType=Interactive`,
  `Umask=077`, stdin `/dev/null` (`src/daemon/launchd-plist.ts:342`,
  constants at `src/daemon/launchd-plist.ts:8-13`). `gateway stop` uses
  `launchctl bootout` (KeepAlive still recovers crashes); `--disable`
  persists a stop (`docs/gateway/index.md` ~line 196). A conflicting
  *system* LaunchDaemon with the same label makes install fail closed —
  `--force` does not bypass (~line 208).
- **Linux**: systemd user unit `openclaw-gateway[-<profile>].service`:
  `Restart=always`, `RestartSec=5`, `StartLimitBurst=5`,
  `StartLimitIntervalSec=60`, `RestartPreventExitStatus=78`,
  `TimeoutStopSec=30`, `SuccessExitStatus="0 143"`, `OOMPolicy=continue`,
  `KillMode=control-group`, `After/Wants=network-online.target`
  (`src/daemon/systemd-unit.ts:77-90`). Docs advise
  `loginctl enable-linger` for headless persistence.
- **Windows**: Scheduled Task `OpenClaw Gateway`, with a Startup-folder
  launcher fallback when task creation is denied.
- **Config-invalid exit code 78** is the universal "don't respawn me" signal;
  on systemd `RestartPreventExitStatus=78` honors it. Newer-DB-schema also
  exits 78, and on macOS the process *parks its own LaunchAgent* to stop
  KeepAlive retries (`docs/reference/database-schemas.md` ~line 33).

**Self-restart uses a detached helper.** On macOS a spawned handoff process
performs `kickstart`/`bootout+bootstrap` after the gateway exits, polling for
`ExitTimeOut + 15s` so it can't advance mid-stop and strand the agent
(`src/daemon/launchd-restart-handoff.ts:1-30`). Agent-requested restarts write
a typed `gateway_restart_sentinel` SQLite row before exiting; on boot the
gateway posts the outcome back to the originating chat and fires a one-shot
continuation turn so the agent resumes where it left off
(`docs/gateway/restart-recovery.md` "Agent-requested restarts";
columns at `src/state/openclaw-state-db-schema-additive.ts:485-491`).

**Single-instance enforcement** is three-layered (`docs/gateway/gateway-lock.md`):
state-dir lock → per-config lock (records runtime port) → exclusive socket
bind. Locks live under `$OPENCLAW_STATE_DIR/tmp/openclaw-<uid>`, are reclaimed
when the recorded PID is dead via a SQLite coordinator, retry 5 s then fail
`GatewayLockError`; `EADDRINUSE` retries 20x500 ms. Under a supervisor, a new
process that hits a lock instead probes `/healthz` on the incumbent and exits
78 if it's healthy — "leave the healthy one in control" rather than flap.

Config hot reload is `gateway.reload.mode: "hybrid"` (default): hot-apply when
safe, restart when required (`docs/gateway/index.md` ~line 111).

## 2. Wake sources

### Heartbeat (system-owned automation)

Heartbeat is **not** a separate timer — it's a system-owned cron job per
heartbeat-enabled agent, projected from config at startup and on reload
(`docs/gateway/heartbeat.md`; `src/cron/heartbeat-monitor.ts`):

- Declaration key `heartbeat:<agentId>`, `payload: {kind:"heartbeat"}`,
  `sessionTarget: "main"`, `wakeMode: "next-heartbeat"`, schedule
  `{kind:"every", everyMs, anchorMs}` where `anchorMs` is a deterministic
  phase derived from a scheduler seed + agentId so multi-agent beats don't
  align (`src/cron/heartbeat-monitor.ts:95-118`).
- Config (`agents.*.heartbeat`) is the desired state; the persisted job owns
  the actual tick. `openclaw doctor --fix` materializes missing/stale monitor
  rows. `cron.enabled=false`/`OPENCLAW_SKIP_CRON=1` disables scheduled
  heartbeats entirely — *there is no fallback timer*.
- Default interval **30m**, or **1h** when resolved auth is Anthropic
  OAuth/token (`DEFAULT_HEARTBEAT_EVERY = "30m"`,
  `src/auto-reply/heartbeat.ts:19`). `every: "0m"` disables the cadence but
  keeps the disabled job + scratch, and targeted event wakes still work.
- **Prompt is sent verbatim as the user message** on the normal system
  prompt. Default (`src/auto-reply/heartbeat.ts:8-13`):

  > "Follow the heartbeat monitor scratch context when provided. Recurring
  > tasks are automations; create or change their schedules with the
  > automations tool, not heartbeat scratch. Do not infer or repeat old tasks
  > from prior chats. If nothing needs attention, reply NO_REPLY."

  When the structured tool is used, the prompt appends:
  "Use heartbeat_respond to report the wake outcome. Set notify=false when
  nothing needs the user's attention. Set notify=true with notificationText
  only when the user should be interrupted." (`HEARTBEAT_RESPONSE_TOOL_*`,
  `src/auto-reply/heartbeat.ts:14-16`).
- **Monitor scratch** replaced `HEARTBEAT.md`: a per-job prose blob (<=256
  KiB) stored in the cron store (`cron_job_scratch` table,
  `src/cron/store/schema.ts:8`), edited via `openclaw cron scratch <jobId>
  --set` with CAS (`--expected-revision`), or by the agent itself via
  `heartbeat_respond`'s `scratch` param (full replace, runner-side CAS).
  `doctor --fix` imports legacy `HEARTBEAT.md` → scratch, converts its
  `tasks:` block into real cron jobs, archives the file; runtime never reads
  it. Effectively-empty scratch (only comments/headings/empty checkboxes)
  skips the turn with `reason=empty-heartbeat-file`
  (`src/auto-reply/heartbeat.ts:55-90`); missing scratch still runs.
- **Busy/cooldown guards**: scheduled wakes defer while the main queue or
  automation work is active, while any same-agent run is active, or while the
  target session has work. Min wake spacing 30 s; flood guard defers anything
  (even `immediate`) at >=5 wakes/60 s
  (`src/infra/heartbeat-cooldown.ts:20-27,88-104`). Manual/immediate wakes
  bypass the same-agent check but not main/automation/target guards.
  Coalescing window `DEFAULT_COALESCE_MS = 250`
  (`src/infra/heartbeat-wake.ts:120`) merges simultaneous wake requests into
  one turn.
- **Active hours**: `heartbeat.activeHours {start,end,timezone}`; outside the
  window ticks skip. Same start=end = zero-width = always skipped.
- **Timeout**: `heartbeat.timeoutSeconds`, else `agents.defaults.timeoutSeconds`,
  else `min(every, 600s)`.
- Cost levers: `isolatedSession` (fresh session, no history — ~100K→2-5K
  tokens/run), `lightContext` (skip workspace bootstrap files), cheap `model`,
  `target:"none"` for internal-only. Warned failure mode: a heartbeat that
  switches the shared session to a small local model leaves it for the next
  real turn ("model bleed" detection exists).
- Manual wake: `openclaw system event --text "..." --mode now|next-heartbeat`;
  `system heartbeat last|enable|disable`.
- Event-driven wake sources also include exec completion:
  `tools.exec.notifyOnExit` (default true) enqueues a system event + heartbeat
  request when a backgrounded exec exits; `notifyOnExitEmptySuccess` covers
  silent successes (`docs/gateway/background-process.md`).

### Response contract (proactivity gate)

`heartbeat_respond` is a **one-shot tool** created per turn: fields `outcome`,
`notify`, `summary`, `notificationText`, `reason`, `priority`, `nextCheck`,
`scratch`; second call throws "already accepted"
(`src/agents/tools/heartbeat-response-tool.ts:75-85`). The structured result
beats text fallback. Text fallback: `NO_REPLY` = silent; legacy `HEARTBEAT_OK`
accepted only at start/end and suppresses the reply if the remainder is <=300
chars (`DEFAULT_HEARTBEAT_ACK_MAX_CHARS`, `src/auto-reply/heartbeat.ts:18`,
`stripHeartbeatToken` ~153-220 — normalizes `**HEARTBEAT_OK**`/`<b>` markup
before matching; mid-message tokens ignored). Outside heartbeats, stray
edge-positioned `HEARTBEAT_OK` is stripped + logged; a token-only message is
dropped (`src/auto-reply/tokens.ts:5-7`). A `notify:false` outcome still gets
recorded as bounded internal context for the next user turn; an undelivered
`notify:true` alert is recorded with its delivery reason — latest outcome, not
a history. Reasoning-only payloads never deliver; the last outbound-capable
non-reasoning payload is selected.

### Cron/scheduler (Automations)

Everything scheduled is a row in `cron_jobs` (SQLite, shared
`~/.openclaw/state/openclaw.sqlite`) (`src/cron/store/schema.ts`). CLI:
`openclaw automations` (`cron` alias). Schedule kinds (`src/cron/types.ts:28-53`,
`docs/automation/cron-jobs.md` ~line 72):

| kind | flag | notes |
|---|---|---|
| `at` | `--at` | one-shot ISO/relative; offset-less + `--tz` |
| `every` | `--every` | `everyMs` + deterministic `anchorMs` |
| `cron` | `--cron` | 5/6-field, `--tz`; DOM/DOW use **OR** (doc warns); top-of-hour wildcard auto-staggered <=5 min unless `--exact`/`--stagger` |
| `on-exit` | `--on-exit` | fires once when a watched command exits; survives turn teardown |
| `stream` | `--stream-command` | supervised long-lived argv; `line`/`match` modes, regex gate, batch on 250 ms quiet or 16 KiB; 5 consecutive sub-60s runs → error state |

- **Timer**: single re-arming `setTimeout`, delay clamped to
  `MAX_CRON_TIMER_DELAY_MS = 60_000` and floored at `MIN_REFIRE_GAP_MS =
  2_000` (anti-spin) (`src/cron/service/timer-execution-timeout.ts:24-35`).
- **Trigger scripts** (condition watchers): JS evaluated each interval —
  min 30 s, 30 s eval budget, <=5 tool calls — returning
  `{fire, message?, state?}`; `state` <=16 KiB, deeply frozen, persisted only
  when a fired payload run *succeeds* (so checks are read-only, actions live
  in the payload). `cron.triggers.enabled: false` is the hard off switch —
  they run unattended with the agent's full tool policy including exec.
- **Dynamic cadence (pacing)**: per-job `{min,max}` bounds; agent/script can
  propose `nextCheck` which is clamped into bounds
  (`src/cron/pacing.ts:37-52`). Script payloads can return
  `{notify, wake:"now"|"next-heartbeat", state, nextCheck}` (300 s default /
  900 s cap timeout, 50/200 tool budget).
- **Startup catch-up**: overdue *isolated agent-turn* jobs are deferred
  `DEFAULT_STARTUP_DEFERRED_MISSED_AGENT_JOB_DELAY_MS = 2min` rather than
  fired during channel-connect; at most `DEFAULT_MAX_MISSED_JOBS_PER_RESTART
  = 5` missed jobs run per restart, staggered `DEFAULT_MISSED_JOB_STAGGER_MS
  = 5_000` (`src/cron/service/timer-execution-timeout.ts:37-41`).
- **Failure backoff**: consecutive execution errors reschedule at
  `[30s, 1m, 5m, 15m, 1h]` (`src/cron/service/jobs-scheduling.ts:49-55`);
  10 consecutive failures → `autoDisabled{reason:"consecutive-failures"}`
  (`src/cron/service/auto-disable.ts:14`); 3 schedule-computation errors →
  `schedule-errors` (`src/cron/service/jobs-scheduling.ts:370`).
- **Payloads**: `systemEvent` (text enqueued to main session, no model call),
  `agentTurn` (`--message`), `command` (shell/argv on gateway host, 10 min
  default, no model), `script` (code-mode headless); system-owned kinds
  `heartbeat` + `skillCollectionReview` can't be created/edited via CLI.
- **Session targets**: `main` (system event + optional wake; uses owning
  session's delivery context — chat `--channel` is *rejected* for main jobs),
  `isolated` (fresh `cron:<jobId>` session; carries safe prefs but no ambient
  routing/elevation), `current` (detached run bound to the creating
  conversation, commits result back), `session:<id>` (named persistent).
  CLI without session context defaults to `isolated`.
- **One-shots** auto-delete on confirmed delivery, intentional suppression, or
  best-effort; failed/unknown required delivery keeps the job *disabled* for
  inspection without replaying.
- **Run policy**: explicit per-job tool list stored at creation; agent-created
  jobs are capped to the creating turn's tools and can't widen. Exec approval
  during an automation → "Always allow" mints a scoped standing grant.
- **Failure alerts**: job route → `delivery.failureDestination` layered over
  global `cron.failureAlert` → primary announce target. Default: alert after
  2 consecutive failures, 1 h cooldown. Delivery failure is a *distinct*
  outcome (`status:ok` + `completionStatus:"failed"`, no backoff).
  Notifications carry a Control UI `Inspect` link when `gateway.publicOrigin`
  is set; raw errors stay in run history, not in chat.
- **Deliberate design**: promoted/created jobs are **enabled**, not
  pending-approval — "a disabled job is invisible to every guard… a worse
  failure than a job that runs and visibly complains"
  (`docs/automation/cron-jobs.md` ~line 210). Promotion flow: agent notices a
  repeated request → restates schedule+task in plain words → creates enabled
  → immediately force-runs once as a visible test → deletes on failure.
- `/loop` chat shortcut exists for quick loops.

### Event triggers (hooks/webhooks)

Three distinct surfaces (`docs/automation/hooks.md` "Choose the right
surface"):

1. **Internal hooks** — `HOOK.md` (YAML frontmatter `metadata.openclaw.events`,
   `requires.bins/env/config`, `os`) + JS/TS handler, run *in the gateway
   process* (trusted code, unsandboxed). Events: `command:new|reset|stop`,
   `session:auto-reset`, `session:compact:before/after`, `session:patch`,
   `agent:bootstrap` (can mutate bootstrap file list), `gateway:startup`,
   `gateway:shutdown` (5 s wait), `gateway:pre-restart` (10 s budget),
   `message:received|transcribed|preprocessed|sent` (async observation only).
   Enabled via `hooks.internal.entries.<key>.enabled`; shipped bundled hooks:
   `boot-md`, `command-logger`, `compaction-notifier`, `session-memory`.
2. **Plugin hooks** — typed `api.on(...)` for in-process interception
   (tool calls, `before_agent_reply`, `reply_payload_sending`, etc.).
3. **HTTP webhooks** — `hooks.enabled` (default off), bearer `hooks.token`:
   `POST /hooks/wake` (system event → `eventOutcome: queued|coalesced`,
   `mode:"now"` requests immediate heartbeat), `POST /hooks/agent` (full agent
   turn; `sessionMode` isolated|persistent, `deliver` flag, `Idempotency-Key`
   for safe retry — replayed 200 doesn't re-run), `POST /hooks/<name>` mapped
   through `hooks.mappings` (template/JS transform → wake|agent|null→204;
   `forEach` fan-out <=200 items, ~8 s admission window, in-memory replay
   cache — explicitly *not* durable exactly-once). Caller session keys need
   `hooks.allowRequestSessionKey` + `allowedSessionKeyPrefixes`. 429 after
   repeated auth failures.
   - **Gmail preset**: `openclaw webhooks gmail setup` configures a
     `gog gmail watch serve` sidecar the gateway auto-starts when
     `hooks.gmail.account` is set (`OPENCLAW_SKIP_GMAIL_WATCHER` opts out);
     per-message session `hook:gmail:<message-id>`; recommended pattern is a
     dedicated `mail_reader` agent — sandboxed `all`, `workspaceAccess:none`,
     `tools.profile:"minimal"`, denied fs/runtime/web/browser/cron — because
     email is untrusted input (`docs/automation/cron-jobs.md` ~line 812).
     IMAP variant ships as a plugin (`docs/automation/imap.md`).

## 3. Proactivity policy

The default posture is **quiet-unless-needed**, enforced structurally:

- The heartbeat prompt literally forbids inventing work ("Do not infer or
  repeat old tasks from prior chats") — proactive behavior is opt-in via
  scratch or automations, and the doc recommends a separate scheduled job even
  for a "check in with the human" nudge (`docs/gateway/heartbeat.md` ~line 90).
- Structured `notify` boolean + text fallback `NO_REPLY`/`HEARTBEAT_OK` with
  strict edge-position rules (see §2). `no_change` acks and confirmed
  notifications aren't stored; silent-but-meaningful results are kept as
  bounded context for the next user turn.
- Wake-side guards: min spacing 30 s, flood defer at 5/min, coalescing 250 ms,
  busy checks on main queue/automation/target session, active-hours window,
  empty-scratch skip — the machine side suppresses wakes before the model ever
  decides (`src/infra/heartbeat-cooldown.ts:20-27`;
  `src/infra/heartbeat-wake.ts:120`).
- Delivery-side suppression reasons (`deliverySuppressionReason`) are
  recorded — `NO_REPLY`, empty output, heartbeat acks — so silence is
  auditable, and a vetoed send (hook) is a delivery *error*, not silence.
- `directPolicy:"block"` suppresses DM heartbeat delivery (`dm-blocked`);
  `target:"owner"` never resolves to a group.

## 4. Channels & delivery

**Inbound routing is config-deterministic, not model-chosen**
(`docs/channels/channel-routing.md` ~line 87): exact peer → parent peer
(thread inheritance) → peer wildcard → Discord guild+roles → guild → Slack
team → account → channel → default agent (`agents.entries.*.default`, else
first/`main`). Broadcast groups can fan one peer to multiple agents after
mention-gating.

**Session keys**: DMs collapse to `agent:<id>:main` under default
`session.dmScope:"main"` (alternatives `per-peer`, `per-channel-peer`,
`per-account-channel-peer`); groups default `per-group` →
`agent:<id>:<channel>:group:<id>` (`:thread:`/`:topic:` suffixes). Even with
shared main history, external DMs get a *derived per-account runtime key* for
sandbox/tool policy so channel-originated input isn't trusted like local main
runs. **Owner pinning**: with exactly one non-wildcard `allowFrom`, that entry
pins the main-DM owner; non-owner DMs don't overwrite `lastRoute`.

**Owner/main session**: "Home" — the rolling main session every DM lands in,
which also receives: coalesced group-activity notices (per-conversation, "the
agent sees them next time it runs"), subagent/task completion announcements,
heartbeat turns (`docs/concepts/main-session.md` ~52-71). `commands.ownerAllowFrom`
(first concrete entry, else channel `allowFrom`) defines the owner DM used by
heartbeat `target:"owner"`; unresolvable → `reason=no-route` skip.

**Outbound is a durable SQLite queue**, `delivery_queue_entries` in
`openclaw.sqlite` (`src/infra/delivery-queue-sqlite.ts:97,165`;
`src/infra/outbound/deliver-queue.ts`):

- Admission writes a stable `deliveryIntentId`; `withStableDeliveryPreparation`
  serializes prep under a lease (`STABLE_PREPARATION_LEASE_MS = 5min`, renewed
  30 s — `src/infra/outbound/delivery-queue-preparation.ts:22-23`). Duplicate
  stable intents attach to the pending/completed owner instead of sending twice.
- Platform send runs under `PLATFORM_SEND_OWNER_LEASE_MS = 30_000` with
  heartbeat at lease/3 (`src/infra/delivery-queue-sqlite-claim.ts:20`;
  `src/infra/outbound/delivery-queue-lease.ts:3`) — a crashed producer's
  custody expires and is reclaimed.
- `queuePolicy: "required" | "best_effort"` per payload
  (`src/infra/outbound/delivery-queue-types.ts:43`).
- Retry: `DEFAULT_MAX_RETRIES = 5`; permanent-error regexes (chat not found,
  bot blocked/kicked, invalid recipient…) fail fast without retry
  (`src/infra/outbound/delivery-queue-recovery.ts:94-106`). Announce retries
  only when *no payload may have reached the recipient* — partial/ambiguous
  sends are never replayed (at-most-once beats at-least-once for chat).
- Media are spooled to disk before queue admission; orphan spool grace 24 h
  (`src/infra/outbound/delivery-queue-media-spool.ts:26`).
- After settlement, failed rows discard payloads; crash-ambiguous owners keep
  a minimal permanent receipt preventing duplicate delivery. Delivered
  receipts are durable tombstones so a reconnecting producer can't re-send.

**Target syntax**: provider prefixes (`telegram:123`) select a channel only
when channel is unresolved; kind prefixes (`channel:<id>`, `user:<id>`,
`thread:<id>`, `room:<id>`) are channel-internal. Explicit channel + foreign
prefix is rejected — WhatsApp never interprets a Telegram id as a phone number.

## 5. Sessions & long work

- **Main session** is rolling: no auto-reset by default; compaction in place;
  `/new|/reset` flushes the tail to daily memory notes and re-primes next
  session, keeping old transcripts searchable under the same key. Disk budget
  ~10 GB → oldest unreferenced history archived to compressed files
  (`docs/concepts/main-session.md` ~87-106).
- **Tasks** = the ledger for all detached work (ACP runs, subagents, every
  automation run, CLI ops); heartbeat turns create none
  (`docs/automation/tasks.md`). Lifecycle `queued→running→
  {succeeded,failed,timed_out,cancelled,lost}`; stored in `task_runs` /
  `task_delivery_state` / `flow_runs` tables. Sweeper every **60 s**:
  reconcile vs runtime backing (mark `lost` after 5 min grace; 30 min for
  childless native subagents), ACP session repair, `cleanupAfter` stamping,
  prune (terminal 7 d, lost 24 h).
- Completion is **push**: notify a channel directly or wake the requester
  session/heartbeat — polling loops are "the wrong shape".
- Steer/`/queue` gives mid-run steering; queued-turn cancellation has a
  gateway-owned cancel identity.

## 6. Memory & user model

Out of scope for deep dive, but the seams that matter for always-on:
`MEMORY.md` curated file + `memory/YYYY-MM-DD.md` daily notes (flushed before
compaction, re-primed after reset); recall across the agent's own sessions
defaults on only for true personal setups (dmScope `main`); monitor scratch is
a per-job memory slot the *agent itself* can rewrite via `heartbeat_respond`;
automation `state` (16 KiB frozen per run) is the machine-checkable memory for
watchers.

## 7. Goals / standing orders / task lists

- **Standing orders** (`docs/automation/standing-orders.md`): permanent
  authority grants as Markdown in workspace files (AGENTS.md), injected into
  every session. Anatomy: Program → authority, approval gate, trigger
  cadence, escalation rules, "what NOT to do". Enforced by cron jobs whose
  prompt *references* the order ("Execute daily inbox triage per standing
  orders…"). Explicit "Execute-Verify-Report" discipline and "3 attempts max,
  then escalate" rules exist to kill the acknowledge-without-doing failure
  mode. Living document; start narrow, widen as trust builds.
- **Standing intents** (`docs/concepts/standing-intents.md`): event-conditioned
  prospective memory — "when X is mentioned, remind me Y". Stored in the
  agent DB; deterministic FTS keyword prefilter (<=256 candidates, all-terms
  match, no model call); scope/cooldown (24 h)/fire budget (3)/expiry (90 d)
  rechecked in one sync transaction; on hit injects a bounded hidden context
  block into the reply. Owner-only `intent` tool; creation requires
  authenticated channel identity. Time-based → automation; aspiration →
  markdown with review date.
- **Task Flow**: durable multi-step orchestration above the task ledger
  (managed/mirrored sync, revision tracking, `tasks flow list/show/cancel`).

## 8. Reliability

The standout subsystem — `docs/gateway/restart-recovery.md` is essentially a
crash-safety contract:

- **Graceful restart drains**: stop admission, wait up to **5 min** for active
  turns/tasks; node-command replies still accepted during drain so cleanup
  finishes. Forced restart/crash marks each active session for recovery.
- **Three detection mechanisms**: at turn *admission* (append user msg + mark
  running + record recovery delivery claim in **one SQLite txn**, before model
  runs); at *shutdown* (stamp recovery marker); at *startup* (scan for
  "running" sessions with no live owner + clean stale transcript locks).
- **Resume**: seconds after boot, re-dispatch with a synthetic "your previous
  turn was interrupted; continue from transcript" message; if a final reply
  was generated but undelivered, its text is included so the agent delivers
  rather than redoes work. Retries: 3 transient reconciliations w/ backoff +
  durable budget of **3 charged dispatch attempts** surviving restarts;
  charge refunded on pre-acceptance reject, kept when outcome ambiguous.
  Exhaustion → **tombstone** (session quarantined until `/new`/`/reset`).
  One durable dispatch id per cycle → ambiguous failures can't double-dispatch.
- **Message-tool-only replies** get a second durable correlation: unresolved
  delivery intent → provider confirm resolves to delivered receipt; unknown
  outcome resumes with **restart-safe (read-only) tools** so the model can
  inspect/report ambiguity without replaying external effects. Turns without
  reconstructable channel authority are tombstoned — "cannot safely mint
  message-action authority without the original channel-ingress claim."
- **Subagents**: registry in SQLite, restored on boot; interrupted >2 h →
  finalized not resumed; repeated recovery failure → wedged tombstone.
- **Cron**: schedules re-arm on boot; overdue isolated jobs deferred (§2).
- **Delivery queue**: drains/retries (§4); expired producer custody reclaimed;
  settlement is resumable *without resending*.
- **Crash-loop breaker**: `gateway_boot_lifecycle` table; **3 unclean boots in
  5 min** → control plane still starts, channel/plugin autostart suppressed;
  manual override `gateway call channels.start`; self-recovers after the full
  window drains (a clean safe-mode boot proves control plane, not that
  autostart is safe) (`src/infra/gateway-boot-lifecycle.ts:20-24,83-95`).
  Outcomes: `clean_stop|planned_restart|safe_mode_stable|startup_failed|
  startup_failure_repaired|forced_stop`; 24 h row retention.
- **Freeze/sleep detection**: within ~30 s of a clock gap, restarts channel
  connections once work is idle; refreshes health/presence.
- **Not recovered**: sessions owned elsewhere (subagent/cron/ACP), PTYs,
  never-admitted inputs (rejected during drain, not silently queued).
- Health probes: `/health(z)` liveness, `/startup(z)` (503 starting/draining),
  `/ready(z)` deep channel checks; detail gated to local/authed callers
  (`docs/gateway/health.md` ~61-73). Channel health monitor restarts channels
  only after their own 10-attempt auto-restart ladder gives up; a channel
  whose durable *ingress queue* can't open is unhealthy even if transport is
  fine. Prometheus: `openclaw_session_recovery_total`,
  `openclaw_session_recovery_age_seconds`.
- **Schema gate**: newer on-disk `user_version` → exit 78 + park LaunchAgent —
  refuses to flap an incompatible build (`docs/reference/database-schemas.md`).

## 9. Safety for unattended action

- Structured escalation everywhere: approval cards to connected surfaces;
  "Always allow" mints a **scoped standing grant** (per job/action, revocable)
  so recurring automations don't re-prompt.
- Unattended run contract: the final reply *is* the deliverable — no plans or
  questions; `NO_REPLY` when nothing to do; failures stated plainly; scheduler
  owns retry (`docs/automation/cron-jobs.md` ~line 380).
- SSRF guard on all outbound automation webhooks (loopback/private refused;
  `cron.webhookSsrfPolicy` exact-host exemptions).
- Hook ingress hardening: dedicated token, `allowedAgentIds`,
  `allowRequestSessionKey`+prefixes, body caps, auth-failure throttling;
  untrusted content → restricted reader agent (the Gmail pattern).
- Per-job tool allowlists; agent-created jobs can't exceed creator's tools.
- Per-channel `healthMonitor.enabled`, `directPolicy:"block"`, active hours,
  `target:"none"` — plenty of "do less" knobs.
- Internal hooks are explicitly flagged trusted/unsandboxed; `cron.triggers`
  flag gates the unattended-code-exec surfaces.

## 10. UX

- Terminal-first by design (VISION: "users see docs, auth, permissions,
  security posture up front"); Control UI = web app with Home main session,
  Threads/Groups/Coding sidebars, "Talk to your Home agent" dock (⌘⇧H) that
  attaches a bounded snapshot of the current page/work file to your message.
- `openclaw doctor --fix` is the migration/repair hammer (config schema, cron
  rows, HEARTBEAT.md import, service drift); `openclaw status`, `channels
  status --probe`, `logs --follow`, `security audit --deep` (recommends DM
  isolation when it sees multiple senders).
- Automations CLI is complete: `list/get/show/runs/run --wait/edit/
  enable/disable/remove/scratch`; jobs show resolved delivery routes at
  create time; `run --due`, `run` `mode:"if-enabled"`.
- "Feel alive" details: typing on enqueue, presence events, heartbeat display
  in UI (`ui/src/lib/chat/heartbeat-display.ts`), Inspect links into Control
  UI in failure notifications, completion pushes instead of polling.
- Multiple gateways per host supported via profiles (unique
  port/config/state/workspace), mostly "rescue bot" use; `gateway
  status --deep` scans for stale cross-install services.

## 11. Verdict for Gray

Ranked steals, with the Gray-shaped version:

1. **Heartbeat = a system-owned cron job, not a parallel timer.** One
   scheduler, one job store; heartbeat config becomes desired-state projected
   into a `heartbeat:<agent>` job row (with a disabled-row-stays trick for
   `0m`). Gray already has a cron ticker — make heartbeat a reserved job kind
   instead of a second loop. (`src/cron/heartbeat-monitor.ts`)
2. **The response contract: one-shot `heartbeat_respond` + edge-anchored
   NO_REPLY fallback.** Structured `notify/summary/notificationText` beats
   regexing prose; keep the text token as fallback with the <=300-char edge
   rule so a stray "HEARTBEAT_OK." mid-paragraph can't nuke a real reply.
   (`src/agents/tools/heartbeat-response-tool.ts`, `src/auto-reply/heartbeat.ts`)
3. **HEARTBEAT.md → DB "scratch" with CAS + empty-scratch skip + agent
   self-edit.** Per-job prose memory the agent rewrites through the same tool;
   `doctor --fix`-style migration from the file. Cheap to copy: a scratch
   column on the cron row + a `--expected-revision` CLI flag.
4. **Deterministic wake guards before the model decides**: min spacing,
   flood cap (5/min), 250 ms coalescing, busy checks, active hours, empty-file
   skip. Every proactivity "don't spam" rule belongs in the scheduler, not the
   prompt. (`src/infra/heartbeat-cooldown.ts`)
5. **Durable outbound delivery queue with custody leases.** SQLite table +
   stable intent ids + producer lease (5 min) + send lease (30 s) + permanent-
   error regexes + "retry only when provably nothing was sent". This is the
   piece that turns "the gateway crashed mid-send" from a duplicate-message
   bug into a resume. Gray's queue needs the intent-id + lease pair even in a
   v1. (`src/infra/outbound/delivery-queue-*`)
6. **Admission-time recovery record.** The single biggest restart-recovery
   idea: write `user msg + session running + delivery claim` in *one* txn
   before the model runs. Then startup recovery is a scan, not archaeology —
   and undelivered-reply text rides along so the agent delivers instead of
   redoing. (`docs/gateway/restart-recovery.md`)
7. **Charged attempt budget + tombstone.** N (they use 3) durable resume
   attempts, refunded on pre-accept reject, kept when ambiguous; exhaustion
   quarantines the session instead of looping forever. Copy the *accounting*,
   not just the retry.
8. **Crash-loop breaker.** 3 unclean boots/5 min → control plane up, side
   services (channel plugins for us) suppressed, manual override + self-heal
   after the window. Small table, big operational win.
   (`src/infra/gateway-boot-lifecycle.ts`)
9. **Payload spectrum for cron**: `systemEvent` (no model) / `agentTurn` /
   `command` (no model) / `script`, crossed with `main|isolated|current|
   session:<id>` targets and `announce|webhook|none` delivery — plus
   `on-exit` and `stream` triggers. Gray's cron can start with
   `systemEvent`+`agentTurn`×`main|isolated` and grow.
10. **Failure alerting as a first-class route**: threshold 2, cooldown 1 h,
    escalation chain job→failureDestination→announce target, auto-disable at
    10 with a *notification* (never silent disable). Plus "create enabled +
    immediate test run" promotion — supervising enabled jobs beats invisible
    disabled ones.

**Explicitly not worth copying:**

- The full delivery-queue idempotency baroque (receipt keys, mirror rows,
  settlement FSM across ~15 files). Copy intent+lease+max-retry+permanent-
  error list; defer the rest.
- Multi-agent broadcast groups, Codex-app envelopes, mapped-hook transforms —
  volume of machinery for niches Gray doesn't have.
- `HEARTBEAT_OK`-in-the-middle nuance and the whole legacy-token compatibility
  matrix — start with `NO_REPLY`/`heartbeat_respond` and don't acquire the
  debt.
- Per-agent SQLite-per-database fleet topology (OpenClaw's multi-tenant
  shared-gateway model) — Gray's single-owner model doesn't need it.
