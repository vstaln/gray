# hermes-agent recon — steal list for gray

Repo: `/home/vstaln/.cache/checkouts/github.com/NousResearch/hermes-agent` (Python).
Scale check: `run_agent.py` 9,269 lines; `hermes_state.py` 14,311 lines; `gateway/run.py`
31,358 lines; `agent/` + `tools/` + `gateway/` alone ≈ 409k LOC. **Hermes is ~100–500x
the size gray is aiming for.** Almost every mechanism below is wrapped in layers of
guardrails, ledgers, recovery logic, and multi-platform plumbing that gray should not
port. Verdicts are calibrated accordingly.

Status: REFINED (key mechanisms confirmed by targeted reads; bulk files never read end-to-end).

## Refined mechanism notes (CHECKED — signatures/bodies read)

### Skills index injection (`agent/prompt_builder.py` `build_skills_system_prompt`, L1828)
Scans `~/.hermes/skills/**/SKILL.md`, parses YAML frontmatter
(`agent/skill_utils.py::parse_frontmatter`), renders a compact name+description
index into the system prompt's *volatile* section (system prompt = stable part +
volatile tail: skills index, memory snapshot, USER.md profile, timestamp — see
`agent/system_prompt.py`). Full body loaded on demand via `skill_view` /
`skills_list` tools. Hermes adds two cache layers (LRU + disk mtime manifest) —
unnecessary at gray scale.

### Self-improving loop is PROMPT-driven, not code-driven (`agent/learn_prompt.py`)
`/learn <request>` calls `build_learn_prompt(user_request)` which returns ONE big
authoring-standards prompt (SKILL.md must be lean, ≤ size cap; large sources become
a "knowledge-base" SKILL.md index + on-demand `references/` chapters; untrusted-source
hygiene rules) that is fed to the agent **as a normal turn**. The agent then writes
the skill itself using its ordinary file tools (`skill_manage`, `write_file`).
So "skill creation from experience" = a good static prompt + the skills dir being
writable by the agent. The background `agent/curator.py` is a separate scheduled
consolidation pass (configurable interval/idle/stale/archive days) that merges
overlapping umbrella skills and archives stale ones; `tools/skill_usage.py` keeps a
`.usage.json` sidecar (activity counts per skill) to inform it.

### Memory = MEMORY.md / USER.md stores + static guidance ("the nudge")
`tools/memory_tool.py::MemoryStore`: per-target list-of-string entries persisted to
markdown-ish files, char limit per target, ops add/replace/remove/apply_batch,
rendered into every system prompt via `format_for_system_prompt`.
The "memory nudge" is NOT runtime magic — it is a static paragraph:
`agent/prompt_builder.py::MEMORY_GUIDANCE` (L171): tells the model to save durable
user facts, prefer preference-facts over imperative instructions, never save
session/task state (use session_search for transcripts instead), and route reusable
procedures to skills instead. Plus a recall layer (`agent/memory_manager.py`
`prefetch_all(query)`) that can pull from external memory providers before a turn —
skippable for gray.

### Session search FTS5 (`hermes_state_search.py`)
SQLite virtual tables: `messages_fts` (standard tokenizer) plus `messages_fts_cjk`
and `messages_fts_trigram` shadow tables for substring/CJK matching;
`_sanitize_fts5_query` quotes user input to keep it out of raw MATCH syntax;
graceful degradation when the fts5 extension is missing (`fts5_unavailable`).
Exposed to the model as `session_search` tool (`tools/session_search_tool.py`),
which MEMORY_GUIDANCE explicitly points at for transcript recall.

### Provider layer (`providers/base.py` + `run_agent.py` + adapters)
`providers/base.py::ProviderProfile` is only a config/hook object (prepare_messages,
extra_body, max_tokens, fetch_models) — the actual wire handling lives in run_agent's
AIAgent plus per-backend adapters (`agent/anthropic_adapter.py` etc.). The OpenAI-
compat path is one giant method family with provider-sniffing helpers
(`_is_openrouter_url`, `_is_copilot_url`, ...). Gray should NOT imitate this shape:
two clean trait impls beat one sniffing mega-class.

### Gateway surfaces (`gateway/platforms/`)
Present platforms: `signal.py`, `whatsapp_cloud.py`, `webhook.py`, `api_server.py`,
`qqbot/`, `weixin.py`, `yuanbao.py`, msgraph webhook, bluebubbles (iMessage).
Telegram/Discord were NOT found under gateway/platforms in this checkout — they may
live in plugins/ or be absent here (INCONCLUSIVE; doesn't change the verdict).
Each platform adapter normalizes inbound chat events into sessions handled by
`gateway/run.py`; cross-cutting concerns (delivery ledger, pairing, wake, TTS,
slash commands) are what make it 31k lines.

## Capability map

## Capability map

| Capability | Hermes location | Verdict | Minimal Rust sketch for gray |
|---|---|---|---|
| ReAct loop | `run_agent.py` (`class AIAgent`, L421+); `agent/conversation_loop.py` (8,590 L) | ADAPT | Gray's spec already has this right: one loop fn, event enum, injected provider/tool traits. Do NOT port hermes's stream-diag/retry/status-buffer machinery — it is 90% of the bulk. |
| Provider layer | `providers/base.py` (`ProviderProfile` — thin config/hook object); wire logic in `run_agent.py` AIAgent + per-backend adapters `agent/anthropic_adapter.py` (3,284 L), `gemini_native_adapter.py`, `bedrock_adapter.py`, `codex_responses_adapter.py`; provider sniffing via `_is_openrouter_url`-style helpers | STEAL (concept), SKIP (shape) | Two hand-rolled HTTP/SSE clients behind a `Provider` trait returning `ContentBlock` streams, exactly as gray spec says — do NOT copy hermes's one-mega-class-plus-sniffing shape. Steal only SSE frame-parsing edge cases (reasoning deltas, fragmented tool-call args, `[DONE]`, Anthropic `content_block_delta`/`message_delta`). |
| Tools / registry | `tools/registry.py`, `toolsets.py` (1,083 L), `agent/tool_executor.py`, `agent/tool_dispatch_helpers.py` | ADAPT | `Tool { name, schema, run(args) -> Result }` trait + static registry vec. Skip hermes's toolset-distribution profiles, approval gates, guardrails for v0. |
| Skills as SKILL.md dirs | `skills/*/SKILL.md` (15 categories on disk); loading/validation: `agent/skill_utils.py` (`parse_frontmatter`), `agent/skill_bundles.py`; index injection: `agent/prompt_builder.py::build_skills_system_prompt` L1828; runtime tools: `tools/skills_tool.py` (`skill_view`, `skills_list`) | STEAL | Scan `<skills_dir>/*/**/SKILL.md`, parse YAML frontmatter (name/description), inject name+description index into system prompt, load full body on demand via a `read_skill` tool. ~200 lines total. |
| Skill CREATION from experience (self-improving loop) | Prompt: `agent/learn_prompt.py::build_learn_prompt` (one static standards prompt fed as a normal turn); agent writes SKILL.md via its own file tools; guards/validators in `tools/skill_manager_tool.py` (`_validate_frontmatter`); background consolidation `agent/curator.py` (interval/idle/stale/archive config, merges umbrella skills); usage sidecar `tools/skill_usage.py` (`.usage.json`) | ADAPT (steal the prompt pattern, skip curator) | The kernel is: one good authoring-standards prompt + skills dir being writable by the agent. For gray: a `/learn`-style slash command that prepends the standards prompt to the turn and lets the normal write tool create/update SKILL.md. Skip curator consolidation, usage ledgers, AST audits, org mirrors — product-scale tax. |
| Agent-curated memory | `tools/memory_tool.py` (`MemoryStore`: per-target entry lists on disk, char limits per target, `add/replace/remove/apply_batch`, `format_for_system_prompt` renders entries into system block, file-locking + drift detection via `.bak`) ; orchestration `agent/memory_manager.py`, `memory_provider.py` | STEAL | Memory = one markdown file (or N sections) injected verbatim into system prompt + `memory_add/memory_replace/memory_remove` tools that rewrite the file atomically. That is genuinely small and matches hermes's data shape (list-of-string-entries with char cap). Skip locking/drift/consolidation machinery. |
| Memory nudges | Static `MEMORY_GUIDANCE` paragraph, `agent/prompt_builder.py` L171 (injected into system prompt volatile tail by `agent/system_prompt.py`) + optional prefetch recall `agent/memory_manager.py` | STEAL (the guidance text) / SKIP (prefetch layer) | The nudge is just a well-written system-prompt paragraph: "save durable facts, prefer declarative over imperative phrasing, don't save session state (search transcripts instead), route procedures to skills". Copy the idea and write gray's own 6-line version. No code. |
| Session store + search (FTS5) | `hermes_state.py` (14k L, sqlite schema incl. messages/events), FTS5 in `hermes_state_search.py` (2,510 L): `messages_fts` + `messages_fts_cjk` + `messages_fts_trigram` virtual tables, `_sanitize_fts5_query` quoting, graceful `fts5_unavailable` fallback; exposed as tool `tools/session_search_tool.py`; MEMORY_GUIDANCE routes transcript recall here | ADAPT | Gray already has JSONL sessions. Minimal version: keep JSONL as source of truth, add optional SQLite sidecar with one `messages_fts USING fts5(content)` table rebuilt lazily, quote user input, expose a single `session_search(query)` tool. The FTS5 idea is cheap; hermes's 2.5k-line wrapper (CJK/trigram shadow tables, corruption recovery) is not. |
| Messaging gateway (one process, many surfaces) | `gateway/run.py` (31k L orchestrator), platform adapters `gateway/platforms/` (present: signal, whatsapp_cloud, webhook, api_server, qqbot, weixin, yuanbao, bluebubbles; telegram/discord NOT found here — INCONCLUSIVE, maybe plugins), session routing `gateway/session.py` (4,172 L), delivery ledger `gateway/delivery_ledger.py`, slash commands `gateway/slash_commands.py` (6k L) | ADAPT | The architectural idea — every surface normalizes into an inbound `Message{session_id, text}`, agent events stream back out through a per-surface sink — is exactly right and costs nothing in Rust: an enum `Gateway::{Cli, Web}` feeding the same `Agent::run`. Port NOTHING of hermes's implementation (pairing, ledgers, wake words, TTS, stickers...). |
| Cron scheduler | `cron/scheduler.py`, `cron/jobs.py`, `cron/executions.py`, `cron/notepad.py`; user-facing tools `tools/cronjob_tools.py` | ADAPT | Minimal: tokio interval task reading a `jobs.json` ({id, cron_expr, prompt}), each fire spawns a headless `Agent::run` and appends result to a session. Skip failure-streak nudges, blueprint catalogs, suggestion engines. |
| Subagents | spawn/delegate: `tools/delegate_tool.py` (4,963 L), lifecycle `agent/subagent_lifecycle.py`, steering `steer_subagent()`/interrupt, worktrees `tools/subagent_worktree.py`, async delegation `tools/async_delegation.py` | ADAPT | v0 gray has none by design. If ever added: spawn a thread running another `Agent` with its own session id, return final transcript as tool result. Steering/interrupts/worktrees/heredoc-capture are all skip-tier. |
| Compaction / context compression | `agent/context_compressor.py` (8,211 L), `conversation_compression.py` (4,465 L), `native_compaction.py`, `trajectory_compressor.py` (1,598 L) | SKIP (for now) | Flagged because gray will need *something*: minimal = when tokens > threshold, ask the summarizer model to compress oldest half into one message. Hermes's 12k+ lines here include display, feedback, native-API variants — pure bloat at gray scale. |

## Notable structural observations

- `run_agent.py`'s `AIAgent.__init__` takes dozens of params; the "loop" is smeared
  across `conversation_loop.py` + `agent_runtime_helpers.py` (4,541 L). Lesson for
  gray: keep the loop in ONE function; hermes proves how fast it metastasizes.
- Skills live as plain markdown with YAML frontmatter under category dirs — no DB,
  no registry service. That choice is what makes them steerable/portable; copy it.
- Memory data shape confirmed: `MemoryStore._entries_for(target) -> List[str]`,
  per-target char limit, `format_for_system_prompt(target)` injects rendered block.
  (CHECKED in tools/memory_tool.py signatures.)
- Everything has a guard/ledger/forensics twin (delivery_ledger, shutdown_forensics,
  restart_loop_guard, curator_backup...). This is the cost of being a product;
  gray's non-goals exist precisely to avoid this tax.

## Honesty labels

- File locations & line counts: PROVEN (ls/wc/grep output above).
- Skills index injection, /learn prompt flow, MEMORY_GUIDANCE text, FTS5 table
  strategy, MemoryStore ops, ProviderProfile shape, gateway platform list:
  CHECKED (read directly this session).
- Curator consolidation behavior: CHECKED-SIGNATURES + config getters
  (interval/idle/stale/archive/prune/consolidate); full 1k+ line body not read.
- Cron job JSON schema fields, subagent steering internals, compaction internals:
  INCONCLUSIVE at detail level — verdicts rest on the architectural role only.
