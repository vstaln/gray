# Gray Always: the gateway becomes the agent

Status: design, 2026-10-07. Built from the studies in `study/` (Hermes core + memory,
OpenClaw gateway + agent, nanobot/nanoclaw/picoclaw/zeroclaw/tinyclaw, openhuman,
OpenMausBot, open-dots, opengrok) and the public material on OpenAI Dots, Meta Muse and
xAI Grok Bot (sources at the bottom).

## The problem

Gray is a coding agent with a cron ticker bolted on. The real "always on" runtime lives
inside the Discord plugin: its own SQLite inbox/outbox, its own scheduler, its own
lifecycle notices, a separate gray home per conversation, a `gray -p` spawn per message.
So every always-on behaviour has to be rebuilt per channel, a reminder added in one
conversation cannot see another, nothing wakes the agent unless a human types, and the
gateway itself runs one cron turn at a time inline (a 10 minute job freezes the daemon).

Every product we studied converges on the opposite shape:

* **One brain, many surfaces.** One long-lived gateway owns sessions, scheduling, wakes
  and delivery. Channels are thin adapters. All of the owner's DMs land in one rolling
  **main session** (OpenClaw `agent:<id>:main`, Dots "carry context across every
  channel", Muse "one persistent conversation").
* **Wakes are events.** User messages, cron fires, finished background work,
  external triggers and restart recovery all enter the same queue and become turns in
  some session (openhuman's single trigger funnel, OpenClaw announces, nanobot's file
  trigger inbox).
* **Proactivity is policy, not prompt.** Cheap deterministic gates decide whether the
  model wakes at all (active hours, busy, empty checklist, script gate, flood caps); the
  model then answers `NO_REPLY` or speaks; a delivery policy decides whether and when the
  owner is interrupted (quiet hours, hourly cap). Dot's post-mortem lesson: weak
  proactive messages train the user to ignore all of them.
* **Delivery is durable.** A reply that was generated is never lost: outbox with intent
  ids, attempts and redelivery on boot (OpenClaw delivery queue, Hermes obligation
  ledger, zeroclaw outbox). A turn that was cut by a crash is resumed, a bounded number
  of times, from an admission record written before the model ran.
* **Everything is inspectable.** An activity log of what the agent did, decided not to
  say, and plans to do (Muse audit trail, Dots Activity View), and a pause switch.

## Shape

```
 adapters (discord, telegram, cli, web…)          wake sources
   │  send / subscribe / ack  (gateway.sock)        cron · triggers/ ·
   ▼                                                task done · recovery · gray gateway wake
 ┌──────────────────────────── gray gateway run ─────────────────────────────┐
 │ events/  →  router (session key → gray session)  →  lanes (1 turn/session, │
 │ (spool)       main | chat:<p>:<c> | job:<id>         collect pending)      │
 │                                                     │                      │
 │                                    turn runner: child `gray -p --json      │
 │                                    --session <sid> --input-json` (crash-   │
 │                                    isolated, killable, same code as today) │
 │                                                     │                      │
 │  activity.jsonl ◄── decision: silence tokens → proactivity policy (quiet   │
 │                     hours, hourly cap, owner route) → outbox/ (attempts,   │
 │                     ack, redelivery on boot) → pushed to the adapter       │
 └────────────────────────────────────────────────────────────────────────────┘
```

State lives under `~/.gray/gateway/` as atomic JSON files, the same style as `cron/`
(no new dependencies):

| Path | What |
|---|---|
| `events/<ts>-<uuid>.json` | admitted, not yet run events (the inbox) |
| `sessions.json` | session key → gray session id, last route per key, owner routes |
| `running/<turn>.json` | admission record: events + session + attempt, removed when the turn ends |
| `outbox/<ts>-<uuid>.json` | delivery intents not yet acked by an adapter |
| `activity.jsonl` | append-only audit: event, turn start/end, delivered, suppressed (+ why) |
| `PAUSED` | estop sentinel: autonomous wakes stop, user messages still run |

## Pieces

1. **Events.** `{id, origin, session_key, text, route?, sender?, created_at}`.
   `origin` is provenance (zeroclaw `TurnOrigin`): `user | cron | trigger |
   task | recovery | system`. Every turn knows why it exists; the prompt envelope tells
   the model, and the delivery policy differs by origin.
2. **Router.** Session keys: `main` (owner DMs from any channel, the CLI,
   announcements), `chat:<platform>:<chat>[:<thread>]` (groups and non-owners),
   `job:<id>` (isolated work). The owner is configured per platform
   (`gateway.owners: ["discord:<user id>"]`); with no owner configured, everything is
   per-chat, which is today's behaviour.
3. **Lanes.** One turn per session key at a time, a global cap (default 2). While a lane
   is busy, new events for it queue; when it frees, all pending events for that key run
   as **one** turn (OpenClaw `collect`). Steering into a running turn is a later step
   (needs a core change at the tool boundary, Hermes `steer`).
4. **Turn runner.** A child `gray -p --json --session <sid> --input-json <file>`, with
   `GRAY_CRON_ORIGIN` set to the event's route so `gray cron add` inside the turn binds
   back to the right chat, and `GRAY_TURN_ORIGIN` for provenance. Progress rows are
   relayed to subscribers, so an adapter can render the same live tool bubbles it renders
   today. Behind a trait so tests run with a stub.
5. **Decision + delivery.** Silence tokens, dual strictness (Hermes): for a user turn
   only an exact `NO_REPLY` / `[SILENT]` reply is silent; for an autonomous turn the
   token on the first or last line suppresses it. Then the policy for autonomous output:
   quiet hours defer (not drop) until the window opens, `max_per_hour` caps, and the
   route is the event's route or the owner's last-used DM. Deliveries go to `outbox/`;
   a subscribed adapter for that platform gets them pushed and acks; unacked intents are
   retried with backoff, survive restarts, and die after 8 attempts with an activity
   entry.
6. **Heartbeat: removed.** A timed self-wake cost ~25k input tokens per run for mostly
   `NO_REPLY`. Recurring checks are `gray cron` jobs.
7. **Cron joins the event loop.** The gateway no longer blocks on a fire. Jobs keep their
   store and claim semantics; a fired job's result becomes a delivery (chat-bound jobs)
   or an event into its session (`deliver: main`). Fires run concurrently under the same
   cap.
8. **Triggers.** Any file dropped into `~/.gray/gateway/triggers/` becomes an event
   (`origin: trigger`) and is removed: the zero-ceremony hook for scripts, mail
   watchers, CI, webhooks behind a tiny relay. `gray gateway wake "<text>"` does the same
   from a shell.
9. **Recovery.** On boot, every `running/` record is a turn that was cut short: re-admit
   it as a `recovery` event telling the model the previous attempt was interrupted
   (attempt n of 3); after 3, drop it and tell the owner. Undelivered outbox entries are
   simply retried.
10. **Activity + control.** `gray gateway activity` tails the log; `gray gateway send`
    talks to the main session from a terminal; `pause`/`resume` flip the estop;
    socket verbs `send`, `subscribe`, `ack`, `wake`,
    `activity` next to the existing `identify`/`status`.

## Later (not in the first cut)

* Steer/interrupt into a running turn (core change at the tool-result boundary).
* Goals with a judge loop and evidence gates (Hermes `/goal`, Muse Goals tab).
* Memory consolidation ("dreaming") as a nightly system job on top of gray-memory, and a
  pre-compaction memory flush.
* Triage for noisy external sources (openhuman's drop/ack/escalate classifier on a
  cheap model).
* "Proactive research" with read-only tools while the owner is idle (Dots).
* Crash-loop breaker, failure incidents with signature dedup, deadman alert.

## Migration of the Discord plugin

The plugin becomes an adapter: owner DMs go to `send` on the socket; it subscribes for
`discord` deliveries and progress rows and renders them with its existing card and
streaming code. Its own scheduler/outbox stay for groups until the gateway path proves
itself, behind a config switch, so nothing breaks while both exist.

## Sources (closed products)

* OpenAI, "Introducing dots", 2026-09-29: https://openai.com/index/introducing-dots/ and
  https://chatgpt.com/features/dots/ (proactive research with read-only tools, Activity
  View, Custom Rules allow/approve/block, auto-review, context across channels, notes to
  itself).
* Meta, "Introducing Muse", 2026-09-08: https://about.fb.com/news/2026/09/introducing-muse
  and https://www.testingcatalog.com/meta-introduces-muse-as-a-proactive-personal-agent/
  (one persistent conversation, scheduled and event-driven background work, decides
  whether a result warrants a notification, controls to reduce/increase/disable
  initiative, Goals tab, activity log, editable memory files, Sentinel egress approval).
* xAI Grok Bot, 2026-08-11:
  https://www.unite.ai/xai-launches-grok-bot-always-on-ai-teammates-with-their-own-cloud-computers/
  (own cloud computer, comes back only for approvals, routines by demonstration, bots in
  group chats).
* Dot (New Computer), shut down 2025-10-05: lesson on weak proactive messages,
  https://notes.andymatuschak.org/zBmgU9c2rjvApZvTa68YfAr
