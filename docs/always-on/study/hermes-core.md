# Study: Hermes Agent — gateway, cron, heartbeat, goals, delivery

Reference: `/home/vstaln/.hermes/hermes-agent` (Python, ~460 KLOC). HEAD at study time:
`5a8e8a6b87 fix(terminal): strict Linux-only gating for background-executor systemd scopes`.
Scope per assignment: `gateway/` (platform adapters, delivery, session routing), `cron/`
and scheduler, heartbeat, monitor/trigger code, `hermes_cli/goals.py`, background review,
delivery ledger. Memory/skills/CLI/TUI are covered by the sibling study
(`hermes-memory.md`); they appear here only where they intersect the wake/delivery path.

Hermes is extremely battle-tested — comments cite issue numbers into the #90000s, and most
mechanisms below exist because a specific incident forced them. The comments are themselves
a catalog of always-on failure modes.

---

## 1. Process model

- **One long-lived gateway process per profile** (`gateway/run.py:7297`,
  `class GatewayRunner`; entry `main()` at `gateway/run.py:33432` →
  `asyncio.run(start_gateway(config))` at :33508). All chat adapters (Telegram, Discord,
  Slack, WhatsApp, Signal, webhook, api_server, ~20 platforms —
  `gateway/config.py:325` `class Platform(Enum)`) run **in-process** on one asyncio loop.
- **Agent turns are in-process, on executor threads.** The runner builds/caches an
  `AIAgent` per session (`gateway/run.py:6185` `ctx.AIAgent(...)`, cache OrderedDict at
  :7635-7641 with LRU eviction `_enforce_agent_cache_cap` at :29410) and runs turns via a
  `ThreadPoolExecutor` (`self._executor`, `gateway/run.py:7542`; calls wrapped in
  `_run_in_executor_with_context`, :11916). No subprocess per turn.
- **Cron jobs also run in-process** on a dedicated thread pool
  (`cron/scheduler.py:1426` `_get_parallel_pool`, submitted at tick time so the ticker
  never blocks). Each run constructs a fresh `AIAgent` with an isolated session id
  `cron_{job_id}_{timestamp}` (`cron/scheduler.py:5780`).
- **Supervision**: systemd/launchd. `gateway/systemd_notify.py` implements `sd_notify`
  watchdog (nonblocking datagram, :21-38); shutdown code sanity-checks
  `TimeoutStopSec` against drain budget (`gateway/run.py:13505-13532`); there is an
  "exit-78" contract for `RestartPreventExitStatus` (:14165-14190) so config-fatal
  failures don't restart-loop.
- **Concurrency per session**: routing-key busy guards (`_running_agents`,
  `gateway/run.py:7394+`) plus a durable **turn lease** keyed by *resolved session_id*
  (`gateway/turn_lease.py:1-45`) — needed because `switch_session()` makes routing
  key→session_id many-to-one, and two aliased turns once interleaved transcript flushes
  into a permanent `user;user` alternation wedge. Lease release is generation-scoped,
  identity-checked, fail-closed on timeout.
- **Control socket**: local-only Unix socket `$HERMES_HOME/gateway.sock` (named pipe on
  Windows) answering versioned JSON verbs `identify` / `status` — one request per
  connection, filesystem ACLs are the auth boundary (`gateway/control_socket.py:1-55`).
  A connectable socket IS liveness; no PID-reuse heuristics.
- **Scale-to-zero**: on relay-only deployments the gateway can quiesce the relay socket
  and *self-suspend* the Fly machine once "no in-flight turn ∧ inbound-quiet ∧ no live
  background work" holds (`gateway/scale_to_zero.py:1-50`). Wake is platform-side via a
  registered wakeUrl.

## 2. Wake sources

Hermes has **four distinct wake mechanisms**, deliberately separated:

### 2a. Cron (durable, cross-process)

- Jobs live in `~/.hermes/cron/jobs.json` (per-profile — `cron/jobs.py:60-75` warns never
  to anchor on the shared root; that would leak credentials/skills across profiles).
- **Ticker**: in-process daemon thread, `TICKER_INTERVAL_SECONDS = 60`
  (`cron/jobs.py:99`), calls `tick()` (`cron/scheduler.py:7829`). A file lock
  `~/.hermes/cron/.tick.lock` serializes overlapping processes. The ticker touches
  `cron/ticker_heartbeat` every loop and `cron/ticker_last_success` on clean ticks
  (`cron/jobs.py:91-97`) so `hermes cron status` can tell "ticker thread alive but
  failing" from "healthy".
- **Schedule kinds** (`parse_schedule`, `cron/jobs.py:962`): `interval` ("every 30m",
  bare "30m"), `cron` (5-6 field croniter expressions, plus natural phrases "every
  monday 9am" / "weekdays at 9am" → `_natural_every_to_cron` :906), `once` ("in 30m",
  ISO timestamp; naive timestamps anchored to the *configured* Hermes timezone, not
  server-local, :1051-1084). One-shots get a 120 s past-due grace
  (`ONESHOT_GRACE_SECONDS`, `cron/jobs.py:119`).
- `next_run_at` is persisted per job and recomputed defensively: due-scan re-arms jobs
  whose persisted next-run doesn't match the current expr (`_cron_next_run_matches_expr`,
  `cron/jobs.py:1361`), self-heals `next_run_at=None` scheduled jobs, and re-arms stale
  `last_status="error"` jobs parked in the future (:1217-1225).
- **Claim-before-run**: `claim_job_for_fire` (`cron/jobs.py:3440`) + an in-memory
  `_running_job_ids` set with per-job allowance `max(2×interval, 30min)`
  (`cron/scheduler.py:803`, `_INFLIGHT_MIN_ALLOWANCE_MINUTES`). A stale-claim sweep
  (`sweep_stale_inflight` :1053) force-releases leaked claims in-cycle. Execution rows in
  `cron/executions.db` carry owner `pid`+process-start-time; dead-owner rows are
  reclaimed periodically (`recover_interrupted_executions`, tick at :7897+).
- **Missed runs**: `advance_next_runs` pre-advances recurring jobs at most once per fire
  — no catch-up storm; a missed window is simply skipped to the next occurrence.
- **EMFILE resilience**: fd-exhaustion during tick raises loudly (not swallowed as "lock
  held"), triggers fd reclamation + exponential tick backoff capped at 15 min
  (`cron/scheduler_provider.py:30-58`, `_EMFILE_BACKOFF_MAX_SECONDS`).
- **Pluggable scheduler providers**: `cron.provider` config selects a provider
  (`cron/scheduler_provider.py:1-25`); the built-in is the 60 s thread, but a managed
  provider (Chronos, for scale-to-zero NAS deployments) can own *when* while
  `run_job`/`_deliver_result` own *what*.

### 2b. Session heartbeat (in-process, session-scoped)

- `/heartbeat every 10m <prompt>` — **user-owned recurring instruction bound to a
  session** (`hermes_cli/heartbeat.py:1-27`). Persisted in SessionDB `state_meta` keyed
  `heartbeat:<session_id>` so `/resume` picks it up.
- Hard floor `MIN_INTERVAL_SECONDS = 60` (:43) — "more often than once a minute is a
  busy-loop, not a heartbeat". Drivers poll every `POLL_SECONDS = 5.0` (:45).
- The injected prompt template (:47-53):

  ```
  [Heartbeat — recurring instruction, fires every {interval}]
  {prompt}

  If there is nothing meaningful to do or report for this instruction right now,
  reply briefly that nothing has changed and stop — do not invent work.
  ```

- Fires **only when the session is idle** — the gateway poller skips sessions in
  `_running_agents` (`gateway/run.py:23409-23433`); a due tick during a busy turn
  **coalesces**: it fires once at the next idle poll, never stacks a backlog. Firing is
  recorded *before* the turn runs so a slow turn can't double-fire
  (`due_prompt`, `hermes_cli/heartbeat.py:214-230`). Missed ticks re-anchor to NOW, not
  the theoretical schedule.
- Injected as a **plain user message** — no system-prompt mutation, so prompt caching and
  role alternation stay intact (invariant stated at :14-27).
- **Known gap, deliberate**: the gateway's watch registry (`_heartbeat_watch`) is
  in-memory (`gateway/run.py:23373` comment) — after a restart, firing resumes only when
  the user touches `/heartbeat` again. Durable scheduling is cron's job. Heartbeats also
  migrate across compression session-rotation (`migrate_heartbeat_to_session`, :295+).

### 2c. Event triggers

- **Webhook adapter** (`gateway/platforms/webhook.py:1-65`): aiohttp server, per-route
  config in `config.yaml platforms.webhook.extra.routes`: event-type header filters,
  **required HMAC secret** (V2 signature binds a timestamp for replay protection; V1
  accepted with warning), per-route rate limit (fixed window), body size limits,
  **idempotency cache** for webhook retries, prompt templates formatted with the
  payload, optional skills to load, `deliver` routing — and `deliver_only: true` mode
  that skips the LLM entirely (the rendered prompt IS the message) for zero-cost push
  notifications.
- **msgraph_webhook** (email/calendar surface) and signal/whatsapp/weixin/etc.
  adapters all normalize inbound into `MessageEvent`.
- **Process-completion / watch-pattern triggers**: background processes spawned via
  `terminal(background=true)` are tracked by `tools/process_registry.py`. Completion
  events go onto a shared `completion_queue` (:473) with a JSON checkpoint file
  `~/.hermes/processes.json` (:63) for crash recovery — undelivered completions are
  restored on boot (:478). `watch_patterns` let a long-lived process signal mid-run
  (e.g. a CI poller matching a log line), rate-limited hard: min 15 s between matches,
  3 consecutive strikes → watch disabled, 8 lifetime hits → permanent fallback to
  notify-on-exit (:72-88 — added after a service restarting all day forced a full-context
  agent turn per match, #93513).
- **Kanban watchers** (`gateway/kanban_watchers.py`): background loops subscribing to
  kanban boards, driving a multi-agent dispatcher.
- **Delivering a wake**: `gateway/wake.py` `deliver_wake` picks one of two strategies by
  the adapter's `supports_async_delivery` flag (`gateway/platforms/base.py:3092`,
  default True; the API server sets False at `api_server.py:1505`):
  - push-capable adapters: inject a synthetic `MessageEvent(internal=True)` through
    `adapter.handle_message`;
  - stateless request/response adapters: **self-POST `/v1/chat/completions`** with the
    raw `X-Hermes-Session-Id` header so the wake resumes the real session (running under
    the derived session key would land in an invisible parallel session — a real bug
    documented in the module docstring). Timeout 600 s, retries at 2/5/10 s on HTTP 429
    (the API server's global concurrency cap), and **failures raise** so callers can
    rewind cursors.

### 2d. Self-scheduling

The agent can create cron jobs itself via the `cronjob` tool
(`tools/cronjob_tools.py`) → `create_job` directly, and `/goal wait`-style barriers let
an in-flight goal park on a pid/session/timer (below). The lifecycle guard prevents the
agent from scheduling its own gateway restart (§9).

## 3. Proactivity policy — when to speak vs stay silent

Two silence lanes, deliberately different strictness (`gateway/response_filters.py`):

- **Interactive lane** (`is_intentional_silence_response`, :50): the response must be
  *exactly* a marker — `[SILENT]`, `SILENT`, `NO_REPLY`, `NO REPLY` (64-char cap, edge
  punctuation stripped, :17-45). Prose mentioning a marker mid-sentence is delivered;
  swallowing a real answer is worse than a stray token.
- **Autonomous lanes** (`is_autonomous_silence_response`, :70 — shared by cron and
  webhook): marker as whole response, on its own first/last line, or bracketed opening
  the response. Rationale in `webhook.py:60-95`: models reliably append "I stayed silent
  because…", which under the strict rule flips to deliver — that produced a support lane
  pinging its owner to report it had nothing to report. The two lanes share
  `LIVE_GATEWAY_SILENT_MARKERS` so the sets can't drift.
- A third regex catches **silence narration** (`*(silent)*`, `🔇`, bare `.`/`…`,
  `gateway/delivery.py:33-55`), length-guarded ≤64 chars, toggleable via
  `gateway.filter_silence_narration` / `HERMES_FILTER_SILENCE_NARRATION`.
- **Cron prompt contract** (prepended to every agent cron job, `cron/scheduler.py:4705-4714`):

  ```
  [IMPORTANT: You are running as a scheduled cron job. DELIVERY: Your final response
  will be automatically delivered to the user — do NOT use send_message or try to
  deliver the output yourself. ... SILENT: If there is genuinely nothing new to
  report, respond with exactly "[SILENT]" (nothing else) to suppress delivery. Never
  combine [SILENT] with content — either report your findings normally, or say
  [SILENT] and nothing more.]
  ```

- **Script-level gates before the LLM**: a job can carry a `script` whose JSON output
  `{"wakeAgent": false}` skips the agent run entirely (`_parse_wake_gate`,
  `cron/scheduler.py:4537`) — cheap pre-filter so the model is only invoked when the
  deterministic check says something changed. `no_agent` jobs skip the LLM outright:
  script stdout is delivered verbatim, empty stdout = silent run
  (`cron/scheduler.py:5511-5627`).
- **Monitor mode** (`cron/monitor.py:1-35`): attach `monitor_script`/`monitor_url` to an
  LLM job; each tick hashes the source's exact output bytes against the stored hash —
  unchanged → agent suppressed entirely (recorded as silent `no_change`); changed → a
  "MONITOR CHANGE DETECTED" block with a capped unified diff (4 000 chars) + new output
  (8 000 chars) is injected into the prompt. Source failure = error, never "change", so a
  recovering source still suppresses. Prior output kept in
  `output/<job_id>/monitor_last_output.txt`.
- **Dedup/rate limits**: dead-target registry skips confirmed-unreachable chats
  (`gateway/delivery.py:344-360`); watch-pattern strike limits (§2c); heartbeat
  60 s floor; webhook per-route rate limits; incident dedup so one repeating failure
  doesn't re-ping (§8).
- On a wake the agent sees: the job prompt + optional `context_from` previous job outputs
  (8 K-char cap, `cron/scheduler.py:4660-4690`), its per-job durable **notepad** section
  (KV scratchpad in `cron/notepad.db`, 16 KB/key, 64 KB/job, `cron/notepad.py:1-45`),
  monitor diff, skills, and the cron delivery contract. No quiet-hours/active-hours
  mechanism exists — the only global pause is ESTOP (§9).

## 4. Channels & delivery

- **Inbound routing**: every adapter produces `MessageEvent` (`base.py:2425`) carrying a
  `SessionSource` (`gateway/session.py:149-230`: platform, chat_id, chat_type
  dm/group/channel/thread, user_id, thread_id, scope_id (guild/workspace), profile,
  reply context, `delivered_via_upstream_relay` trust flag, auto-thread metadata).
  `build_session_key` (`session.py:1090`) is the single key constructor:
  `agent:main:<platform>:dm:<chat>[:thread]`; groups → `...:group:<chat>[:user]` with
  per-user isolation on by default; threads shared across participants by default.
  Discord auto-thread continuity keys the session on `prospective_thread_id` so the
  channel-originating message and all thread follow-ups share ONE session.
- **Outbound**: `DeliveryRouter.deliver` (`gateway/delivery.py:318`) fans out to
  `DeliveryTarget`s: explicit `platform:chat_id[:thread]`, platform home channels
  (`TELEGRAM_HOME_CHANNEL`/`DISCORD_HOME_CHANNEL` env, resolved via
  `_get_home_target_chat_id` `cron/scheduler.py:2292`), `origin` (back to where the job
  was created — `deliver` defaults to `"origin"` when origin captured, else `"local"`,
  `cron/scheduler.py:2313`), `all`, `bot-chat:<profile>` (machine-local operator chat),
  `local` (file only). Relay-fronted platforms resolve through
  `resolve_delivery_transport` (`delivery.py:92`) — a Relay adapter can front a logical
  platform only if it explicitly advertises `fronts_platform`.
- **Cron delivery fan-out**: `_resolve_delivery_targets` (`cron/scheduler.py:2840`),
  media attachments routed per-extension to `send_voice/send_image_file/send_video/
  send_document` (:2894+), output truncated at 4 000 chars for non-chunking platforms
  (`MAX_PLATFORM_OUTPUT`, `delivery.py:24`) with a "full output saved to …" footer;
  `MAX_PLATFORM_OUTPUT` is bypassed for adapters that chunk natively.
- **Retries**: connection-class errors are retried by adapters
  (`_RETRYABLE_ERROR_PATTERNS`, `base.py:2910+` — notably read/write timeouts are
  *excluded* because the request may have reached the server; retrying risks duplicates.
  Platforms that know a timeout is safe set `SendResult.retryable=True` explicitly).
  Whole-chat-death errors mark the target in `DeadTargetRegistry`; a later successful
  send clears it.
- **Message batching**: `merge_pending_message_event` (`base.py:2829`) merges photo
  bursts/albums and (optionally) rapid TEXT follow-ups into the queued event so a
  multi-part thought isn't truncated to the last fragment.
- **Delivery ledger** (the crown jewel, `gateway/delivery_ledger.py`): a generated-but-
  unconfirmed final response is "the one artifact the gateway can lose without a trace —
  the turn already burned its tokens". Three checkpoints around every send into a
  `delivery_obligations` table in shared `state.db` (WAL):

  `record_obligation(pending)` → `mark_attempting` → `mark_delivered` / `mark_failed`.

  Crash semantics (:1-45):
  - `pending` — send never started → redeliver plainly;
  - `attempting` — crashed mid-await → redeliver **with a visible marker**: `"♻️
    Recovered reply — the gateway restarted during delivery, so this may be a
    duplicate"` (`RECOVERED_MARKER`, :70). Honest at-least-once, never silent dup;
  - `failed` — retried at the restart boundary, same marker. A distinct
    `RECONNECTED_MARKER` (:80) covers post-reconnect runtime replay, and only errors
    allowlisted as transient (`_RUNTIME_RETRYABLE_ERRORS = {"send_path_degraded"}`,
    :88) are replayed without a restart — fail-closed.
  - Poison rows can't spin: `MAX_ATTEMPTS = 3`, `STALE_AFTER_SECONDS = 24h` →
    `abandoned`, 7-day retention, 500-row cap (:63-66).
  - Owner `pid` + process-start-time on every row; `sweep_recoverable` (:309) only
    claims rows whose owner is *proved* dead. Best-effort: ledger failures must never
    block a send (:43).

## 5. Sessions & long work

- Session-per-channel-key; transcripts + metadata in `state.db` via SessionDB.
- **Resume after restart**: sessions carry `resume_pending` (`session.py:848-854`).
  Crash-left turn markers are promoted to `resume_pending` on boot (:3421+);
  `mark_resume_pending` (:3494) also fires on graceful-drain interruption. Auto-continue
  is freshness-gated — default 1 h window, `agent.gateway_auto_continue_freshness` /
  `HERMES_AUTO_CONTINUE_FRESHNESS` (`session.py:31-51`); a stale `resume_pending` is a
  "zombie" and gets reset instead of replayed (:2980-3000). The resumed turn gets a
  system note: "The previous turn was interrupted … CONTINUE the interrupted task to
  completion" (`run.py:1532-1586`), and interrupted tool-call tails are stripped so the
  model doesn't re-execute them (:1892-1894).
- **Busy policy**: `display.busy_input_mode` ∈ `interrupt` (default) | `queue` | `steer`,
  per-profile, live-changeable (`run.py:10273-10341`). `queue`/`steer` imply the user
  wants messages preserved: FIFO via `_enqueue_fifo` (:9671) producing one full turn per
  event, no merging; `interrupt` cancels the running turn. On restart,
  queue/steer-mode pending messages are preserved, interrupt-mode dropped (:9652-9658).
- **Background tasks**: `delegate_task(background=true)` → daemon-thread subagent
  (`tools/async_delegation.py:1-37`); completions ride the shared `completion_queue` and
  surface as a **new turn when idle** — never spliced mid-turn, preserving role
  alternation and prompt cache. The completion payload carries a rich self-contained
  task-source block (goal, context, toolsets, model, result) because "the parent may be
  deep in unrelated context and won't remember why the subagent existed".
- **Compaction**: `agent/conversation_compression.py` rotates the session; goals and
  heartbeats migrate to the child session (`migrate_goal_to_session`,
  `migrate_heartbeat_to_session`).
- **Stall notification**: `gateway/session_stall.py` — pending inbound + stale progress
  (from `agent.get_activity_summary()`, observation-only contract) → notify once. Kept
  deliberately separate from liveness watchdog and delivery ledger.

## 6. Memory & user model (intersection only)

- File-backed `MEMORY.md` + `USER.md` injected as a **frozen snapshot** at session start —
  mid-session writes update disk but never the live system prompt (cache-preserving;
  `tools/memory_tool.py:1-30`).
- **Background review** (`agent/background_review.py`): after each turn a daemon thread
  forks the AIAgent — same provider/model/credentials/byte-identical cached system
  prompt so the replay is a warm prefix-cache read (:1109-1300) — and asks it to update
  memory and/or the skill library. Tool whitelist limited to memory/skill tools.
  - `_MEMORY_REVIEW_PROMPT` (:465): save persona/preferences/expectations; "If nothing
    is worth saving, just say 'Nothing to save.' and stop."
  - `_SKILL_REVIEW_PROMPT` (:476) / `_COMBINED_REVIEW_PROMPT` (:615): aggressively
    opinionated — "Be ACTIVE — most sessions produce at least one skill update";
    class-level umbrella skills only (no `fix-PR-123` artifacts); enforced
    read-before-write via `skill_view`; long list of do-NOT-capture rules (environment
    failures, negative tool claims, unresolved failures, one-off narratives) because
    "these harden into refusals the agent cites against itself for months".
  - Cron sessions skip it (~30K tokens/event, no human benefit —
    `agent/turn_finalizer.py:800-818`, per sibling study).

## 7. Goals / standing orders — `hermes_cli/goals.py` (2 326 lines)

"The Ralph loop for Hermes" (:2). A `/goal` is a free-form objective that stays active
across turns; after **every** turn an auxiliary-model **judge** decides done/continue/
wait; on continue the goal injects its own continuation user-message until done, budget
exhausted, or preempted.

- **State** (`GoalState`, :547): goal, status (`active|paused|done|cleared`),
  `turns_used`/`max_turns` (default `DEFAULT_MAX_TURNS = 20`, :52), last verdict/reason,
  consecutive parse (cap 3, :73) and transport (cap 5, :78) failure counters →
  auto-pause, `subgoals` list, wait-barrier fields, optional `contract`, `gates` list.
  Persisted in `state_meta` `goal:<session_id>`; migrates across compression.
- **Judge** (`judge_goal`, :1169): strict JSON contract (:152-189):
  - `DONE` — goal satisfied, deliverable produced, or *blocked/needs input* (block =
    done, surfaced);
  - `WAIT` — progress genuinely gated on async work; returns `wait_on_session` /
    `wait_on_pid` / `wait_for_seconds`. Judge sees the live `process_registry` snapshot
    (`JUDGE_BACKGROUND_BLOCK_TEMPLATE`, :195) so it can pick WAIT with a target instead
    of re-poking — "re-poking now would be pure busy-work";
  - `CONTINUE` — default when in doubt.
  - Judge output budget `4096` tokens (:63) because reasoning models burned hidden
    tokens then truncated the JSON verdict — a real incident.
  - **Fail-open**: judge errors → `continue`; the turn budget is the backstop. But
    consecutive *parse* failures (3) or *transport* failures (5) auto-pause — a broken
    judge must not burn the whole budget.
- **Continuation prompts** (:92-150): plain user messages, variants for bare goal,
  contract, subgoals, and gate-failure. Base template:
  "[Continuing toward your standing goal] Goal: {goal} … Take the next concrete step.
  If you believe the goal is complete, state so explicitly and stop. If you are blocked
  and need input from the user, say so clearly and stop."
- **Completion contract** (`GoalContract`, :334): five fields — outcome, verification,
  constraints, boundaries, stop_when — adapted from Codex's "strong goal" guidance;
  `/goal draft` uses `DRAFT_CONTRACT_SYSTEM_PROMPT` (:261) to turn plain language into
  the contract for user review. Contract judge demands *concrete evidence* of the
  verification criterion (:233-260).
- **Quality gates** (`GoalGate`, :429): deterministic shell commands (`/goal gate add
  <cmd>`) that must ALL pass before the judge may say done — a failed gate *short-
  circuits the judge*; its bounded output tail (3 000 chars) becomes the continuation
  prompt (`CONTINUATION_PROMPT_GATE_FAILED_TEMPLATE`, :135). Defaults: 300 s timeout,
  3 retries (:86-87).
- **Wait barriers** (:575-590, `wait_on`/`wait_on_session`/`wait_for_seconds`
  :1746-1832): while parked, `evaluate_after_turn` short-circuits without burning a turn
  or calling the judge; auto-resumes when the pid exits / session trigger fires /
  deadline passes.
- **User preemption**: a real user message mid-loop preempts the continuation *and*
  pauses the goal for that turn (:21-24). Goal status notices are delivered *after* the
  main response via adapter post-delivery callbacks so reading order stays natural
  (`run.py:23497-23540`).
- **Cron suggestions** (`cron/suggestions.py`): automation proposals (catalog starters,
  skill `blueprint:` blocks, usage-observed recurring asks, integration onboarding) are
  surfaced for **explicit one-tap accept** — they never auto-create jobs; dismissals
  latch by `dedup_key`. Consent-first design worth copying wholesale.

## 8. Reliability

- **Restart recovery**: resume_pending + freshness gate (§5); delivery-ledger sweep on
  startup (§4); process-registry checkpoint restore (§2c); dead-owner cron execution
  reclaim in-cycle (§2a).
- **Restart-loop breaker** (`gateway/restart_loop_guard.py`): if the gateway keeps
  booting with restart-interrupted sessions pending (e.g. an agent session whose resumed
  turn re-runs the SIGTERM that killed it — a documented ~10 s respawn loop), it
  records each such boot in `gateway/restart_loop.json`; chains while inter-boot gaps
  stay under 300 s (gap-chaining catches slow wedges a fixed window can't see), trips at
  3 boots/60 s → **skips auto-resume that boot** but still serves inbound. Fails open on
  any I/O error.
- **Crash-loop guards everywhere**: cron execution rows with owner liveness; in-flight
  claim allowances + stale sweep; interrupted-fire tokens so a killed agent thread can't
  overwrite `last_status` with a plausible-looking false "ok"
  (`mark_running_jobs_interrupted`, `cron/scheduler.py:1253`); goal judge failure caps;
  watch-pattern strikes; delivery-ledger attempt caps.
- **Health/observability**: `agent/monitoring/` (cron_health, gateway_health, OTLP
  exporter, redaction); ticker heartbeat + last-success files; `_log_tick_yield_once`;
  code-skew detection yields the tick to a fresher gateway build after an update
  (`CronTickYielded`, `cron/scheduler.py:209-248`); loop-liveness watchdog on the asyncio
  loop; `shutdown_watchdog`/`shutdown_forensics` for bounded drain.
- **Incidents** (`cron/incidents.py`): durable failure records in `executions.db` keyed
  by `(job_id, normalized error signature)`; lifecycle `detected → alerted → closed`.
  Acking is per-signature — same job+same error resolves to the same incident and stays
  quiet; a changed error mints a new incident. Failure-type ordering buckets 429/
  timeout/auth/delivery/config/script/agent.
- **Idempotency**: `compute_obligation_id` hashes (session_key, message_ref, content);
  webhook idempotency cache; cron claim-before-run; one request/connection on the
  control socket.

## 9. Safety for unattended action

- **ESTOP** (`agent/estop.py:1-60`): `hermes pause` writes `$HERMES_HOME/ESTOP`
  (profile home + fleet root both checked); while present, cron skips dispatch, kanban
  workers aren't spawned, new gateway turns get a "Hermes is paused" reply. **Never
  kills in-flight work** — pause-new-work, resumable. Check = 1-2 `stat()` calls, no
  cache. Corrupt sentinel still counts as engaged (fail-safe).
- **Lifecycle guard** (`cron/lifecycle_guard.py`): `create_job` rejects specs whose
  prompt/script contains gateway-lifecycle command shapes (`hermes gateway restart`,
  `launchctl kickstart …hermes-gateway`, `systemctl restart`, `pkill`) — command-anchored
  regexes so prose doesn't false-positive; profile-aware (`hermes -p <profile>` blocked
  only when it names the running profile). Companion to `terminal_tool` blocking and
  `hermes gateway stop|restart` refusing to self-target. Exists because an agent
  scheduled its own restart → KeepAlive respawn → auto-resume → re-SIGTERM loop.
- **Prompt-injection scan** on assembled cron prompts (`_scan_assembled_cron_prompt`,
  `CronPromptInjectionBlocked`, `cron/scheduler.py:522`) — content fetched from external
  sources (context_from, monitor output) is untrusted.
- **Approvals** (`tools/approval.py`): dangerous-command pattern detection, per-session
  approval state keyed by ContextVar (thread-safe for concurrent gateway turns),
  CLI+gateway-async prompting, auxiliary-LLM smart-approve for low-risk commands,
  permanent allowlist in config.yaml. `HERMES_YOLO_MODE` is **frozen at import** so a
  skill can't set the env var mid-process to bypass approval (:31-34).
- **Authorization**: per-platform allowlists + DM pairing codes (`gateway/pairing.py`:
  8-char codes, 1 h TTL, 3 pending max, 1 request/10 min/user, 5-strike lockout,
  file perms 0600, never logged); `authz_mixin.py` resolves adapter-vs-gateway auth
  boundaries; `internal=True` events bypass user auth checks (synthetic wakes).
- **Budgets/limits**: goal turn budget (20), heartbeat floor (60 s), watch-pattern
  strikes, in-flight claim allowance, delivery-ledger caps, per-job notepad caps,
  media size limits, webhook body/rate limits.
- **Media delivery path validation** (`base.py:1809+`): outbound files must be under
  allowed roots / recently produced; denied prefixes include credential dirs.

## 10. UX

- CLI (`cli.py`), TUI (`ui-tui`, separate Node/React-Ink process over JSON-RPC), Desktop
  app, web dashboard, `tui_gateway` server. Slash commands: `/goal` (set/pause/resume/
  subgoal/gate/wait/draft), `/heartbeat every|pause|resume|clear`, `/queue`, `/busy`,
  `/new`, `/resume`, `hermes cron` family (create/list/run/pause/notepad/suggestions).
- Status lines are alive: heartbeat renders `♥ Heartbeat (every 10m, next in ~Xs, fired
  N×): <prompt>` (`heartbeat.py:194-205`); goal status `⏳ Goal parked — waiting on
  pid …`; cron sessions titled `cron <job_id> …` with unique-title dedup
  (`_set_cron_session_title`, `cron/scheduler.py:85-122`) so they appear searchable in
  `/resume`.
- Gateway status phrases, runtime footer, streaming tool progress / TTS, startup
  watchdog, `gateway_state.json` + control socket for external observability.
- The *feel-alive* levers: proactive delivery to home channels, post-delivery status
  notices in natural reading order, auto-thread creation with LLM-renamed titles,
  pairing-code onboarding (no manual user-ID hunting), one-tap suggestion acceptance.

## 11. Verdict for Gray — ranked list of ideas worth copying

Gray today: Rust agent + `gray gateway` daemon (cron ticker + control socket) + chat
plugins as separate processes (Discord plugin). Ranked by value:

1. **Delivery-obligation ledger** (§4). Gray's biggest gap for always-on: a generated
   response that crashes between finalize and send vanishes silently. Implement:
   `delivery_obligations` table (SQLite or sled) with pending→attempting→delivered,
   owner pid+start-time liveness, sweep-on-boot, `MAX_ATTEMPTS=3`, and the honest
   `RECOVERED_MARKER` prefix on ambiguous redelivery. Steal the `attempting` semantics —
   it's the difference between at-least-once and silent loss.
2. **Dual-strictness silence tokens** (§3). Interactive lane: response must be exactly
   `NO_REPLY`/`[SILENT]`. Autonomous lane (cron/wakes): marker on first/last line counts.
   The incident history (agent pinging to report nothing) is exactly what Gray will hit.
   One shared marker set, two matchers.
3. **`wakeAgent:false` script gate + monitor-hash suppression** (§3). Let a cheap
   deterministic check decide whether the LLM wakes at all: script JSON gate, and/or
   hash-of-output compare injecting a diff only on change. This is the single biggest
   cost lever for frequent cron ticks — Gray should build it before adding more wake
   sources.
4. **Session heartbeat as injected user message** (§2b). `/heartbeat every N <prompt>`
   persisted in session meta, fires only when idle, coalesces missed ticks, min 60 s,
   prompt ends with "if nothing meaningful, reply briefly and stop — do not invent
   work". Cheap to build on top of the existing ticker; keep the in-memory watch +
   "durable schedules belong to cron" split.
5. **Goal loop with DONE/WAIT/CONTINUE judge + wait barriers** (§7). Gray already has
   long-task primitives; add: post-turn judge call on an auxiliary model, turn budget
   (20) as fail-open backstop, parse/transport failure auto-pause counters, and the
   `wait_on_pid`/`wait_for_seconds` barrier so a goal parked on CI doesn't burn turns.
   Skip the contract/subgoals/gates layers initially — but *do* copy quality gates
   (deterministic command must pass before "done") early; evidence-beats-vibes is the
   whole point of an autonomous loop.
6. **Busy-input mode as first-class config** (§5): `interrupt|queue|steer` per channel,
   queue = one full FIFO turn per message (no merging), merge only media bursts.
   Prevents both message loss and turn interleaving.
7. **ESTOP sentinel** (§9). One file check per tick, pause-new-work-only, fail-safe on
   corrupt file, shared root across profiles. Trivial to implement, invaluable once
   Gray runs unattended.
8. **Session-key discipline + turn lease** (§1, §4). Single pure function
   `build_session_key(source)` with documented dm/group/thread rules; plus a lease keyed
   by *resolved* session id (not routing key) serializing load→run→flush. Gray's
   plugin-process model makes the DB-level variant mandatory — Hermes's in-process lease
   has the same documented gap.
9. **Wake routing by adapter capability** (§2c). `supports_async_delivery` flag:
   push-capable plugin → synthetic internal event; request/response surface → re-enter
   through the *real* entry path with the raw session id. Failures raise so the caller
   rewinds/requeues — never silently deliver to a parallel invisible session.
10. **Failure incidents with signature dedup** (§8). Durable `(job, error-signature)` →
    `detected→alerted→closed`; one alert per distinct failure, ack persists. Pairs with
    the delivery ledger to make unattended operation quiet-but-honest.

**Explicitly NOT to copy (or copy with care):**

- **Heartbeat non-durability** — Hermes's in-memory watch registry losing firing on
  restart is a documented accepted gap; Gray's plugin model should persist the watch
  list too, it's cheap.
- **The sheer bespoke complexity**: 8 KLOC scheduler, 33 KLOC run.py, per-profile cron
  stores, dual codec targets, relay fronting. Most of it is scar tissue from a much
  larger deployment surface; Gray should take the *invariants* (claim-before-run,
  owner-liveness, coalesce-missed-ticks, honest redelivery markers) not the structure.
- **In-process Python threads for everything** — Gray's plugin-process isolation is
  arguably better; don't consolidate.
- **No quiet-hours** — Hermes simply doesn't have them; if Gray adds proactive wakes, a
  quiet-hours config is a genuine differentiator to add rather than copy.
