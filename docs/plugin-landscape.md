# Plugin landscape scan - 2026-10-09

External indexes + ecosystems worth mining for gray plugins. Complements
plugins.md (23 named port units already: 9 pi protocol-2.0 drivers + 14
hermes candidates, +~48 pi extensions via gray-pi-compat).

## The indexes

- ClawHub (clawhub.com): OpenClaw marketplace. Plugins + skills, signed
  manifests, moderated releases, version history, publisher handles,
  download counts. 12 plugins / 30 skills — small but high-signal.
- skills.sh: open agent-skills directory (`npx skills add owner/repo`),
  AGENTS.md-ecosystem leaderboard, 1.2M+ tracked installs.
- Glama (glama.ai/mcp/servers): MCP registry with quality+maintenance
  grades. 97,619 servers.
- smithery.ai / mcp.so: other MCP registries, larger and noisier.
- pi.dev: 79 example extensions in reference/pi-mono (catalogued).
- hermes-agent: 18 plugin dirs in ~/.hermes (catalogued).
- opencode: JS hook plugins (.opencode/plugin/*.ts), tool.execute
  before/after + auth + events. No central index; npm community pkgs.
- OpenClaw docs (docs.openclaw.ai): the most complete plugin taxonomy
  anywhere — steal the shape.

## OpenClaw's taxonomy (the map worth copying)

Plugin kinds: feature / tool / channel / provider / CLI-backend
(wrapping another agent runtime — Codex harness, Copilot SDK harness) /
meeting (Zoom, Meet, FaceTime, Teams, Slack huddles, voice-call via
Twilio/Telnyx/Plivo). Plus hooks (tool-call policy, prompt/session,
message/delivery, gateway+install lifecycle), automations (heartbeat,
schedules, inbound webhooks, Gmail PubSub, IMAP trigger, standing
orders), skills (custodian, workshop, self-learning), secrets
(1Password, Vault SecretRefs).

gray's sidecar protocol already covers tool/hook/ask surfaces; the gap
is catalog breadth, not protocol.

## Concrete plugins seen in the wild (not yet in plugins.md)

### From ClawHub featured list (download counts = demand signal)
- Memory LanceDB (15.5k) — vector LTM w/ auto-recall. Complements
  gray-memory (curated) with retrieval-grade memory.
- Lobster (10.9k) — typed pipelines + resumable approvals. A workflow
  engine as plugin.
- Diagnostics OTel / Prometheus (7.9k / 9.1k) — same as hermes
  observability candidate; confirmed demand.
- OpenClaw Firecrawl / DuckDuckGo / Parallel / Tavily (8-13k) — web
  search/fetch providers; overlaps graysearch, check first.
- Diffs (6.7k) — read-only diff viewer + file renderer.
- Voice Call (7.3k) — real phone calls (Twilio/Telnyx/Plivo).
- Google Meet / Zoom (4k / 630) — meeting participant plugins.
- Lossless Context Management (2.5k) — DAG-based conversation context;
  maps to the hermes context_engine idea.
- OpenShell Sandbox (1.7k) — NVIDIA sandbox exec backend w/ mirrored
  local FS. A bash-backend plugin.
- Expedia / Shopify AI Toolkit — vertical SaaS toolkits; the pattern is
  one sidecar per SaaS, cheap to multiply.

### From OpenClaw docs (utility plugins not on ClawHub's front page)
- Admin HTTP RPC — expose the agent over HTTP.
- Session Share — publish/hand off a session (pairs w/ pi handoff).
- Session sync/attach/search/prune — session lifecycle tooling.
- Workboard / Logbook — persistent structured state + activity log.
- 1Password / Vault SecretRefs — secrets never in plaintext config.
- Honcho memory + user model + standing intents + dreaming + active
  memory — memory that works between sessions; an architecture, not a
  plugin.
- Local ONNX decision models — tiny local classifiers as a plugin
  (routing, policy, safety) with no LLM call.
- Inbound webhooks / Gmail PubSub / IMAP triggers / standing orders —
  gray has cron (time); the gap is event-driven triggers.
- Presence + multi-agent routing / parallel specialist lanes / delegate
  architecture — coordination layer past gray-subagents.
- Codex harness / Copilot SDK harness — a sidecar that wraps another
  agent as gray's runtime. 'Foreign-agent harness' is a new plugin kind.

### From skills.sh (skill-shaped; install via github:/url: spec)
Top by installs, filtered to gray-relevant: find-skills (meta-skill:
discover skills for the task — gray should ship this), grill-me /
to-questionnaire (structured interrogation; overlaps gray-questions UX),
improve-codebase-architecture, domain-modeling, tdd, diagnosing-bugs,
handoff (also a pi driver), triage, to-prd, to-issues, code-review,
resolving-merge-conflicts, git-guardrails, setup-pre-commit,
image-to-code, writing-for-agents. Takeaway: a skill pack is ~90%
cheaper than a plugin — anything prompt-shaped should be a SKILL.md,
not a sidecar.

### From Glama (97k MCP servers)
gray-mcp (protocol 1.3, dynamic tools) makes all of MCP one config away,
so the opportunities are around MCP:
- MCP Verification Gate (seen on Glama) — preflight a server before
  connecting: tool diffing, poisoned-description scan, trust grade.
  A `gray mcp doctor` / gate sidecar.
- Registry browser — search Glama/smithery, install into gray-mcp
  config: `gray plugin install mcp:<server>`.

## Infrastructure worth stealing (host-side, not a plugin)
- Signed manifests + moderated index — ClawHub's trust story: sign index
  entries, `gray plugin audit`, publisher handles, version
  history/rollback. gray already sha256-pins; signing is next.
- Plugin bundles — one name installs a curated set.
- `gray plugin doctor` — probe every registered sidecar
  (manifest/version/claim conflicts).
- Compat sidecars beyond pi: claude-code-plugin-compat (a CC plugin is
  skills+commands+hooks+mcp — mountable), opencode-hook-compat.
- Skill self-authoring (custodian skills / skill workshop) — distill a
  solved session into a SKILL.md. Pairs with hermes self-learning.

## Ranked: the shortlist
1. find-skills skill — free, highest leverage: makes every other
   ecosystem reachable from inside gray.
2. Event triggers (webhook/email-in -> turn) — cron's missing half.
3. Vector memory (LanceDB-style) alongside curated gray-memory.
4. MCP gate + registry install — 97k tools, safe and one-command.
5. Session share/handoff (pi handoff + OC session-share merged).
6. Sandbox exec backend (OpenShell-shaped) — safer bash.
7. Foreign-harness sidecar — run Codex/Copilot/Claude as gray.
8. Voice call — the demo nobody forgets.
9. Local ONNX policy model — instant, free routing/permission calls.
10. Plugin signing + bundles + doctor — trust infra before the catalog
    grows.
