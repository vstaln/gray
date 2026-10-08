# Study brief (shared by every research agent)

Goal: Gray (a Rust coding agent with a small `gray gateway` daemon: cron ticker + control socket,
chat channels as separate plugin processes like the Discord plugin) must become a proper
ALWAYS-ON, PROACTIVE agent. You are studying a reference product to steal its best ideas.

Rules: READ-ONLY on the reference source. Write ONLY your assigned output file. No builds, no
installs, no git commits, no network unless told. Cite `path:line` for every claim about code.
Prefer reading the real code over docs; when docs and code disagree, say so. Be concrete:
data structures, file formats, prompts (quote the actual heartbeat/proactive prompt text),
timings/defaults, state machines. Depth over breadth; ~400-900 lines of markdown is fine.

Cover these sections (skip one only if the product truly has nothing, and say so):
1. Process model: what is long-lived (daemon/gateway), how it is started/supervised, how
   agent turns run (in-process? subprocess?), concurrency/queueing per session/channel.
2. Wake sources: heartbeat loop (interval, prompt, HEARTBEAT.md-style checklist, quiet hours,
   active hours), cron/scheduler (formats, catch-up, missed runs), event triggers (webhooks,
   file watchers, email/calendar polling, system events), self-scheduling by the agent.
3. Proactivity policy: how the agent DECIDES to message unprompted vs stay silent
   (NO_REPLY / HEARTBEAT_OK tokens, scoring, dedupe, rate limits, "don't spam" rules),
   what context it sees on a wake (recent messages, memory, pending tasks).
4. Channels & delivery: adapters, inbound routing to sessions, outbound delivery queue,
   retries/acks/durability, owner/"home" channel, multi-channel identity.
5. Sessions & long work: session-per-channel/thread, resume after restart, compaction,
   background tasks, sub-agents, interrupt/steer while busy, message batching.
6. Memory & user model: files/DB, what is written when, consolidation ("dreaming"),
   how it's injected into prompts, per-user vs global.
7. Goals / standing orders / task lists: persistent objectives the agent works on unattended.
8. Reliability: restart recovery, crash loops, health checks, idempotency, catch-up.
9. Safety for unattended action: approvals, sandboxing, allowlists, budgets.
10. UX: onboarding, config surface, CLI/TUI/web control, what makes it FEEL alive.
11. Verdict for Gray: ranked list of the 10 ideas most worth copying, each with
    "how Gray would do it" in one or two sentences, and anything to explicitly NOT copy.
