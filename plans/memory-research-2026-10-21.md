# Agent memory — research & gray-memory redesign

*2026-10-21. Continuation: a previous run died on broken Claude auth before
writing anything; no partial draft existed. This file is the full deliverable.*

**Inputs read.** `~/gray/papers/` (prior sweep: `README.md`,
`RESEARCH-2026-09-22.md` — compaction/context findings, not redone),
`~/gray/arxiv-mega/SOURCES.md` (85-paper annotated corpus, esp. the Memory
section lines 1055-1314), `~/Downloads/2604.03136v4.pdf` (StoryScope — AI-fiction
detection, *not relevant*, listed as input by mistake),
`~/Downloads/supermemory.txt` (**0 bytes — empty file**, refetched via web),
`~/grayplugins/gray-memory` source + git history, sibling plugins
(gray-mem0, gray-learn, gray-hoarder, gray-memory-wiki), host wiring in
`~/gray/crates/{gray,gray-core,gray-plugin}`, and the live store at
`~/.gray/memory/`. The parallel e2e report (`~/.cache/essentials-e2e/reports/
memory.md`) had **not landed** as of writing — reports/ exists but is empty;
reconcile when it appears.

**New papers downloaded** into `~/gray/papers/` (+`txt/`): 2504.19413 (Mem0),
2501.13956 (Zep), 2507.05257 (MemoryAgentBench), 2410.10813 (LongMemEval),
2304.03442 (Generative Agents), 2502.12110 (A-MEM), 2305.10250 (MemoryBank),
2504.13171 (Sleep-time Compute).

---

## 1. Annotated bibliography

### 1.1 The central paper — instruction-file accretion

**arXiv:2608.11095 — "Why Does CLAUDE.md Keep Growing? Catastrophic
Remembering in Agentic Coding"** (Kushal Chakrabarti, Aug 2026; on disk:
`~/gray/papers/2608.11095.pdf`, `txt/2608.11095.txt`). *This is the paper the
user remembered.* Full treatment in section 2.
Claim: agentic instruction files grow without bound because appending is O(1)
while safe deletion is O(2^|D|) once an instruction's rationale is forgotten.
Evidence: **strong** — 247,694 instruction lifetimes across 1,867 repos
(observational), plus a causal arm (inverted-IFEval synthetic worlds, 184
worlds, bootstrap CIs) and a real-world arm (WildIFEval, +23.1%
instruction-following with rationale comments).
Take for gray: the `Why:`/`falsified:` contract already in the ingest guard is
exactly this paper's prescription; the remaining gap is that nothing
*deletes* — see section 3.2/FM-4.

### 1.2 Benchmarks (what to measure against)

- **LoCoMo** — Maharana et al. 2024 (arXiv:2402.17753; ACL 2024). ~300-turn,
  ~9k-token, up-to-35-session persona-grounded dialogues; QA (single-hop,
  multi-hop, temporal, adversarial), event summarization, dialog generation.
  Evidence: strong as a *conversational* yardstick; weak for a coding agent —
  its units are chat facts, not repo decisions.
- **LongMemEval** (arXiv:2410.10813, ICLR 2025; on disk). 500 curated
  questions over 115k-1.5M-token chat histories; five abilities: extraction,
  multi-session reasoning, **temporal reasoning, knowledge updates,
  abstention**. Findings: commercial assistants drop ~30% on sustained
  interaction; long-context LLMs drop 30-60%. Design results that matter:
  session-level indexing +9.4% recall@k / +5.4% QA accuracy; explicit
  time-aware indexing + query expansion +6.8-11.3% on temporal questions;
  abstention-aware prompting up to +10 pts. Evidence: strong (peer-reviewed,
  public data). Take: **stamp dates where the model can see them** and treat
  "don't claim what was never saved" as a scored behavior.
- **MemoryAgentBench** (arXiv:2507.05257, ICLR 2026; on disk). Incremental
  multi-turn ingestion; four competencies: Accurate Retrieval, Test-Time
  Learning, Long-Range Understanding, **Conflict Resolution / Selective
  Forgetting**. Headline findings: RAG-style agents win AR; *long-context
  models* win TTL and LRU (retrieval of partial chunks can't substitute for
  holistic context); **all methods fail multi-hop selective forgetting (<=28%
  accuracy)** — even reasoning models don't rescue it. Evidence: strong.
  Take: forgetting can't be a prompt convention; it must be *structural*
  (supersede links, invalidation), because asking a model to resolve
  contradictions freehand is the demonstrated failure case.
- **Evo-Memory** (arXiv:2511.20857; on disk, annotated in SOURCES.md).
  Streaming task sequences; >10 memory modules unified. Key negative result:
  *naive self-generated memory can hurt* — growth without outcome signal rots
  the store. Take: record outcome evidence, not flattering summaries (see also
  Feedback-Loops 2402.06627 already in corpus).
- Also found but not downloaded: **LoCoMo-Plus** (arXiv:2602.10715 —
  cue-trigger semantic disconnect / implicit constraints),
  **MemoryArena** (interdependent multi-session), **BEAM**. Worth a line in
  the eval section only; LoCoMo/LongMemEval/MemoryAgentBench suffice for the
  redesign bar.

### 1.3 Production memory systems (engineering evidence)

- **Mem0** (arXiv:2504.19413, ECAI 2025; on disk). The canonical write
  pipeline: LLM extracts candidate facts from (summary + last m=10 messages +
  new pair), retrieves similar memories, then picks an op —
  **ADD / UPDATE / DELETE / NOOP**. Results: competitive with/above RAG
  variants on LoCoMo, **91% lower p95 latency, >90% token savings** vs
  full-context; Mem0g (graph variant) +~2%. Evidence: moderate (vendor paper,
  but peer-reviewed and replicated since). Take: the op vocabulary is
  the interesting part — gray's ingest allows ADD and append-only UPDATE but
  has no model-proposable DELETE; see 4.2.
- **Zep / Graphiti** (arXiv:2501.13956; on disk). **Bi-temporal knowledge
  graph**: every fact carries *valid time* (true in the world) and *ingestion
  time* (when learned); contradictions are handled by **invalidating edges,
  not deleting** — history is non-lossy. Beats MemGPT on DMR; ~18% better on
  multi-session reasoning; ~90% lower memory latency. Evidence: moderate
  (vendor paper, strong industry adoption of Graphiti). Take: gray's
  append-only `ingest_edit` (`was: <old>` blobs, store.rs:349) is a
  half-measure of the same idea — upgrade to a real `superseded_by` field
  (4.1).
- **Letta / MemGPT** (arXiv:2310.08560 in corpus; letta.com docs +
  blog/memory-blocks). **Memory blocks**: labeled, size-capped sections that
  live *always in context* (no retrieval), agent-edited via
  `memory_insert`/`memory_replace`/`memory_rethink`; archival memory +
  conversation search sit out-of-context behind tools. The crucial inversion
  vs gray: **the model pages memory in/out deliberately via tools; the harness
  doesn't guess**. Take: gray already has the always-inject block (the <=4KB
  profile); what's missing is the agent's *read* lane into the overflow (4.4).
- **Sleep-time Compute** (arXiv:2504.13171, Letta authors; on disk). Offline
  "thinking" about a context before queries arrive: reorganization done in
  the gap, not in the turn; cuts per-query cost 2.5x and can *raise* accuracy
  up to 13% on multi-query workloads. Evidence: moderate. Take: the removed
  cadenced distill was the right *idea* (background consolidation) at the
  wrong cadence/cost — see 4.2.
- **LangMem** (LangChain, docs.langchain.com/oss/python/concepts/memory).
  Industry-standard answer to *when* writes happen: **hot path** (agent calls
  a tool mid-conversation) **+ background** (async manager extracts/consolidates
  after turns). Explicit tradeoff framing: hot path is immediate but depends
  on the model remembering to call; background is reliable but adds a pipeline
  and a cadence question. Take: gray had exactly this dual and just removed
  the background leg (commit b0ce0c7) — the literature says run both.
- **supermemory** (supermemory.ai/docs + blog/memory-engine; the local
  `supermemory.txt` was empty). Extraction service + per-user profiles +
  automatic contradiction handling + **expiry/decay ("smart forgetting")**,
  on a temporal vector-graph; profile() endpoint = the always-inject surface,
  search.memories() = the retrieval surface. No paper, evidence weak-to-moderate
  (vendor claims, #1 claims on LongMemEval/LoCoMo/ConvoMem unverified). Take:
  profile vs search is the same two-tier split recommended in 4.4.
- **Anthropic, "Effective context engineering for AI agents"** (Sep 2025).
  Memory framed as *structured note-taking outside the window, pulled back
  later*; the governing principle is "smallest set of high-signal tokens".
  Validates gray's capped-summary injection and the `[Context update]`
  user-injected delivery.
- **aihero.dev AGENTS.md guide / xtrace blog / developersdigest coverage** —
  practitioner consensus corroborating 2608.11095: instruction budget
  ~150-200 followable instructions for frontier models, position bias toward
  earlier instructions, wholesale-rewrite treadmill. Evidence: weak
  (blog consensus), directionally consistent with the paper.

### 1.4 Mechanism papers already on disk (built on, not redone)

From `arxiv-mega/SOURCES.md` Memory + Context sections and `papers/RESEARCH`:

- **MemGPT** (2310.08560): paging is *the model's* decision via tools.
- **Governance Decay** (2606.22528): compaction drops standing constraints
  (0%->30-59% violation); fix = pin them, ~47 tokens. *Memory is a standing
  constraint — it must survive compaction verbatim.* gray already pins
  post-compaction via the frozen snapshot + re-injected `[Context update]`.
- **Bad Memory** (2607.14611) + **AgentPoison** (2407.12784) +
  **Remembering More, Risking More** (2605.17830): memory is a persistent
  attack surface; provenance + bounds + treat-as-untrusted are security
  controls. gray's provenance trailers, add-only tool surface, and daily caps
  are all aligned; the gap is no *expiry*.
- **Generative Agents** (2304.03442; downloaded): retrieval score =
  recency x importance x relevance — the classic scoring triple.
- **MemoryBank** (2305.10250; downloaded): Ebbinghaus decay on stored
  memories — strength decays unless *recalled*. Take: decay should be driven
  by use, not wall-clock.
- **A-MEM** (2502.12110; downloaded): Zettelkasten — atomic notes, LLM-
  generated links, memory evolution rewrites related notes. Take: link fields
  (`supersedes`, `relates`) beat monolithic text blobs.
- **Contextual Experience Replay** (2506.06698, corpus): replay the episode,
  not just the distilled lesson — supports keeping transcript references
  (gray-hoarder/session JSONL) as the backing store for each entry.
- **Reflexion** (2303.11366, corpus): reflections must be generated from a
  *verifiable signal* (test result, error), not self-assessment.
- **From Storage to Experience** (2605.06716) + **Memory for Autonomous LLM
  Agents** (2603.07670, corpus surveys): taxonomy (storage->reflection->
  experience; write-manage-read loop) — the organizing frame for section 4.

---

## 2. The CLAUDE.md-growth finding, explained

**Paper:** arXiv:2608.11095, "Why Does CLAUDE.md Keep Growing? Catastrophic
Remembering in Agentic Coding."

**The phenomenon.** Across 1,867 GitHub repos and 247,694 instruction
lifetimes, agentic context files (CLAUDE.md / AGENTS.md /
copilot-instructions.md) grow monotonically — +226% over a file's lifetime,
+4.9 net instructions per commit. Instructions almost never die individually:
**76.8% of instruction deaths happen in wholesale rewrites**, which cut the
file to 59.5% of its pre-rewrite size — and the file regrows to 91.5% within
ten commits, then grows *faster* than before (4.9%/commit vs 4.1%).

**The mechanism.** The paper's causal claim: appending an instruction costs
O(1) — you write down the rule that just bit you. Deleting one safely costs
O(2^|D|): to know an instruction is dead you must rule out every subset of
the file that depends on it, i.e., re-derive *why it exists*. The deletion
hazard **falls with instruction age** (log-hazard -0.032/commit): the older a
rule, the less likely anyone can afford to remove it. Crucially, age is a
proxy for *lost rationale*, not staleness — staleness-based rival models
predict the opposite slope and are falsified. The name is deliberate:
**catastrophic remembering** is the mirror of catastrophic forgetting —
continual-learning systems forget old knowledge under new training; agentic
prompts *retain* old instructions because forgetting is what's expensive.

**The fix.** Write the rationale next to the instruction, in a form the next
maintainer can use and the model doesn't need to see. In the causal arm
(inverted IFEval worlds with known-optimal prompts), comments encoding latent
reasoning remove **99.3% of excess instructions** (+211.3% -> +1.4% excess
size) at equal correctness. On WildIFEval real prompts, comments improve
instruction-following up to **+23.1%**.

**Why it's central for gray.** gray-memory is a self-editing instruction file
wearing a store's clothing. It already implements the paper's fix: ingest
entries must carry `Why:` + `falsified:` (store.rs:598-606), the audit rule
is verbatim the paper's logic ("if the failure has not recurred since the
entry was added, it is probably preventing that failure — keep it",
store.rs:436-460), and `growth_warning` is the paper's
"Gate-0" ratchet signal (store.rs:493,591). What gray does *not* implement:
the deletion half — the audit is advisory and nothing in the write path can
retire an entry. The live store shows the ratchet mid-flight: the ~/gray
project file is **45 entries / 45.5 KB** written in an 8-day burst
(Sep 22-30) with zero deaths, and the injection cap (4 KB) silently hides
~90% of it (section 3.2).

---

## 3. gray-memory today: design and concrete failure modes

### 3.1 Architecture (as of branch `fix/memory-write`, commit b0ce0c7)

- **Store** — `~/.gray/memory/user.md` + `project-<sha256(git-root)>.md`,
  Markdown `- key: text` lines, HTML-comment provenance trailers
  `<!-- gray:saved=...;source=... -->` that never reach the model
  (store.rs:79-95, 735-764). Non-git cwds hash to the cwd itself
  (store.rs:79-92, `find_git_root(...).unwrap_or(cwd)`).
- **Write lanes** —
  (a) `memory` tool, ops `remember`/`amend` only -> `ingest_set`/`ingest_edit`;
      no set/remove/clear — "Hermes deleted `replace` after models used it to
      wipe entries" (lib.rs:155-164, tool_call ~lib.rs:200-255);
  (b) ~~cadenced distill~~ — **removed today** in b0ce0c7 ("memory writes
      stay inline"); `distill-run`/`distill-log` survive as manual verbs and
      distill.rs still compiles (610 lines of now-mostly-dead machinery:
      TRANSCRIPT_LAST=40, PROMPT_MAX_BYTES=48K, MAX_COMMANDS=8,
      MAX_FAILURES=3 -> `distill.stopped`, scratch-home sandboxing);
  (c) human CLI `gray memory set|edit|remove|clear|audit`.
- **Ingest contract** — entries must contain `Why:` + `falsified:`
  (store.rs:598-606); `ingest_set` never overwrites and refuses verbatim-dup
  text (store.rs:322-347); `ingest_edit` is append-only — new text must
  contain old text verbatim (store.rs:349-375); daily caps 10 total / 2 user
  (store.rs:512-527, 595-596); secrets refused via redact (store.rs:671-684);
  single line, no control/invisible chars.
- **Injection** — `prompt/context` serves MEMORY_POLICY + a **frozen
  per-session snapshot** (snapshots/<sid>.json, project-checked,
  store.rs:140-205) rendered as compact JSON `{user, decisions}` where each
  is `profile()`: first-sentence-of-entry, **newest-first**, capped at
  SNAPSHOT_SCOPE_BYTES = 4 KiB per scope, with a `(+N older entries not
  shown)` notice (store.rs:223-261, 812; lib.rs:155-158, 269-287).
- **Host delivery** — `gray-core/src/agent_loop.rs:476-496`: all hook
  `prompt_context()` texts are concatenated once per user turn and pushed as
  a `user_injected` `[Context update]` message, deduped by exact-match
  `contains` — so the frozen snapshot is also the dedupe key and stays
  byte-identical (prefix-cache friendly, consistent with 2601.06007). Skipped
  entirely for non-global hooks under `--lean` (agent_loop.rs:482-484).
- **Growth telemetry** — `growth.json` per-scope {peak, streak, removals};
  warning at streak >=3 appended to write replies (store.rs:493-501,556-588;
  cli.rs:166-169).

### 3.2 Concrete failure modes

**FM-1 — The only writer left is the model's mood.** b0ce0c7 removed the
guaranteed writer; writes now depend on MEMORY_POLICY persuading the model to
call `remember` unprompted (lib.rs:155-158). The distill log itself shows why
that was built: `2026-10-21T08:48 distill concave-nitrogen-moonbow: 3 added`
— facts the inline lane never captured that session. LangMem's docs state the
tradeoff verbatim: hot-path tools alone under-write. The removal is
reasonable in one respect (the distill spawned a full `gray -p` subagent
every 4 turns — expensive, and it hit real rate-limit failures, see
distill.log 22:50Z) — but the fix should be a cheaper trigger, not deletion
(4.2).

**FM-2 — Injection ranks by recency, not relevance.** `profile()` sorts by
`saved` date desc and packs 4 KiB (store.rs:223-261). A two-month-old "user
prefers vim" is identical in rank to yesterday's transient bench note —
actually *worse*, since old entries drop first. There is no relevance signal:
no per-turn query, no keyword match, no scope weighting beyond the file split.

**FM-3 — The 4 KiB cap hides the accretion it was built to bound.** The live
~/gray store is 45 entries/45.5 KB; the model sees ~the newest 10-15
first-sentences plus "(+N older entries not shown; `gray memory list` prints
them all)". Nothing makes the model call `gray memory list` — the `memory`
tool has no read/search op (lib.rs tool schema exposes only
op/key/text/scope/why). Retrieval exists only via the model deciding to run
`gray memory ...` through bash. Effectively: **write-only memory below the
waterline**.

**FM-4 — Nothing can be deleted except by a human.** By design (and correct
per 2607.14611 threat model), but with no *proposed-deletion* path the
ratchet only moves one way. `audit` detects dup targets + falsified entries
(store.rs:436-491) yet is never invoked automatically — no turn, distill, or
cron calls it. Live store has entries like `coach-plan-2026-10-12` (a dated
morning plan) and multiple `...-UNCOMMITTED` status notes that are stale by
construction.

**FM-5 — The home-directory catch-all bucket.** cwd outside any git repo
hashes to *that dir itself* (store.rs:79-92). Sessions run from `~` all pile
into `project-fe5b40c2...md` (the live file: 25 entries / 18.9 KB mixing bench
ops, gray internals, hermes, disk guard) — and that blob injects into *every*
non-repo session. Cross-contamination of unrelated work under one "project".

**FM-6 — Contradiction = text accretion inside an entry.** `ingest_edit`
requires old text verbatim inside new (store.rs:349-375), producing
`as of <date>: <new> (was: <old>)` blobs; served first-sentences can then
lead with the *stale* half of a belief (first_sentence, store.rs:49-66, cuts
at the first ". " — the new claim — but the entry keeps growing). No
structured supersede/invalid field exists (Zep's lesson).

**FM-7 — Temporal grounding is hidden from the model.** `saved=<date>` lives
only in the HTML trailer, stripped before serving (store.rs:766-779). The
model cannot distinguish a 2026-09-22 fact from a 2026-10-21 fact in the
injected block — exactly the capability LongMemEval shows failing (-30-60%)
and its time-aware fix worth +6.8-11.3%.

**FM-8 — Snapshot freezes mid-session learning.** A `remember` at turn 3 is
invisible *to this session's* injected block until the next session (frozen
at first `prompt/context`, store.rs:140-205). Deliberate for cache stability;
the gap is that the model is told "Memory added." but won't see it in its own
served block — a small but real coherence hazard.

**FM-9 — Dead machinery and doc drift.** distill.rs is still compiled and
`distill-run`/`distill-log` still listed (main.rs:37,223-245); README still
documents the cadenced distill as live behavior; `distill.lock`,
`distill.status`, `worker-scratch/` still exist under ~/.gray/memory/. The
`turn_end` handler is gone, so these artifacts now describe a ghost.

**FM-10 — No evaluation loop.** `growth.json` tracks shape, not function.
Nothing measures whether an injected memory ever changed an action (the
2608.11323 reliability-paper point already in corpus: agent explains <3% of
variance — measure inputs, not vibes).

**Positives worth keeping:** add-only model surface (Hermes lesson),
Why:/falsified: rationale contract, daily budgets, secret redaction,
provenance trailers, advisory-only audit, frozen snapshot + dedupe delivery
(cache-stable), `/memory on|off` + `GRAY_NO_MEMORY` kill-switches, symlink/
private-dir hardening (store.rs:832-924).

**Sibling overlap:** gray-learn duplicates the same job loosely (model-saved
lesson lines -> `learn/*.md`, ~4KB inject, `/learn distill` — its own weak
memory lane); gray-mem0 adds a remote semantic store behind 4 tools;
gray-hoarder already archives every transcript content-addressed (the
episodic layer CER wants); gray-memory-wiki is a stdlib heuristic digester
(Hermes salvage). Any redesign should state the boundary: **memory = curated
beliefs, learn = procedures, hoarder = episodes** — or merge.

---

## 4. Prioritized redesign

Ordered by leverage per unit of work. Each item names its evidence.

### 4.1 Data model (do first — everything else hangs off it)

1. **Structured entry fields, still Markdown.** Keep `- key: text` lines but
   give the trailer real fields: `saved=`, `source=`, `superseded_by=<key>`,
   `status=active|superseded|retired`, `last_confirmed=<date>`,
   `hits=<n>` (times surfaced-and-used). Zep/Graphiti's bi-temporal insight at
   file scale: contradict by *linking*, keep the old line queryable, never
   silently rewrite history (replaces FM-6's `(was:...)` blobs).
2. **Retire the home-dir catch-all** (FM-5): non-git cwd -> a `misc` scope
   that is **retrieval-only, never auto-injected**.
3. **Allow multi-line entries** via `key:` + indented continuation, or cap
   single-line length honestly (~240 chars); current one-line rule produces
   semicolon-stuffed paragraphs that defeat first_sentence rendering (visible
   throughout the live store).
4. **Split the store by kind** — `user` (prefs), `decisions` (beliefs with
   rationale), `state` (ephemeral project status like "UNCOMMITTED on branch
   X", *hard-expiring*, e.g. 7-day TTL). This one field kills most staleness:
   state entries auto-retire instead of being audited by hand.

### 4.2 Write policy (the actual bug — restore an automatic lane, cheaply)

5. **Keep the hot path** (memory tool, unchanged semantics) — LangMem's
   "hot path" leg.
6. **Restore the background leg at event boundaries, not a fixed cadence:**
   run the distill worker on **session end** (or on each compaction event —
   the transcript is already being summarized there), not every 4 turns.
   Same sandboxing as the deleted design (scratch GRAY_MEMORY_HOME +
   GRAY_NO_MEMORY; parse only `ingest-set`/`ingest-edit` lines; <=8 commands).
   Sleep-time-compute framing: consolidate in the gap, not mid-turn. Cost
   drops ~4x+ vs the removed ticker; the rate-limit failure mode in
   distill.log is handled by cheap-model default (`GRAY_MEMORY_DISTILL_MODEL`)
   and MAX_FAILURES already exists.
7. **Add model-*proposable* retirement**: new ingest verb `ingest-retire KEY
   REASON` that sets `status=retired` (not deletion — the line stays
   queryable). Cheap, auditable, preserves the no-silent-wipe guarantee, and
   finally gives the ratchet a down direction (FM-4). Human `remove` still
   hard-deletes.
8. **Correction-triggered nudge (deterministic, ~0 cost):** when a user turn
   matches correction patterns ("no,", "wrong", "actually", "that's not"),
   append a one-line hint suggesting `memory remember/amend` may apply.
   Heuristic write-triggering without a model in the loop; ship disabled by
   default.

### 4.3 Consolidation & forgetting

9. **Scheduled consolidation pass** (`gray memory consolidate` —
   invocable from gateway cron, which exists): merge `audit`'s duplicate-
   target findings into single entries, fold falsified entries into their
   replacements via `superseded_by`, re-stamp `last_confirmed` for entries
   whose failure still hasn't recurred, and expire `state` entries past TTL.
   The audit text already encodes the keep/delete rule — promote it from
   report to transform, still human-reversible since nothing is deleted.
10. **Retrieval-driven decay, not time-decay** (MemoryBank corrected by
    Generative Agents): strength = f(recency, confirmations, hits). Entries
    not surfaced or confirmed in N days drop out of the always-inject block
    first — the store remembers, the prompt forgets. Multi-hop selective
    forgetting is the competency *all* systems fail (MemoryAgentBench <=28%);
    keep it structural (status fields), never a prompt request.
11. **Hard bounds:** per-scope entry cap (~60 active) — overflow forces a
    consolidation run; keep the 4 KiB inject cap. A bound you can see is the
    anti-catastrophic-remembering control; the growth streak warning stays.

### 4.4 Retrieval & injection

12. **Two-tier injection** (Letta/supermemory shape):
    - *Always-inject* (unchanged delivery path): pinned profile —
      user scope top entries + project entries marked `status=active` and
      ranked by strength (item 10), not raw recency (fixes FM-2). Include
      `saved=` dates in the served line (fixes FM-7; LongMemEval's temporal
      result).
    - *On-demand*: extend the `memory` tool with `op: "recall"` (grep/BM25
      over all scopes + session-JSONL dig-up via the existing hoarder/session
      path). At this store size BM25 is sufficient; embeddings are a later
      swap. Fixes FM-3 — the below-waterline store becomes addressable.
13. **Snapshot keying on (session, memory_rev):** freeze stays, but a write
    bumps `memory_rev`, so a mid-session `remember` refreshes the served
    block at the *next compaction boundary* — one cache invalidation per
    memory change instead of never-visible (fixes FM-8 without breaking
    prefix-cache discipline; Don't-Break-the-Cache rules are already met).
14. Keep the `[Context update]` user-injected delivery and exact-match
    dedupe (agent_loop.rs:476-496) — already correct.

### 4.5 Eval plan — how we'd know it's better

15. **Replay harness (primary, cheap):** gray already records full JSONL
    sessions; reuse TRACE's paired-continuation method (corpus 2608.06503):
    pick ~30 real past sessions containing (a) a stated durable preference,
    (b) a mid-project correction, (c) a stale-decision trap. Replay each with
    memory-as-today vs redesigned; score: did the agent act on the belief
    without being re-told? did it follow the *new* side of a contradiction?
    did it abstain on never-saved facts? Paired design controls for task
    difficulty (the 2608.11323 variance lesson).
16. **Store-health metrics** (already half-built): growth streak/removals,
    served bytes/turn, retired-vs-active ratio, % entries with
    `last_confirmed` refreshed, cap-notice count. Dashboard via
    `gray memory audit` + distill.status.
17. **External benchmarks as a regression bar, not the target:** LoCoMo /
    LongMemEval-S are chat-QA shaped — run once to sanity-check the retrieval
    layer, but the gate is the replay suite above. MemoryAgentBench's CR
    subtasks are the right shape for testing supersede handling
    specifically.
18. **Cost accounting:** injected tokens/turn x turns/session; distill worker
    cost per consolidated entry. If memory costs more than it saves in
    re-explanation tokens, the block should shrink — measure, don't assume
    (2601.16746/SWE-Pruner's discipline).

### 4.6 Notes / risks

- Distill resurrection must keep its sandbox (scratch home + GRAY_NO_MEMORY +
  ingest-only parse) — that part was correctly designed; only the trigger was
  wrong.
- `ingest-retire` is a *proposed* new model verb — the one deviation from
  "model can only add"; it's still not deletion (status flag), so it doesn't
  reopen the Hermes wipe hole. If reviewers disagree, gate it behind
  `memory_retire: true` config and let `/memory audit` surface proposals.
- `~/.cache/essentials-e2e/reports/memory.md` (parallel agent) had not landed
  at write time — the e2e perspective (does the wire actually deliver) should
  be merged when it appears; nothing above depends on it.

---

## Appendix — live store snapshot (2026-10-21)

| scope/file | entries | bytes | note |
|---|---|---|---|
| user.md | 18 | 8.1 KB | real prefs + coach/plan entries |
| project-31eb1b37 (`~/gray`) | 45 | 45.6 KB | 8-day burst Sep 22-30, then silence |
| project-fe5b40c2 (`~`, no-git catch-all) | 25 | 18.9 KB | bench+gray+hermes grab-bag — FM-5 |
| project-72284881 (`~/graysite`) | 3 | 3.2 KB | |
| project-60c0469e (`~/graydiscordbot`), b59c8504 (`~/stream`), ba2380f7 (unresolved) | 4 | ~1.4 KB | |
| growth.json | — | — | project peak 25, streak 3, removals 5 |
| distill.log | — | — | last ok run 08:48Z added 3; earlier failure = Devin free-model rate limit |
