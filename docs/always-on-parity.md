# Always-on parity

What makes Hermes (Nous Research), OpenClaw, Grok Bot, Dot and Muse "always on", and what Gray is missing.

Status: study done (2026-10-07). Hermes was read from `~/.hermes/hermes-agent` @ 5a8e8a6b87 because `reference/NousResearch/hermes-agent` is a dangling symlink. OpenClaw was read from `reference/openclaw` @ aec22ecc.

Legend: **D** = documented by the vendor or press coverage of the launch, **I** = inferred. Numbers in brackets are sources (listed at the bottom).

## Parity table

| Capability | Hermes | OpenClaw | Grok Bot (xAI) | Dot (New Computer) | Muse (Meta) | Gray | Gap |
|---|---|---|---|---|---|---|---|
| Heartbeat / wake loop | Session-scoped `/heartbeat every 10m <prompt>`. It is injected only when the session is idle, busy ticks are coalesced, and it persists in `state_meta` [hermes_cli/heartbeat.py:1-26,45,202]. A 5s poll in the gateway sends a synthetic message [gateway/run.py:23369,23395-23435]. **D** | System-owned heartbeat: one cron job per agent, a main-session turn every 30m, sent to the `owner` DM and never to a group, with "reply NO_REPLY" if there is nothing to do [docs/gateway/heartbeat.md:15-23,73-75]. It defers while busy [:81] and keeps a `heartbeat_respond` notify gate and outcome memory [:106-110]. Also supports activeHours and a manual `system event --mode now` wake [src/infra/heartbeat-wake.ts:1, heartbeat-runner-scheduler.ts:50]. **D** | Keeps working with the laptop closed; comes back when approval is needed. **D** [1] | Sent messages "from time to time"; the cadence was never documented. **I** [4] | "Keeps working after people close the app"; "returns when something changes". **D** [5][6] | None. The "heartbeat" in `cron/store.rs:25-41` is only a ticker-liveness stamp. | **Big.** Ship a heartbeat as a cron preset: a recurring job, origin delivery, and the `[SILENT]` gate (`cron_fire.rs:24`). This only becomes useful once gateway fires reach chat (row below). |
| Cron / scheduled jobs | 60s tick under `.tick.lock` [cron/scheduler.py:1-9]. Silent marker [:732,746]. `deliver=origin` by default from chat [cron/jobs.py:2313]. Interrupted runs are recovered [cron/executions.py:205]. **D** | SQLite store that survives restarts. Overdue jobs are rescheduled at boot, and one-shots are deleted only once delivery is confirmed [docs/automation/cron-jobs.md:46-50,67]. Reminders capture the live chat target [:450]. Retries happen only if nothing was delivered, and failure alerts are kept separate [:444,456-472]. **D** | Routines learned by watching a job done once, then run on their own. **D** [1]. Grok (chat) automations: daily, weekly or at set times. **D** [3] | Not documented | "Respond to schedules and relevant events". **D** [6] | `~/.gray/cron` jobs.json, ticked every 60s by the gateway [gateway/run.py:96-113]. Claimed fires, max 4 [cron_serve.rs:512,568]. `[SILENT]` gate, last_* fields [store.rs:142-148]. Origin/Target delivery [store.rs:81]. | **Bug.** Gateway, `serve` and REPL ticks drop the deliveries of chat-origin jobs. Only `cron tick --json` emits `cron_delivery` [cron_serve.rs:613,643; run.rs:113]. Also, the add warning tells the model to start `cron serve`. Fix with an outbox plus an origin-aware warning. |
| Event triggers | Hash-suppressed monitors [cron/monitor.py] and per-job notepad state [cron/notepad.py]. **D** | Condition scripts that return `{fire,message,state}` with persisted state [cron-jobs.md:141-161]. Stream sources [:96-113]. **D** | Works inside inboxes and apps; triggers implied. **I** [1] | Not documented | Watched school emails and a district website, then added dates to the calendar. **D** [6] | A pre-run script wake gate `{"wakeAgent":false}` [cron_fire.rs:12]. | Small: no persisted trigger state and no stream sources. |
| Gateway daemon + channels | A single gateway process that owns its platforms: discord, telegram, slack, signal, whatsapp, matrix, email, sms and more [gateway/platforms/, plugins/platforms/]. **D** | The gateway owns 161 channel extensions [extensions/]. A gateway lock detects stale owners [src/infra/gateway-lock.ts:1]. **D** | Desktop (including Linux) and iOS; Bots talk to each other in group chats. **D** [1][2] | iOS app only. **D** [7] | Muse app, WhatsApp, glasses "soon". **D** [5] | `gray gateway run` handles the cron tick and the control socket. Channels are separate plugin processes (Discord polls `cron tick --json` every 60s). | Medium: the gateway doesn't own channel delivery, so anything it fires must hand off through a durable file (the outbox). |
| Proactive outbound (unprompted) | Heartbeat, cron and goals all deliver to origin. Consent-first automation suggestions [cron/suggestions.py:1-22]. **D** | Heartbeat to the owner, cron announce/webhook [cron-jobs.md:398-400], background tasks wake the heartbeat [heartbeat.md ~333], NO_REPLY token [src/auto-reply/tokens.ts:7]. **D** | Mainly asks for approvals; no unprompted chat documented. **I** [1] | Core feature: "suggestions or questions about advancing your interests". **D** [4] | "Make suggestions unprompted"; decides "whether new results warrant a notification". **D** [5][6] | Only user-created cron jobs, and their chat delivery is currently lost when the gateway fires them. | Big: no unprompted check-in, and no notify-worthiness gate beyond `[SILENT]`. |
| Persistent memory / user model | MEMORY.md and USER.md, snapshotted per session [tools/memory_tool.py:1-20,247]. Background review and curator [agent/background_review.py:1-15, agent/curator.py]. **D** | Markdown memory: USER.md, MEMORY.md, a daily `memory/YYYY-MM-DD.md` and DREAMS.md [docs/concepts/memory.md:7-25]. Nightly "dreaming" consolidation via cron `0 3 * * *` [docs/concepts/dreaming.md:11,154-161]. **D** | "Retain context across conversations, learning preferences and edge cases". **D** [1] | Memory gives info "consistent identity", e.g. recipe "cards". **D** [4]. Structured memories with status and date fields. **I** (from a search summary of [8], not verified) | "One persistent conversation"; Memory files the user can inspect and edit. **D** [6] | The `gray-memory` sidecar plugin (user and decisions, frozen snapshot, `/memory`). | Small or medium: no background consolidation. It could be a cron preset like the heartbeat. |
| Goal planning / tracking | `/goal` Ralph loop with a judge model, state in `goal:<sid>` [hermes_cli/goals.py:1-20]. **D** | Standing orders in AGENTS.md: scope, triggers, approval gates and escalation [docs/automation/standing-orders.md:9,18-35]. **D** | Not documented | Not documented | Goal → plan → "advances the work on its own"; Goals tab. **D** [5][6] | Loads AGENTS.md, but has no `/goal`. | Medium. |
| Approval gates + audit | Execution records [cron/executions.py]. Delivery ledger [gateway/delivery_ledger.py:1-40]. **D** | Run history. The delivery queue tracks pending, failed and completed [src/infra/delivery-queue-sqlite.ts:1,36-37]. **D** | Comes back only when approval is needed. **D** [1] | n/a (no actions) | Approval cards; "complete audit trail of everything it has done and plans to do". **D** [5][6] | `cron/output/` plus last_* fields. The `gray-permissions` and `gray-ledger` plugins. | Small: no delivery ledger (see the outbox). |
| Session resume | Auto-resume of interrupted turns after a restart [gateway/run.py:1298-1379,12508-12513]. **D** | Interrupted turns are auto-resumed: a synthetic message after boot, 3 attempts, then a tombstone [docs/gateway/restart-recovery.md:21-34,193-215]. **D** | Implied by long-running jobs. **I** [1] | Single ongoing thread. **I** [4] | "One persistent conversation". **D** [6] | Manual: `gray -r`, `--continue` [main.rs:161-213, resume.rs:325]. | Medium: no auto-resume after a restart. |
| Supervision / auto-restart | systemd `Restart=always` [hermes_cli/gateway.py:4077] and launchd KeepAlive [:314]. Restart-loop guard and sd_notify [gateway/restart_loop_guard.py, gateway/systemd_notify.py]. Watchdog [gateway/run.py:3025-3032]. **D** | systemd `Restart=always` [src/daemon/systemd-unit.ts:82] and launchd KeepAlive and throttle [launchd-plist.ts:342]. Restart sentinel [src/infra/restart-sentinel.ts:1,52]. **D** | Hosted cloud computer, so the vendor runs it. **I** [1] | Hosted | Hosted | runit or systemd --user units [gateway/service.rs:5-7]. Crash versus clean is recorded [gateway/lifecycle.rs:8-9,31]. 65s drain [run.rs:18]. | Small: no restart-loop guard and no startup delivery recovery. |

## What the closed products do that the open ones don't

Hermes and OpenClaw already have partial versions of 1 (OpenClaw `heartbeat_respond` notify) and 2 (Hermes `/goal`). The closed products make them the main thing users see.

1. **Decide whether to notify** (Muse [6]). The agent judges whether a result is worth a ping instead of sending one every time. Dot's failure mode argues for this: "after a handful of weak suggestions… I'd rapidly start ignoring proactive messages" [4]. **Worth copying:** add a gate to proactive sends ("is this worth interrupting?") and a quiet default.
2. **Goals as first-class objects** (Muse [5][6]). A goal becomes a plan, the agent works it in the background, and progress shows on a Goals tab. **Worth copying:** this maps directly onto a `/goal` command whose state survives restarts.
3. **Audit trail of done + planned actions** (Muse [5]). **Worth copying:** this is cheap if every background turn already writes a log line.
4. **Routines by demonstration** (Grok Bot [1]). **Skip for now:** it needs a recording UI and its own computer.
5. **Bot-to-bot group chats** (Grok Bot [1]). **Skip:** Gray runs one agent.
6. **Memory the user can inspect and edit** (Muse [6], Dot [4]). **Worth copying:** keep memory as plain files the user can edit.

## Gray's gaps, in priority order

1. **Deliver gateway-fired chat jobs.** Non-host drivers write a durable `cron/outbox/`, and `tick --json` drains it. Also, the add warning should stop telling chat-bound jobs to start a driver. This is the root cause of "gateway keeps failing" (`gray-discord-plugin/docs/cron-reminders-handoff.md`).
2. **Heartbeat preset.** A recurring origin job whose prompt ends with "reply [SILENT] if nothing needs attention" (OpenClaw's pattern).
3. **Delivery recovery at startup.** The outbox is the ledger: anything not drained gets re-sent.
4. **`/goal` plus standing orders.**
5. **Auto-resume of interrupted turns, and a restart-loop guard.**
6. **Memory consolidation** as a nightly cron preset.

open-dots (`reference/Anil-matcha/open-dots`) is a self-hosted Muse/Dot-style prototype with chat, a deny-by-default action gateway and audited approvals. It has no proactive loop, so there's nothing to copy for this goal.

## Notes

- Dot shut down on 2025-10-05 [7], so it only counts as a design reference. Its main lesson is that weak proactive messages train users to ignore them [4].
- "Grok Bot" (Aug 2026, a cloud agent [1]) is a different product from Grok's chat "automations" [3]. Both are listed above.

## Sources

1. https://www.unite.ai/xai-launches-grok-bot-always-on-ai-teammates-with-their-own-cloud-computers/
2. https://www.iphoneincanada.ca/2026/08/12/xai-debuts-grok-bot-ai-teammates-you-can-give-real-work-to/
3. https://mindstudio.ai/blog/grok-automations-scheduled-tasks-email-triggers
4. https://notes.andymatuschak.org/zBmgU9c2rjvApZvTa68YfAr
5. https://about.fb.com/news/2026/09/introducing-muse
6. https://www.testingcatalog.com/meta-introduces-muse-as-a-proactive-personal-agent/
7. https://techcrunch.com/2025/09/05/personalized-ai-companion-app-dot-is-shutting-down
8. https://sourceforge.net/app/dot-by-new-computer/web-app/
