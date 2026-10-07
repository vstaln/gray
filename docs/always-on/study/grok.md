# Study: xAI / Grok — opengrok (Grok Bot model-routing shim kit) + grok-build

References assigned:
- `/home/vstaln/gray/reference/opengrok` — present, studied in full (~69 files, 3.3 KLOC).
- `/home/vstaln/gray/reference/xai-org/grok-build` — **MISSING**: it is a dangling
  symlink to `~/.cache/checkouts/github.com/xai-org/grok-build`, which does not
  exist on this machine (verified: only `pi-mono` remains in that checkout cache;
  a broken-symlink sweep in session `3bf788c1` already flagged it on 2026-10-04).
  Recoverable secondary evidence: a Sept-17 multi-agent benchmark session pinned it
  at commit `bc7f02e` alongside hermes-agent/kilocode/openclaw/opencode/pi, and
  several gray sessions describe it as the source of gray's TUI architecture —
  i.e. **grok-build is xAI's Rust, codex-rs-derived coding-agent TUI** (crates like
  `xai-grok-pager`, `xai-grok-markdown/streaming.rs`). Nothing recoverable speaks to
  always-on/proactive behavior; it is an interactive coding agent, not a daemon.
  Everything below about grok-build is marked [INF] and thin by necessity.

## Evidence-quality legend (premise correction — read this first)

The assignment assumed `wire-captures/` holds captures of the real Grok app/bot
showing proactive features, tasks, scheduled tasks, companions and memory.
**It does not.** `wire-captures/` contains exactly ONE capture set,
`wire-captures/glm-5.3-flash/` — a 7-probe ladder against Zhipu's GLM endpoint
measuring reasoning-effort tokens (`wire-captures/README.md:30-33`). There are
**zero captures of Grok app protocol, proactive pushes, task/scheduler APIs,
companions, or memory sync** anywhere in the repo (verified by full-tree read
and git-history grep). opengrok itself is third-party (OnlyTerp/opengrok), not
xAI code — a "run any model inside Grok Bot" kit.

What *is* documented about the Grok Bot host comes from three indirect sources:
- **Patch anchors** — `tools/apply-box-patch.py` asserts verbatim strings/symbols
  inside the sealed `sand-host/host-main.cjs` + `openai-hop-session.cjs` bundle
  (each anchor asserted `count==1`, `apply-box-patch.py:56-58`). These are real
  symbol names in the shipping host, asserted — not read — so still strong.
- **Watch tables** — `tools/doctor.py` encodes live port/file expectations of a
  production Grok Bot deployment.
- **Route tables** — `tools/provider-maps*.cjs` + `examples/hop-health-snapshot.example.json`
  encode the Grok Bot *harness control plane* verbatim (parameter ids, maxMode).

Tags used below: **[CAP]** = raw wire capture · **[CODE]** = repo source ·
**[DOC]** = repo docs claims · **[INF]** = my inference.

---

## 1. Process model

### Grok Bot host (the product opengrok patches)

- Two host shapes **[DOC]** (`docs/CLOUD-HOST.md`): a desktop app with a config
  dir (`~/AppData/Roaming/Grok Bot`, `~/.grokbot`, `~/.config/Grok Bot`, macOS
  `Application Support/Grok Bot` — `setup.py:41-48`), and a **cloud "box"** —
  a remote computer running `node /home/box/sand-host/host-main.cjs` under a
  supervisor (`apply-box-patch.py` tail: "bounce the host process
  (supervisor-safe, NOT a raw kill)").
- The host bundle is **sealed/attested and self-updating** **[DOC]**
  (`docs/CLOUD-HOST.md` §the-missing-step: "the bundle is sealed/attested";
  `docs/MODEL-GUIDELINES.md` §5: "stock hosts get replaced by self-updates;
  your patches/bindings vanish or get refused").
- Agent execution model, from patch anchors **[CODE]**:
  - `host.subagentModelId` — the host has a **sub-agent** notion with its own
    model resolution (`apply-box-patch.py:64`).
  - `sessionOptions.isSummarizationSession` gates
    `requestKind: "summarization" | "main"` (`apply-box-patch.py:106-108`) —
    the host runs **background summarization turns as a separate request lane**.
  - Every turn's session options may carry
    `{ openaiBaseUrl, provenanceAgentId: host.getConversationId(), skipLabeling: true }`
    (`apply-box-patch.py:84`) — per-conversation provenance is stamped on
    outbound requests.
  - `createOpenAiHopSession(...)` + an executor with `allowTestVisibleRecovery`
    (`apply-box-patch.py:121,131`) — turns go through an "openai hop session"
    abstraction; the executor has a built-in *test-visible recovery* flag
    (suggests canned-recovery behavior exists for tests — an interesting
    honesty hazard: recovery that only triggers under test).
  - A `localQwen` lane is explicitly excluded from the map call
    (`apply-box-patch.py:144-146`) — the host special-cases local models.
- Bindings are **re-read every turn** **[DOC]** ("model-bindings.json — routing
  authority, read fresh EVERY turn", `docs/MODEL-GUIDELINES.md` mental model).

### opengrok's own processes [CODE]

All stdlib-only Python/Node, no deps (a stated virtue, `README.md`):
- `tools/hop-server.py` — threaded HTTP shim, loopback `:18790` default,
  relays any method/path to an upstream `:8642` api_server, injects
  `Authorization` from env/`%LOCALAPPDATA%/hermes/.env`, streams SSE/chunked
  both directions, `_TIMEOUT=1800` ("long agent turns"), `/healthz` probes
  upstream without waking it (`hop-server.py:20-27,41`).
- `tools/model-picker.py` — config UI web server on `127.0.0.1:8766`
  (`model-picker.py:241`), serves one inline HTML page + `/api/state`,
  `/api/save`, `/api/test`, `/api/aiprompt`.
- `tools/file-relay.py` — box-side push/pull file drop on `:8799`
  (`file-relay.py:10,98`), name-sanitized `[A-Za-z0-9._-]`, atomic
  `tmp`+`os.replace` writes, **no auth** (documented as convenience-only).
- `tools/doctor.py` — cron-ready watchdog; `tools/wire-probe.py` — evidence
  probe; `tools/apply-box-patch.py` — idempotent host patcher.
- Persistence model: Windows Startup-folder `.vbs` launchers calling `pythonw`
  (`hop-server.py` docstring; `doctor.py:124` lists `claude-shim.vbs`,
  `codex-shim.vbs`, `antigravity-shim.vbs`, `hermes-hop.vbs`,
  `start_gmsg_daemon.vbs`, `start_sms_lane.vbs` — the last two hint the wider
  deployment has a "gmsg daemon" and an "SMS lane" beyond this repo).

## 2. Wake sources

**Grok Bot itself: nothing documented.** No heartbeat, cron, reminder, or
scheduled-task machinery appears in any anchor, doc, or capture. If Grok's
app-side scheduled tasks/companions exist, this repo contains no evidence of
their wire shape — stated plainly so nobody mistakes silence for absence.

opengrok's wake sources **[CODE]**:
- `doctor.py --quiet` is explicitly "cron watchdog mode: print NOTHING when
  clean, print only problems otherwise; same exit codes" (`doctor.py:15-17`).
  FAILURE-MODES F01 prescribes identity probes "on a cron … every-30-min
  schedule" (`docs/FAILURE-MODES.md:14`). ROADMAP lists "doctor as packaged
  cron" (schtasks/launchd/cron one-flag install) as an open item.
- Staleness-as-event: `~/.grok/models_cache.json` carries `fetched_at` +
  `grok_version`; doctor warns when `age_days > 14` as a "silent-update
  suspect" (`doctor.py:201-217`) — a *vendor heartbeat*, using cache freshness
  to detect the host silently changed under you.
- No file watchers, no event bus, no self-scheduling. The whole system is
  poll-and-report.

## 3. Proactivity policy

Nothing from Grok proper **[INF: no evidence]**. opengrok does encode a crisp
"when to speak" policy for an unattended watchdog worth stealing wholesale:

- **Silent when clean** — `--quiet` exits 0 with zero output when there are no
  FAILs and no *new* WARNs (`doctor.py:305-308`). Noise only on deltas.
- **Known-warning dedupe with a stable key format** — `--init` stores each WARN
  as `"LEVEL::tag::detail"`; later runs diff against that set so already-seen
  warnings never re-alert (`doctor.py:286-291`; the comment warns the key format
  "MUST use the exact same format --init stores" — F03 is the incident where
  mismatched suppression keys made the detector lie).
- **Escalation split** — exit codes are the channel contract: 0 clean /
  1 new warn / 2 fail (`doctor.py:300-316`). A cron wrapper can map these to
  "stay silent / notify / page".

## 4. Channels & delivery

Grok Bot **[CODE/DOC]**:
- Identity = per-agent UUID keys in `model-bindings.json`; each entry =
  `{name, modelId, provider, hopBaseUrl, maxMode, parameters[]}`
  (`examples/model-bindings.example.json`). `provider` is audit metadata only —
  routing never reads it (`docs/MODEL-GUIDELINES.md` §2 hard rules).
- Conversations are first-class: `host.getConversationId()` →
  `provenanceAgentId` on requests (`apply-box-patch.py:84`).
- Newer builds ship native BYOK (`ModelAllowlistByok` — paste a key in-app,
  `docs/BYOK-DECISION.md:3`); the hop+bindings lane coexists for anything
  needing wire fidelity or OAuth/subscription auth.
- Cloud delivery path: local picker → `POST <BOX_RELAY_URL>/push/model-bindings.json`
  → box writes `/home/box/sand-data/model-bindings.json` → patched host reads it
  per turn → requests egress through `hopBaseUrl` (`docs/CLOUD-HOST.md` §flow).
- **Verification of delivery is behavioral, not configurational** **[DOC]**: the
  picker's own `/api/test` "verifies the hop, not the routing — the only proof
  of routing is a normal message in the Bot conversation hitting the hop port"
  (`docs/CLOUD-HOST.md` §6, `tcpdump -i lo port <hop-port>`). Saved ≠ pushed ≠
  consumed ≠ routed; each state has a distinct check.
- Multi-machine delivery over Tailscale: hop `/health` route tables carry
  `shim`, `socks`, `httpProxyFallback` per route
  (`examples/hop-health-snapshot.example.json`) — loopback preferred, SOCKS/HTTP
  proxy as fallback lanes.

## 5. Sessions & long work

Documented host internals (anchors) **[CODE]**:
- **Two request kinds**: `main` and `summarization`
  (`apply-box-patch.py:106-108`). The patch deliberately carries
  `maxMode`/`parameters` into the main lane's sessionOptions spread and skips
  the summarization lane's identical spread (detected by trailing
  `isSummarizationSession: true`, `apply-box-patch.py:84-101`) — i.e. Grok Bot
  runs background summarization turns that are deliberately *not* given the
  user's routing/effort overrides.
- **Sub-agents**: `host.subagentModelId` is the resolution starting point
  (`apply-box-patch.py:64`).
- **Retry/recovery**: `allowTestVisibleRecovery` on the hop executor
  (`apply-box-patch.py:131`) — flagged above as an honesty hazard.

Failure-mode knowledge around long work **[DOC]** (the real gold):
- **F10 — summarizer eats the lane**: background summaries sharing the
  constrained lane produced "100k+ tokens in one turn"; lock = separate
  summarizer route OR strip-tools+cap-output profile; "bounded blocking
  compaction budgets per user turn, fail-closed" (`docs/FAILURE-MODES.md` F10,
  `MODEL-GUIDELINES.md` §4 table).
- **Compaction loops**: "failed turns re-enter summarize→main loops; strict
  one-compaction-per-turn budget, fail closed, reset only on new user turn"
  (`MODEL-GUIDELINES.md` §4).
- **F11 — retry storms**: transient 5xx must retry same-plan immediately;
  cooldown only after *persisted* errors; "return REAL upstream codes verbatim",
  never synthesize 429s that evict healthy lanes.
- **F12 — fail-open routing lies**: bound-route errors must NOT fall through to
  a default provider masquerading as your model — "fail CLOSED on bound routes:
  error out visibly rather than substitute".
- **F08**: wire-level fields passed as SDK kwargs crash client-side mid-run
  ("TypeError mid-run kills a 23-minute job") — unknown keys belong in
  extra_body/body root, behind an allowlist.

## 6. Memory & user model

**Nothing documented.** No companion, persona, memory-sync, or user-model
machinery appears anywhere in the repo. The only memory-adjacent mechanism is
the host's `summarization` request lane (§5) — session compaction, not durable
user memory — plus `~/.grok/models_cache.json`, which is a model-catalog cache
(`{fetched_at, grok_version, models{}}`, `doctor.py:201-217`), not user memory.
[INF] The presence of a dedicated summarization lane + `provenanceAgentId`
suggests per-conversation continuity state server/host-side, but its format is
unknown.

## 7. Goals / standing orders / task lists

**Nothing documented** for Grok Bot. opengrok's own standing order is
`model-bindings.json` — a declarative desired-state file the host re-reads
every turn — and `services.json`/`watched-files.json`/`baseline.json` giving
the watchdog its standing expectations (`doctor.py:86-130`). That pattern —
*agent behavior driven by a small re-read-every-turn JSON file a UI edits* — is
the closest thing to a persistent-objectives mechanism here.

## 8. Reliability

The strongest section of this reference — a production failure encyclopedia
(`docs/FAILURE-MODES.md`, F01–F18) plus tooling that enforces it:

- **Baseline + drift detection** `[CODE]`: `doctor.py --init` snapshots SHA-256
  of watched files + bindings SHA + config flags + known warnings into
  `baseline.json` (`doctor.py:290-299`); later runs emit `drift:file` /
  `drift:bindings` / `drift:config` warnings, including the targeted
  `discover_models False→True` "flood risk" flip (`doctor.py:281-282`).
- **Identity probes, not TCP** `[CODE/DOC]`: a port being open isn't enough —
  probe must return the service/model NAME (`/v1/models` expecting `401` for
  hermes, `"upstream_reachable": true` for the hop, `doctor.py:104-111`).
- **Liveness coupling** `[CODE]`: bindings that route to a *local* port doctor
  owns but that isn't listening → FAIL (`bindings:liveness-coupling`,
  `doctor.py:172-179`); remote/box hops are exempt — don't flag what you don't
  supervise.
- **Persistence inventory** `[CODE]`: every expected launcher (VBS) checked
  present each cycle — "a vanished launcher predicts next-boot breakage"
  (`doctor.py:124-140`, MODEL-GUIDELINES §5.4).
- **Auto-heal, fail-closed** `[CODE]`: `--fix` only relaunches dead services via
  their canonical launcher; "never force-kills a live listener"
  (`doctor.py:19-22`, `try_fix` `doctor.py:222-245`).
- **Anchored patching** `[CODE]`: `apply-box-patch.py` never blind-seds — each
  anchor asserted `count==1` ("a changed upstream bundle fails loudly instead
  of half-patching"), timestamped backup dir first, `node --check` before AND
  after, idempotent re-runs (`apply-box-patch.py` header + `:50-58,170-200`).
- **Update survival** `[DOC]`: layered — SHA attestation, current-only gate
  (refuse unreviewed host versions), cache-staleness tripwire, persistence
  inventory, fail-closed routing (MODEL-GUIDELINES §5).
- **Testing doctrine** `[DOC]`: "every green needs a proven red" (break a real
  service, watch the detector fire), negative control on auth boundaries
  (with-key=200 AND keyless=401 — "both-open = worst outcome"), never mirror
  logic under test, runtime over grep assertions, don't pipe verification
  through exit-code-eating filters (`docs/TESTING.md`).

## 9. Safety for unattended action

- **Secrets law** `[CODE/DOC]`: bindings/host configs carry "ports and slugs
  only. Ever."; keys in env/OS-store/mode-600 dotfiles read by the shim; shims
  never log `Authorization` or bodies; `qa.py` leak-scans every file for
  `sk-`/`Bearer`/key-shaped strings and private IPs on every push
  (`qa.py:55-75`, MODEL-GUIDELINES §2). Captures never store auth headers
  (`wire-captures/README.md` §hygiene).
- **Loopback by default** `[CODE]`: hop binds `127.0.0.1`, file-relay binds
  loopback and documents "never a public port" (`file-relay.py` security notes).
- **Fail-closed over fake success** `[CODE]`: unverifiable controls ship as
  *documented noops with reason strings* (`applied.wire` entries like
  `{status:"noop", reason:"no-hop-context-wire"}`), never pretended —
  `provider-maps-hop.cjs` returns `{body, route, applied, unknownIds}` so every
  applied control is auditable (`provider-maps-hop.cjs:13-17,85-95`).
- **Guardrails on automation**: probe ladder aborts on hard auth errors to
  avoid burning metered quota (`wire-probe.py:76`); routine doctor checks are
  keyless/static so "monitoring itself never costs tokens" (F15).

## 10. UX

- **One-command setup** `[CODE]`: `python setup.py` — DETECT (OS, Grok Bot
  config dir, live services on 8 ports) → PLAN (prints exactly what it will do,
  asks Y/n) → WIRE (adopts existing bindings instead of overwriting; asks only
  the 3 questions it can't answer: agent count/name/model/base-url) → VERIFY
  (runs doctor, prints *why* for every non-green) → OPEN (launches picker)
  (`setup.py` header + `:63-200`).
- **Picker** `[CODE]`: single-file dark web UI; per-agent dropdown fed by the
  *live* hop `/health` route table (falls back to a baked catalog offline);
  per-row "test" pill does a real 25s `chat/completions` ping *from this
  machine*; save writes `model-bindings.json` atomically (`.tmp`+`replace`)
  and, if `BOX_RELAY_URL` set, pushes to the box (`model-picker.py:180-235`).
- **"Let an AI do it"** `[CODE]`: the picker emits a paste-ready prompt
  (`AI_PROMPT`, `model-picker.py:36-40`) describing the bindings schema +
  invariants ("no API keys ever | unknown wire behavior ⇒ omit parameters,
  never guess | xAI effort=xhigh-not-max | GLM effort=max-literal") — a
  self-describing config format designed for agent editing.
- Feels-alive factors: detect-first onboarding, adopted-not-overwritten
  config, live-vs-catalog badge on the picker, doctor that "tells you exactly
  what moved" after an update.

## 11. Grok/xAI wire facts (the actual xAI-specific content)

What the repo verified about xAI's API **[CODE/DOC]**:
- `reasoning_effort` ∈ `{low, medium, high (default), xhigh}`; **`xhigh`, not
  `max`, is the top token**; reasoning is **always-on, no off switch** —
  omitting the field is the correct "default" (`provider-maps.cjs:11-12,43-49`;
  "Source: docs.x.ai verified 2026-08-22" — doc-verified, not captured).
- Harness mapping: `maxMode:true→xhigh`; `fast:true→low` (overrides effort);
  `thinking` param is a no-op (never emit `none`); `context:"1m"` is a client
  display hint with no wire field (`provider-maps.cjs:43-49`,
  `provider-maps-hop.cjs:96-140`).
- Grok route detection: model slug `/^grok[-.]/i` or baseUrl `:18779` (the
  "grok-shim", `provider-maps.cjs:27-31`, `doctor.py:110`).
- Catalog slugs seen live: `grok-4.6`, `grok-4.6-superheavy`
  (`examples/hop-health-snapshot.example.json`, picker FALLBACK).
- [CAP] From the one real capture (GLM, not Grok): a *bare* request to a
  think-by-default provider burned 78 reasoning tokens on "reply WIRE_OK"
  while `thinking:{type:"disabled"}` truly emitted 0
  (`wire-captures/glm-5.3-flash/capture.json` P1 vs P6) — the generalizable
  lesson: "silence is not cheap; a bare request burns reasoning tokens"
  (README law 3).

### grok-build (all [INF], source missing)
- Rust workspace, codex-rs-architecture TUI: gray's `repl/` modules carry a
  "grok-build architecture sized for gray" provenance header, and past sessions
  name crates `xai-grok-pager`, `xai-grok-markdown/streaming.rs`, plus
  `bottom_pane/chat_composer.rs` / `textarea.rs`-equivalent structure —
  i.e. an interactive terminal coding agent. It was benchmarked Sept-17 at
  `bc7f02e` for shell/background-task/scheduling tooling, but the benchmark
  output lives only in session logs, not this checkout.
- For always-on purposes: nothing recoverable suggests heartbeat/cron/
  proactive features. If the checkout is restored, the interesting diff would
  be what xAI changed vs codex-rs upstream — flag it for re-study.

## 12. Verdict for Gray — ranked steals

1. **Silent-when-clean watchdog with known-warning keys.** `doctor.py --quiet`
   prints nothing when clean; `--init` persists `LEVEL::tag::detail` warn keys
   so only *new* warns alert. → Give the gray gateway cron job a
   `gray doctor --quiet` health pass that notifies the owner channel only on
   deltas — this *is* a proactivity policy in miniature (speak on state change,
   not on schedule).
2. **Baseline-SHA drift tripwire.** Snapshot SHAs of config/plugin/gateway
   files + structured config flags; alert on drift, including targeted
   flip-alerts (`discover_models False→True`). → Baseline `gateway.yaml`,
   `auth.json` *shape* (not values), plugin dir, and plugin index SHAs; a
   self-update or sibling-agent edit becomes a loud event, not a silent
   behavior change.
3. **Identity probes, not port checks.** Probe endpoints that return the
   service's own name/model list; a lookalike listener must not pass. → Gray's
   gateway/plugin health checks should assert identity content (plugin name,
   protocol version), not just TCP-open.
4. **Separate summarization lane with a hard per-turn budget.** Grok Bot's
   `requestKind: summarization|main` + F10's "100k tokens in one turn" +
   "one-compaction-per-turn, fail closed, reset on new user turn". → Gray's
   compaction/memory writes should run as a distinct, capped, tool-stripped
   lane that can never starve the interactive lane.
5. **Fail-closed routing + real error codes.** Bound-route errors must not fall
   through to a default provider; return real upstream codes, retry same-plan
   on blips, cooldown only on persisted errors (F11/F12). → Plugin/channel
   delivery failures should surface as themselves, never silently retried into
   a different path.
6. **`applied` audit trail per control.** `applyHarnessControls` returns
   `{body, route, applied, unknownIds}` including *documented noops with reason
   strings*. → When gray normalizes config (models, channel options), record
   what it mapped, what it ignored, and why — an always-on agent owes its
   owner an auditable "what did you actually do" per turn.
7. **Anchored, idempotent, backed-up mutation.** `count==1` anchors, backup
   dir, syntax-check before+after, re-run = "no changes needed". → Apply to
   gray's plugin installer and any config migration: refuse to half-patch, and
   make re-runs free.
8. **Saved ≠ delivered ≠ consumed ≠ routed — verify the last stage.**
   The picker's own test doesn't prove routing; only a real turn hitting the
   port does. → For gray outbound delivery (Discord/cron notifications), the
   health signal should be *last successful end-to-end delivery*, not
   "config valid" or "queue non-empty".
9. **"Every green needs a proven red."** A detector that never failed is
   indistinguishable from a broken one; both-open auth boundary = worst
   outcome. → Gray's gateway auth/rate-limit tests need negative controls
   that must reject, exercised in CI, plus chaos drills for the watchdog.
10. **Desired-state JSON re-read every turn, agent-editable by design.**
    Bindings are the routing authority, read fresh per turn, with a
    paste-ready AI prompt describing schema+invariants. → Standing orders /
    channel config for an always-on gray should be a small hot-reloaded file
    whose format is documented for agent editing (schema + "never guess"
    invariants), so a wake can re-plan without a restart.

**Explicitly NOT to copy:**
- *Monkeypatching a sealed vendor host* (`apply-box-patch.py`) — gray owns its
  host; never adopt runtime-patching fragility you don't need.
- *Unauthenticated file relay* (`file-relay.py`) — a documented convenience
  hole; gray's control socket already does this properly.
- *Windows-Startup-VBS persistence* — systemd/launchd user units instead
  (opengrok itself scopes VBS to Windows-only).
- *Provider-knob guesswork* — their core law: no wire claim without a capture;
  if gray adds provider controls, probe-verify or ship an honest noop.

**Sources gap note:** grok-build (broken symlink since ≥2026-10-04) and any
real Grok-app captures of tasks/companions/memory were unavailable; sections
2–3 and 6–7 above state where evidence ends rather than extrapolating.
