# Study: Hermes Agent — memory, self-improvement, sessions, subagents, steer, safety, UX

Reference: `/home/vstaln/.hermes/hermes-agent` (Python, ~460 KLOC single `run_agent.py`
plus `agent/`, `tools/`, `hermes_cli/`, `plugins/`, `gateway/`, `ui-tui/`).
Scope: memory & user model, skills self-improvement, session persistence/resume,
subagents, interrupt/steer, unattended approvals, CLI/TUI/onboarding.
Gateway/cron/heartbeat internals are covered by a sibling study; they are touched
here only where they intersect the topics above.

Hermes is a *very* mature codebase (issue numbers in comments run to #99000+).
Almost every mechanism described below exists because a specific incident forced
it; the comments are a goldmine of failure modes for an always-on agent.

---

## 1. Process model (brief)

- One long-lived **gateway process** (`gateway/run.py`, ~11 KLOC observed) runs
  per profile; it hosts all chat channels (Telegram/Discord/Slack/webhook/API)
  in-process. A `SessionDB` (SQLite, WAL) is the shared durable store.
- Each user message builds/reuses an `AIAgent` and calls
  `run_conversation()` on an executor thread — turns are in-process, not
  subprocesses. Subagents fork `AIAgent` in-process too
  (`tools/delegate_tool.py`, `agent/background_review.py`).
- Concurrency is per-thread state: interrupt bits (`tools/interrupt.py`),
  approval session keys (`tools/approval.py:38`), session env
  (`gateway/session_context.py`) are all `ContextVar`/thread-ident keyed so one
  session's stop or approval never leaks into a sibling.
- The TUI is a **separate Node process** (React+Ink) talking JSON-RPC over stdio
  to an embedded `tui_gateway` Python server (`ui-tui/README.md`). The CLI
  (`cli.py`, 22 KLOC) is a prompt_toolkit REPL driving `run_conversation`
  directly.
- Agent init is cheap enough to rebuild per message for the gateway; heavy
  state (memory store, system prompt, session row) is cached on the agent or
  re-derived from SQLite.

## 2–4. Wake sources, proactivity, channels (sibling's area — intersections only)

Things in my scope that touch wakes:

- **Cron sessions skip the self-improvement fork** — `skip_background_review`
  is set for cron because "review forks spawn another AIAgent (~30K tokens /
  event) and cron sessions have no human-in-the-loop benefit"
  (`agent/turn_finalizer.py:800-818`). A real cost/attention tradeoff worth
  copying for Gray's unattended wakes.
- **Machine-generated turns must never become durable user memory.** The Honcho
  provider filters an anchored regex of gateway notification shapes
  (`[ASYNC DELEGATION COMPLETE…`, `[CONTEXT COMPACTION…`, delegation-completion
  fan-out notices) before syncing to the user model
  (`plugins/memory/honcho/__init__.py:33-49`). The comment notes it is
  "deliberately anchored: a human discussing one of these strings mid-message
  is still valid input."
- **Busy-input policy is a first-class config**: `display.busy_input_mode` ∈
  `interrupt` (default) | `queue` | `steer` | `redirect`, per-profile overridable,
  changeable live via `/busy <mode>` (`gateway/run.py:10273-10341`,
  `agent/onboarding.py:40-95`). Queued follow-ups are hard-capped per session
  (`gateway/run.py:10797`).
- **`ESTOP` sentinel**: `hermes pause` writes `$HERMES_HOME/ESTOP`; while it
  exists, cron skips dispatch, kanban workers aren't spawned, and new gateway
  turns get a "Hermes is paused" reply. In-flight work is never killed —
  "pause-new-work, not panic/exit". Checked with a bare `os.stat` every tick;
  corrupt file still counts as engaged (fail-safe)
  (`agent/estop.py:1-60`). Ported from gastown's estop.go — dead simple,
  exactly the kind of thing Gray's gateway should have.

## 5. Sessions & long work

### 5.1 Durable session store

`hermes_state.py` (16 K lines) is the heart: `SessionDB` over SQLite, **WAL
mode** for many-readers/one-writer, **FTS5** full-text index over all messages
(`hermes_state.py:1-15`). Design decisions from the docstring:

- "Compression-triggered session splitting via `parent_session_id` chains" —
  a context compaction doesn't mutate history; it ends the current session row
  and opens a child row whose `parent_session_id` points back. Resumes walk the
  **lineage root-to-tip** (`_session_lineage_root_to_tip`,
  `hermes_state.py:13094+`), so the model-visible history is the compressed
  continuation while the display history can still include ancestors
  (`model_history` vs `display_history` are materialized separately).
- Session **source tagging** (`cli`, `telegram`, `cron`, `subagent`, `kanban`,
  `tool`) drives visibility in search/browse.
- End reasons are a typed vocabulary (`hermes_state_common.py:192-240`):
  - `_RESET_END_REASONS` = `session_reset`, `session_switch`, `idle`, `daily`,
    `suspended`, `resume_pending_expired` — deliberate boundaries; children
    are *fresh* starts (prior content NOT in context).
  - `_RECOVERABLE_END_REASONS` = `agent_close`, `ws_orphan_reap`,
    `superseded_by_resume`, `startup_orphan_reap` — *accidental* ends
    (crashes, dead processes). These sessions are resumable: a startup sweep
    closes orphaned rows at next boot but keeps them resumable.
  - `_AUTOMATIC_END_REASONS` adds `tui_shutdown`, `ws_disconnect`,
    `idle_timeout`, `lru_evict` — "some runtime went away, NOT this
    conversation ended"; a live writer can prove the conversation is still
    alive and clear the stamp.

### 5.2 Resume mechanics

- `hermes chat -c/--resume`, bare `/resume` (prints numbered recent list, next
  bare number selects), `/resume <id|title>` (`cli.py:10124-10601`,
  `hermes_cli/main.py:3068` via `HERMES_TUI_RESUME` for TUI).
- `assert_resume_safe(session_id)` refuses to materialize a lineage over
  `sessions.max_resume_messages` (raises `SessionResumeTooLargeError`;
  `0` disables the guard) (`hermes_state.py:13430-13460`,
  `resolved_max_resume_messages` at :138).
- On resume, the CLI prints a **"Previous Conversation" recap box** —
  configurable: `resume_exchanges` (10), `resume_max_user_chars` (300),
  `resume_max_assistant_chars` (200), `resume_skip_tool_only` (true)
  (`cli.py:495-501`). Cheap, huge perceived-continuity win.
- Messages get `display_kind`/`platform_message_id` stamped **at persist time**
  so a crash mid-turn leaves a correctly typed row and a restart drain can
  dedupe by `has_platform_message_id` (`agent/turn_context.py:797-820`).
- Nudge counters (`_turns_since_memory`, `_iters_since_skill`) are **hydrated
  from persisted history** on resume by counting prior user turns — a resumed
  session's review cadence continues where it left off
  (`agent/turn_context.py:786-794`).

### 5.3 `session_search` — model-callable long-term recall

One tool, four modes inferred from args, **zero LLM calls** — every shape
returns real DB rows (`tools/session_search_tool.py:1-30`):

1. **Discovery** (`query`): FTS5 + dedupe by session lineage. Top hit fully
   hydrated with ±5-message window + bookends; lower hits keep anchor +
   metadata. `detail="full"` hydrates everything.
2. **Scroll** (`session_id` + `around_message_id`): ±window around an anchor;
   re-anchor on edge message ids to page.
3. **Read** (`session_id` alone): whole session or bounded head/tail.
4. **Browse** (no args): recent sessions w/ titles+previews.

Ranking details worth stealing:

- Hidden sources `("kanban","subagent","tool")` are excluded entirely; cron is
  **demoted not excluded** — "cron jobs accumulate large volumes of repetitive
  vocabulary; under bare BM25 they dominate the top-N FTS rows… producing
  'recall blindness' where only cron sessions surface (#19434). Demoting keeps
  cron content reachable when it's the only match"
  (`session_search_tool.py:40-56`).
- Compaction-summary rows (`[CONTEXT COMPACTION`, `[CONTEXT SUMMARY]:`) are
  excluded from bookends so a huge generated payload doesn't get re-injected
  via search (:60-70).
- Scan limit 300 raw FTS rows before lineage dedupe (:58) — enough depth to
  find interactive hits buried under cron walls.

### 5.4 `todo` tool — task list that survives compaction

In-memory `TodoStore` per agent, revisioned for UI staleness; single `todo`
tool (write by passing `todos`, read by omitting). On context compression the
list is re-injected as a synthetic message headed
`"[Your active task list was preserved across context compression]"`
(`tools/todo_tool.py:1-45`). Caps: 256 items, 4000 chars/item, 512 KB replay
budget. It is also the dedupe key for gateway-internal turn filtering.

## 6. Memory & user model

Two coexisting tiers, cleanly separated:

### 6.1 Built-in file memory — MEMORY.md + USER.md

`tools/memory_tool.py` (1394 lines) implements two curated stores under
`$HERMES_HOME/memories/`:

- **MEMORY.md** — "agent's personal notes and observations (environment facts,
  project conventions, tool quirks, things learned)".
- **USER.md** — "what the agent knows about the user (preferences,
  communication style, expectations, workflow habits)"
  (`memory_tool.py:1-20`).

Format rules:

- Entries separated by `\n§\n` (section-sign delimiter — splitting on bare `§`
  alone would break entries containing it; :78).
- **Character limits, not tokens** — "char counts are model-independent":
  `memory_char_limit` default 2200, `user_char_limit` default 1375,
  configurable via `memory.*` config (:924-940). The limit applies to the
  whole file, forcing the model to consolidate.
- **Frozen-snapshot prompt injection**: both files are rendered into the
  system prompt once per session ("MEMORY (your personal notes)" /
  "USER PROFILE (who the user is)" blocks); mid-session writes hit disk
  immediately but do NOT rebuild the prompt — "this preserves the prefix
  cache for the entire session" (:8-13,
  `agent/system_prompt.py:20` puts them in the `volatile` segment).

The `memory` tool itself (`MEMORY_SCHEMA`, memory_tool.py:1260+):

- One tool, actions `add|replace|remove`, target `memory|user`. `replace`/
  `remove` address entries by **short unique substring** (`old_text`), not
  IDs — much friendlier to models.
- **Atomic batch**: `operations: [{action, content?, old_text?}…]` applies in
  one call and "the char limit is checked only on the FINAL result — so a
  single call can remove/replace stale entries to free room AND add new ones,
  even when an add alone would overflow". The schema description literally
  coaches the overflow path ("IF FULL: an add is rejected with the current
  entries shown. Reissue as ONE batch…").
- Description encodes the retention policy: save "user preferences &
  corrections > environment facts > procedures"; SKIP "trivial/obvious info,
  easily re-discovered facts, raw data dumps, task progress, completed-work
  logs, temporary TODO state (use session_search for those). Reusable
  procedures belong in a skill, not memory." — a clean division of labor
  between memory/skills/session-search.
- Tool responses report `current/limit` chars and preview entries so the model
  can self-consolidate.
- Schema is dynamically narrowed per config: if only one store is enabled the
  `target` enum and description shrink (`_build_memory_schema_overrides`).

Durability engineering worth copying wholesale:

- **Atomic writes** via temp-file+rename (never `open("w")+flock` — that
  truncates before the lock and readers can see an empty file)
  (`_write_file`, :887-897).
- **Cross-process lock** on mutation (`fcntl.flock`/`msvcrt`), and
  read-modify-write goes through a *checked* read that refuses to overwrite an
  unreadable file (`_read_raw_checked`).
- **External-drift detection**: before replace/remove/batch, the on-disk file
  must round-trip through the tool's own parser AND have no single entry
  bigger than the file budget. Otherwise a `.bak.<ts>` snapshot is written and
  the mutation is REFUSED with instructions ("Resolve the drift first — either
  rewrite the file as a clean §-delimited list…"). This catches shell appends,
  patch-tool edits, and sister-session writes that flushing would silently
  discard — issue #26045 (:838-880). `add` deliberately skips the drift guard
  (append-only can't lose content).
- **Prompt-injection scan on writes**: memory content goes through the shared
  `tools/threat_patterns.py` scanner at the **strict** scope — rationale:
  "memory enters the system prompt as a FROZEN snapshot, so a poisoned entry
  persists for the entire session and across sessions" (:84-97).

### 6.2 The write gate — `write_approval`

`tools/write_approval.py` stages memory (and skill) writes when
`<subsystem>.write_approval: true`:

- Foreground CLI memory writes prompt inline; everything else (gateway,
  background-review origin, skills) is **staged to disk** at
  `$HERMES_HOME/pending/{memory,skills}/<id>.json` and surfaces via
  `/memory pending` + approve/reject handlers — survives restarts, reviewable
  from CLI/gateway/dashboard (:1-50).
- "There is intentionally no third 'block all writes' state — to disable a
  subsystem entirely use its own enable flag."
- `apply_memory_pending` replays a staged payload directly against the store,
  bypassing the gate (`memory_tool.py:1230+`).

### 6.3 Periodic review nudge → background fork

The built-in path is passive (the model decides to call `memory`); the active
path is a **post-turn background review fork**:

- Counters: `_turns_since_memory` (per user turn, interval default 10 via
  `memory.nudge_interval`) and `_iters_since_skill` (per tool iteration,
  `skills.creation_nudge_interval` default 10)
  (`agent/turn_context.py:841-849`, `agent/agent_init.py:1871,2000-2003`).
- At turn finalization, if either tripped, the agent spawns a **detached
  `AIAgent` fork** replaying the conversation snapshot asking "should anything
  be saved?" (`agent/turn_finalizer.py:786-818`,
  `agent/background_review.py:1-25`). The fork inherits the parent's runtime
  and cached system prompt (same prefix cache), is restricted to a memory/
  skill tool whitelist at dispatch, and its writes flow through the same
  stores + write gate.
- Skipped entirely when `interrupted`, `skip_background_review` (cron), or no
  final response. ~30K tokens per review event — real cost, deliberately
  bounded by cadence.

The review prompts (`agent/background_review.py:465-680`) are the most
stealable artifact in the codebase. The skill prompt tells the fork to be
ACTIVE ("a pass that does nothing is a missed learning opportunity, not a
neutral outcome"), enumerates first-class signals (style/verbosity corrections,
workflow fixes, non-trivial techniques, skills found wrong mid-session),
imposes a preference order (patch loaded skill → patch umbrella → add support
file → create class-level umbrella only), enforces **read-before-write**
("skill_manage refuses otherwise"), and has an explicit anti-list: setup/
config errors, negative claims about tools ("they harden into refusals the
agent cites against itself for months"), transient errors, one-off narratives,
and — my favorite — "Unresolved failures: if the session ended WITHOUT
actually finding a working method… do NOT write those attempts up as a
'reliable workflow'… never the dead ends, and never dressed up as best
practice" (:615-680).

### 6.4 External memory providers — pluggable user modeling

`agent/memory_provider.py` defines the `MemoryProvider` ABC;
`agent/memory_manager.py` orchestrates. Exactly **one external provider** at a
time ("prevents tool schema bloat and conflicting memory backends",
`memory_manager.py:5-7`). Lifecycle per session (`memory_provider.py:14-32`):

```
initialize()            — connect, warm up
system_prompt_block()   — static text (cache-friendly)
prefetch(query)         — recall BEFORE each turn, injected into context
sync_turn(user, asst)   — async write AFTER each turn
get_tool_schemas()      — provider tools exposed to the model
handle_tool_call()
Optional: on_turn_start, on_session_end, on_session_switch,
          on_pre_compress (v2 = fail-closed checkpoint before compaction),
          on_memory_write (mirror built-in writes), on_delegation,
          backup_paths()
```

Cross-cutting details:

- **Trivial-prompt gate**: a shared anchored regex
  (`TRIVIAL_PROMPT_RE`, `memory_provider.py:74-105`) skips prefetch for "hi /
  ok / thanks / lgtm" — "saving a blocking network round-trip and preventing
  stale user-model context from derailing one-word replies". One shared
  classifier so core and providers can't drift.
- Prefetch runs on a background thread per turn; writes are submitted to a
  daemon executor tracked by durability class; `shutdown_all` drains with a 5 s
  cap — "a wedged provider must never block process teardown"
  (`memory_manager.py:74-77,804-878`).
- `on_pre_compress` checkpoint API v2 is opt-in **fail-closed**: providers that
  durably checkpoint get a normalized evidence handoff and strict-mode failure
  propagation (:41-46); signature introspection keeps legacy v1 providers
  working (`memory_manager.py:45-70`).
- Tool schemas from providers are normalized through `normalize_tool_schema`
  because one provider returned already-wrapped OpenAI tools and double-
  wrapping made strict providers reject the whole request — "one bad schema
  disables the whole toolset" (:84-114).
- A per-turn **recall indicator** ("🧠 N memories" or provider glyph) is
  emitted deterministically from `RecallStatus` — model-independent proof
  memory was used (:52-70).

### 6.5 Honcho plugin — hosted user model (dialectic)

`plugins/memory/honcho/` (~5.8 KLOC) is the most developed provider
(Hindsight/Mem0/Byterover/OpenViking/etc. also exist). Concepts:

- **Peers & cards**: user and ai are "peers"; a peer *card* is a short curated
  fact list (cheapest call: no query, no LLM); *representation* is Honcho's
  synthesized user model; *dialectic* is a queryable LLM view over it.
- Three **recall modes** (`system_prompt_block`, :679-720):
  `context` (auto-inject only, no tools), `tools` (5 tools: honcho_profile /
  _search / _reasoning / _context / _conclude, no auto-inject),
  `hybrid` (both).
- **Two-layer prefetch** (:722-903): Layer 1 = cached base context
  (representation + card) refreshed on `context_cadence`; Layer 2 = a
  **dialectic supplement** refreshed on `dialectic_cadence`. Only turn 1 may
  block briefly on either (`_FIRST_TURN_BASE_TIMEOUT`,
  `_FIRST_TURN_DIALECTIC_CAP`); later turns *consume whatever is already
  ready* and never wait — in-flight results surface next turn.
- **Multi-pass dialectic** (`_run_dialectic_depth`, :1229-1296): up to
  `dialecticDepth` `.chat()` calls — pass 0 cold ("Who is this person?") or
  warm ("Given what's been discussed in this session so far…"), pass 1
  self-audit ("What gaps remain…"), pass 2 reconciliation ("Do these
  assessments cohere?"). Each pass is conditional: `_signal_sufficient`
  (>100 chars + structure, or >300 chars) bails early. Per-pass reasoning
  levels come from an explicit `dialecticDepthLevels` list, a proportional
  table, or a base level auto-bumped by query length (+1 at ≥120 chars, +2 at
  ≥400 — `_apply_reasoning_heuristic`).
- **Empty-streak backoff**: cadence widens by the consecutive-empty count,
  capped at `_BACKOFF_MAX`×base; auth failures exempt (:1090-1111). Pending
  results older than `cadence × _STALE_RESULT_MULTIPLIER` are discarded as no
  longer tracking the conversational pivot (:925-947).
- **Injection budget**: `context_tokens × 4` chars, truncated at a word
  boundary (:947-960).
- **Failure UX**: if auth dies, a one-time model-facing notice is injected —
  "[Honcho memory status] Authentication … has expired … Tell the user (once)
  that Honcho memory is paused and that running 'hermes honcho setup' will
  restore it" (:905-923). Memory degrades into a *user-visible, actionable*
  state instead of silently vanishing.
- `sync_turn` chunks messages to the provider's limit and persists in a
  daemon thread (never blocks the turn); `_INTERNAL_GATEWAY_TURN_RE` keeps
  machinery notifications out of the user model (:1300-1500).
- `liveness_snapshot()` exposes cadence/streak/thread-age for diagnostics
  (:1113-1131).

## 7. Goals / standing orders / skills as the durable layer

- `todo` (above) is the session-scoped list; **skills are the durable one**.
- `/learn` builds ONE prompt that points the live agent at whatever the user
  describes (dir, URL, "what I just did", pasted notes) and has it author a
  skill via `skill_manage` — "There is no separate distillation engine"
  (`agent/learn_prompt.py:1-30`). The prompt embeds the maintainer's
  HARDLINE authoring standards: description ≤60 chars ("the system-prompt
  skill index truncates to 60 chars and loads it every session, so anything
  past char 60 is silently cut and never routes"), `author: Hermes` always
  ("an environment-derived name is a privacy leak"), fixed section order,
  hermes-tool framing (:33-120).
- **Skill index in the system prompt**: `build_skills_system_prompt`
  (`agent/prompt_builder.py:1763-1900`) renders a name+≤60-char-description
  index grouped by category, from `~/.hermes/skills/` plus external (read-only)
  and trusted project-local dirs (`./.hermes/skills`, `./.agents/skills` at git
  root — project skills win). Two-layer cache: in-process LRU + a disk
  snapshot validated by mtime/size manifest. `skills_list`/`skill_view` lazy-
  load full SKILL.md and linked `references/|templates/|scripts/` files —
  the index is always in-prompt, the bodies on demand. Focus/coding mode can
  demote categories to names-only but **never hides** a skill.
- **Kanban** exists (`hermes_cli/kanban_db.py`, `tools/kanban_tools.py`) for
  dispatcher-driven board work; kanban workers get a turn-end guard forcing a
  terminal `kanban_complete|kanban_block` call before the run may finish
  (`agent/kanban_stop.py:1-40`).

### 7.1 Skill lifecycle & the curator

- `tools/skill_usage.py` tracks per-skill sidecar state: `use_count`,
  `last_used_at`, `last_activity_at`, `agent_created`, `pinned`, lifecycle
  state `active|stale|archived` (:1-60, 601-1017). Emits `on_skill_lifecycle`
  hooks for observers.
- `agent/curator.py` is the **self-maintenance loop**: inactivity-triggered
  (idle ≥ `min_idle_hours` 2 AND ≥ `interval_hours` 7 days since last run —
  no cron daemon needed; :57-66). It (a) applies deterministic transitions —
  stale after 30 d, archive after 90 d, pinned exempt — and (b) optionally
  spawns an aux-model review fork that can pin/archive/consolidate/patch
  *agent-created* skills via `skill_manage`. Invariants: "Never auto-deletes —
  only archives. Archive is recoverable. Pinned skills bypass all
  auto-transitions. Uses the auxiliary client; never touches the main
  session's prompt cache" (:10-26). Consolidation (LLM umbrella-building) is
  **off by default**; the deterministic prune always runs.
- `skill_manage` hardening (`tools/skill_manager_tool.py`): name/category/
  frontmatter validation, **security scan on write** (`_security_scan_skill`,
  :145), path-redirect and delete-target validation, `_pinned_guard`,
  read-before-write enforcement for the background-review origin, write-gate
  staging, batch ops with rollback, lint findings attached to results.
- `skill_ledger.py`, `skills_sync.py`, `skills_hub.py` cover provenance +
  hub sync — out of scope but notable: skills are a *published artifact*, so
  the privacy rule (`author: Hermes`) is load-bearing.

## 8. Reliability details observed

- Message rows are durable **per turn, early** — user row persisted before the
  model call ("early crash-resilience persist"), with `display_kind` and
  `platform_message_id` stamped up front (`agent/turn_context.py:797-820`).
- Interrupted assistant text is preserved as a **scaffold**, not dropped:
  `"[This response was interrupted by a user correction.]"` + a
  `"Visible response before the interruption:"` header exists "so the MODEL
  sees the interrupted context" while transcripts can render it neutrally
  (`agent/conversation_loop.py:118,453-530`).
- Synthetic nudges (verify-on-stop, dropped-toolcall recovery, `(empty)`
  recovery) are stripped from model history on return so a resumed session
  doesn't replay scaffolding as real user turns
  (`run_agent.py:273-299`, `agent/turn_finalizer.py:58-120,305-312`).
- Compression commit fence + interrupt generation claims: `interrupt(
  require_generation=…)` reserves an abort against the turn's liveness clock
  and only publishes if the turn hasn't resumed in between — a stale `/stop`
  can't hard-cancel a live turn (`run_agent.py:3400-3470`).
- `SessionDB` read connections are budgeted per-process and per-path with fd
  headroom checks and idle-conn reclaim (`hermes_state.py:408-660`) — the
  gateway shares one process with everything.
- `kanban_stop.py`: workers that stop without the terminal board tool get a
  bounded synthetic nudge (2 attempts) instead of a silent `rc=0` →
  `protocol_violation`.

## 9. Safety for unattended action

`tools/approval.py` (~6 KLOC) is the dangerous-command system — single source
of truth for pattern detection, per-session approval state, CLI+gateway
prompts, smart approval, and the permanent allowlist. Unattended-relevant
machinery:

- **Context detection** decides what "ask the human" even means
  (:252-330): cron (`HERMES_CRON_SESSION`), single-query `-q`
  (`HERMES_SINGLE_QUERY_SESSION`), gateway platforms with a human surface, and
  `_UNATTENDED_APPROVAL_PLATFORMS = {webhook, msgraph_webhook, api_server}` —
  "no human is on the other end to answer an approval prompt… Treating them
  as gateway approval contexts blocks the session for the full approval
  timeout (60-300 s) and then fails closed anyway — the deadlock in
  #37284/#87509".
- **Per-context modes**: `approvals.cron_mode`, `approvals.single_query_mode`,
  `approvals.unattended_mode` — each `deny` (default) or `approve`. Deny
  returns a tool-visible message telling the agent how the user can unblock
  ("set approvals.unattended_mode: approve"), not a hang (:3560-3580,
  3840-3890).
- **Deny rules fire before YOLO**: `approvals.deny` patterns and the hardline
  floor are evaluated *before* the yolo/mode=off bypass — "a deny rule is the
  user's explicit veto" (:4120-4130, 4769-4780). `HERMES_YOLO_MODE` is frozen
  at module import "so a skill running inside the process cannot set the env
  var and instantly bypass all approval checks — a prompt-injection
  escalation path" (:29-33).
- **Hardline commands** (a floor that always prompts), user deny rules, a
  real shell parser (segments, exec-flag detection for `bash -c`, `python -c`,
  `$()` spans, parser-limit fail-closed) rather than regex-only matching
  (:644-2060).
- **Gateway async approvals**: pending approvals keyed by session_key with
  notify callbacks, `/approve` `/deny` resolution, timeouts resolving as
  deny — "Fail-closed deny preserves #8697 semantics" (:2812-2930,
  4463-4720).
- **Smart approval**: an auxiliary-LLM verdict (`approve`/`deny`/`escalate`)
  can auto-approve low-risk commands; observer hooks record verdicts
  (:141-190, 3644+).
- Memory/skill writes have their own gate (`write_approval`, §6.2) —
  the background-review fork's autonomous writes are stageable for exactly
  this reason ("the source of the 'wrong assumptions' users complained
  about", `tools/write_approval.py:20`).
- Delegation has its own kill switch: `is_spawn_paused()` refuses new
  fan-outs without killing running children (`delegate_tool.py:3860-3870`).

## 10. UX: CLI, TUI, onboarding

- **Subcommand surface** (`hermes_cli/main.py:471-515`): chat, gateway, cron,
  sync, profile, model, setup, whatsapp, slack, login/logout, auth, status,
  pause, webhook, hooks, doctor, verify, security, approvals, dump, debug,
  backup, import, import-agent, config, skin, console, update, uninstall,
  dashboard, gui, logs, prompt-size, memory, acp, tools, insights,
  monitoring, skills, pairing, plugins, mcp, claw (OpenClaw migration).
- **Interactive CLI** (`cli.py`): prompt_toolkit REPL; `/resume` session
  picker with numbered list + bare-number follow-up; recap box on resume;
  `/steer`, `/stop`, `/busy`, `/btw`, `/learn`, `/memory`, `/verbose`,
  `/loop`. Quiet one-shot `-q/-Q` paths share resume-or-create logic
  (`cli.py:1377-1410`).
- **TUI** (`ui-tui/`): React + Ink TypeScript app spawning
  `python -m tui_gateway.entry` and speaking newline-delimited JSON-RPC over
  stdio; stderr goes to a log ring, malformed stdout = protocol noise, never
  raw terminal writes (ui-tui/README.md). Session control RPCs include
  `delegation.pause`, `/agents` view with `p` pause hotkey.
- **Onboarding = contextual one-time hints**, not a wizard
  (`agent/onboarding.py:1-15`): each flag (`busy_input_prompt`,
  `tool_progress_prompt`, `openclaw_residue_cleanup`,
  `profile_build_offered`) shown once per install under
  `onboarding.seen.<flag>` in config.yaml. The busy-input hint text *names the
  mode that was just applied* and how to change it — e.g. "💡 First-time tip —
  I steered your message into the current run; it will arrive after the next
  tool call instead of interrupting. Send `/busy interrupt` or `/busy queue`
  to change this". Teaches the steering model exactly at the teachable moment.
- **Feels-alive touches**: a 🧠 recall indicator when memory injected;
  affection "reaction" detection (ily / <3 / good bot → hearts callback)
  that's "token-free, never touches the conversation, never fatal — a purely
  optional UI beat" (`agent/turn_context.py:851-863`); session titles
  auto-generated (`agent/title_generator.py`) and threaded to providers;
  `/insights` usage analytics (`agent/insights.py`); `/btw` side questions.

## 11. Subagents & delegation

Two layers: the model-facing `delegate_task` tool (`tools/delegate_tool.py`,
5.2 KLOC) and a public plugin contract (`agent/subagent_lifecycle.py`).

- **Spawn shapes**: single `goal`+`context`, or `tasks[]` batch — one subagent
  each, run in parallel up to `delegation.max_concurrent_children` (one cap
  governs both sync batches and background units; :903-985). Batch goals
  under a minimum length are rejected as probable unexpanded templates, and
  literal template markers in goals are rejected outright (:3787-3810).
- **Isolation contract**: "each gets its own conversation, terminal session,
  and toolset, and only its final summary returns to you" — children know
  *nothing* of the parent conversation; everything goes through `context`.
  Schema text teaches the trust model: "Child summaries are SELF-REPORTS,
  not verified facts … For external side effects, require a verifiable
  handle (URL, ID, absolute path) and verify it yourself" (:4957-5030).
- **Background mode** (`background:true`): the whole fan-out runs on a daemon
  executor, dispatch returns immediately with live transcript paths, and one
  consolidated completion message re-enters the conversation when ALL
  children finish — "Do NOT wait or poll; continue other work" (:4318-4560).
  Completion rides a shared `completion_queue` (:158).
- **Control plane**: same tool, `action=list|steer|stop` — steer queues
  course-correction text into a running child (a steer landing after
  completion is captured as `missed_steer` in the completion entry rather
  than lost, :366-368); stop interrupts it. A process-wide
  `is_spawn_paused()` kill switch freezes NEW fan-outs without touching
  running children (:3860-3870).
- **Nesting**: `delegation.max_spawn_depth` (default 2) bounds orchestrator
  children spawning their own workers; capability is depth-derived, not
  caller-declared (legacy `role` param is ignored). Children are hard-
  blocked from `delegate_task` (at depth cap), `clarify`, `memory`, and
  `cronjob` tools (:3834-3900).
- **`output_schema`**: children can be asked for structured results;
  `SubagentResult` carries `structured_payload` plus a `result_hash`,
  `error_classification`, usage metadata, and a tool-execution summary
  (`subagent_lifecycle.py:110-125`).
- **Lifecycle states** (`subagent_lifecycle.py:34-46`):
  PENDING→STARTING→RUNNING→{SUCCEEDED|FAILED|INTERRUPTED|CANCELLED},
  CANCEL_REQUESTED in between; terminal records retained 1 h
  (`_TERMINAL_RETENTION_SECONDS = 3600`). Immutable dataclasses only — the
  contract "deliberately exposes immutable contracts, not AIAgent objects".
- **Worktree isolation** (opt-in, `delegation.worktree_isolation`): each child
  gets its own git worktree branched from parent HEAD under
  `<repo>/.worktrees/subagent-<id>`; children commit there, results report
  branch/commit-count/dirty state for the parent to review/merge; clean
  worktrees auto-prune — "pruning requires affirmative proof: if a git
  inspection probe fails the state is unknown, so the worktree is kept"
  (`tools/subagent_worktree.py:1-40`). Skipped silently on non-git or remote
  terminal backends.
- **Timeouts**: per-child `timeout_seconds`; a global spawn pause; a
  delegation live-log file (`delegation_live_log.py`) for operator
  visibility.

## 12. Interrupt / steer / redirect — the mid-turn control plane

This is the best-engineered part for an always-on agent; map it carefully:

- **Thread-scoped interrupt bits** (`tools/interrupt.py`): a set of interrupted
  thread idents; tools poll `is_interrupted()` and bail (`returncode:130`).
  The gateway can't kill another session's tools because each thread sees only
  its own bit. Optional per-thread *reason* exposed in tool output.
- **Three distinct user actions**, all in `run_agent.py`:
  1. `interrupt(message)` / `hard_interrupt` — sets `_interrupt_requested`;
     the tool loop checks it every iteration and breaks
     (`conversation_loop.py:2174-2185`), tools self-terminate via the thread
     bit. `hard_cancel=True` additionally signals the compression fence.
  2. `steer(text)` — appends text to `_pending_steer`; **never interrupts
     anything**. At the next tool-result boundary the text is appended to the
     newest tool message inside a marker:
     `[OUT-OF-BAND USER MESSAGE …] … [/OUT-OF-BAND USER MESSAGE]`
     (`prompt_builder.py:675-700`). A pre-API drain also checks before each
     model call so a steer sent mid-API-call lands on the very next iteration
     (`conversation_loop.py:2246-2300`). The system prompt carries
     `STEER_CHANNEL_NOTE` teaching the model the marker is the ONLY trusted
     steer shape and carries full user authority (models previously refused
     steers as prompt injection — #40240). Cache-safe: it mutates a tool
     message, not the system prompt or history shape.
  3. `redirect(text)` — the middle option: during a model request it cancels
     ONLY that request, keeps completed messages/tool results, records the
     partial reasoning as context, appends the correction as a real user
     message, and retries the turn; during tool execution it **degrades to
     steer()** — "Never kill a tool merely to deliver conversational
     guidance" (`run_agent.py:3820-3900`). Codex backend uses native
     `turn/steer`.
- Subagent steering rides the same idea: `delegate_task action="steer"` queues
  text into a running child; `action="stop"` interrupts it; `action="list"`
  shows live children (`delegate_tool.py:3834-3850`).
- `/btw` side question: answered by a **cache-parity fork** replaying the
  parent's message snapshot with tools denied — the live conversation is
  never touched ("no synthetic turns, no role-alternation risk, no
  prompt-cache invalidation"); falls back to a bounded one-shot digest
  (`agent/side_question.py:1-50`).

## 13. Verdict for Gray — ranked steals

1. **Frozen-snapshot curated memory files with a §-delimited budgeted store
   and a single add/replace/remove tool.** Char budgets (not tokens), batch
   ops with final-state limit check, substring targeting, atomic writes +
   drift-refusal. Gray can implement this in ~a day and it beats any
   embeddings-first design for trust. `tools/memory_tool.py` is a blueprint.
2. **Post-turn background review fork.** Cache-parity child with a memory/
   skill-only tool whitelist asking "should anything be saved?", triggered by
   cheap counters (every 10 user turns / 10 tool iters), suppressed for cron
   and interrupted turns. The review prompt's anti-list (no unresolved
   failures, no "tool is broken" claims, read-before-write) should be copied
   nearly verbatim.
3. **Session search as a zero-LLM tool over SQLite FTS5** with the four-mode
   shape (discover/scroll/read/browse), source demotion for automation noise,
   and compaction-payload exclusion. Gray already has SQLite sessions;
   the demote-not-exclude rule for cron is the subtle win.
4. **busy_input_mode as a user-visible policy** (interrupt/queue/steer/
   redirect) + the OUT-OF-BAND steer marker appended to tool results.
   Redirect-degrades-to-steer ("never kill a tool for guidance") is the
   correctness rule. Cheap to implement, transformative for always-on use.
5. **Per-context approval policy**: `cron_mode`/`unattended_mode`/
   `single_query_mode` default deny with an actionable error string, deny
   rules evaluated before any bypass flag, YOLO frozen at import. Gray's
   unattended runs need exactly this "no human → deterministic decision"
   matrix.
6. **Session lineage chains**: compression and resets create child session
   rows linked by `parent_session_id` rather than mutating history; resume
   walks root→tip; end-reason vocabulary distinguishes deliberate resets
   from recoverable accidents (crash/orphan rows stay resumable).
7. **Curator-style skill lifecycle**: deterministic timestamps →
   stale(30d)/archive(90d) transitions, pinned exempt, archive-only never
   delete, optional LLM consolidation, idle-triggered so no daemon needed.
8. **Provider lifecycle contract for external memory**: prefetch before /
   sync after with trivial-prompt gating, cadence + empty-streak backoff +
   stale-result discard, turn-1-only blocking, budget truncation, and a
   fail-visible auth notice injected to the model.
9. **`/btw` side-question fork** — cache-parity replay answering questions
   about the live conversation without touching history or cache.
10. **One-time contextual onboarding hints** keyed in config — teaches
    steer/queue/verbose at the exact moment it's relevant; trivial to build.
11. **ESTOP sentinel file** — `gray pause`/`gray resume` as an `os.stat` check
    consulted by every dispatch path; corrupt-file-fail-safe. ~50 lines.

Explicitly NOT to copy:

- **The mega-files**: `run_agent.py` at 464 KB / `conversation_loop.py` at
  9 KLOC / `hermes_state.py` at 16 KLOC are archaeology; Gray's Rust codebase
  should implement these ideas in small modules, not replicate the shape.
- **`memory`-tool-in-every-subagent? No** — Hermes explicitly *blocks* memory,
  clarify, and cronjob in delegation children (`delegate_tool.py` schema
  text). Worth copying the restriction, not the opposite.
- **The sheer config surface**: dozens of `approvals.*`, `memory.*`,
  `delegation.*`, `display.*` knobs accumulated incidentally. Pick the five
  that matter (busy mode, unattended mode, nudge intervals, char budgets).
- **Regex+heuristic danger detection as sole gate** — Hermes itself had to
  grow a real shell parser after bypasses; Gray should start parser-first.
