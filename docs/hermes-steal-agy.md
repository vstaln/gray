# hermes-agent steal list — agy analysis

Source: `/home/vstaln/.cache/checkouts/github.com/NousResearch/hermes-agent`
(analyzed 2026-08-24). Cross-checked with adventurer recon in
`docs/hermes-recon.md` — verdicts merged in the spec.

## The Ruthless Steal Checklist

| Feature | Action | Destination in `gray` |
|---|---|---|
| **3-Tier System Prompt (stable/context/volatile)** | STEAL | `gray-core/src/prompt.rs` |
| **Tool Output Disk Spillover (`<persisted-output>`)** | STEAL | `gray-tools/src/spillover.rs` |
| **OpenAI Streaming Tool Chunk Accumulator** | STEAL | `gray-provider/src/openai.rs` |
| **Tool Error Text Bounding (2KB cap)** | STEAL | `gray-tools/src/lib.rs` |
| **Progressive Disclosure Skills (`SKILL.md` + index)** | STEAL (Phase 2) | `gray-core/src/skills.rs` |
| **Structured `/learn` Prompt Template** | STEAL (Phase 2) | `gray/src/templates/learn.md` |
| **Curated Frozen Memory (`MEMORY.md`, `USER.md`, `\n§\n`)** | STEAL (Phase 2) | `gray-tools/src/builtins/memory.rs` |
| **Subagent Tool (Nested `Agent::run` + Filtered Tools)** | STEAL (Phase 2) | `gray-tools/src/builtins/delegate.rs` |
| **Git Worktree Subagent Isolation** | STEAL (Phase 2) | `gray-tools/src/worktree.rs` |
| **Minimal Tokio Interval Cron** | ADAPT (Phase 2) | `gray/src/cron.rs` |
| **37 Provider Plugins / Custom Cloud SDKs** | SKIP | Keep hand-rolled OpenAI + Anthropic protocols |
| **Fuzzy Patch Matching / AST Audit** | SKIP | Keep exact-match `edit` tool |
| **SQLite `state.db` (FTS5 triggers, CJK trigram tables)** | SKIP | Keep turn-level JSONL sessions |
| **15+ Messaging Gateways (WeChat, Signal, Discord, etc.)** | SKIP | Keep axum SSE web API + embedded UI |

## Key mechanisms (details worth keeping)

- **Skills kernel:** one static authoring-standards prompt (`build_learn_prompt`)
  fed as a normal turn; agent writes `SKILL.md` with ordinary file tools.
  Index = YAML frontmatter (name+description) injected into system prompt;
  body loaded on demand via one `skill_view` tool. ~200 LOC total. Skip the
  curator/.usage.json sidecars entirely.
- **Memory kernel:** per-target markdown entry lists on disk with char caps;
  add/replace/remove ops; rendered verbatim into system prompt. Plus a static
  `MEMORY_GUIDANCE` system-prompt paragraph (pure text, zero code).
- **Session search:** JSONL stays source of truth; if needed, `rg` over
  `~/.gray/sessions/*.jsonl` first, optional single-table FTS5 sidecar later.
- **Cron kernel:** `jobs.json` ({id, schedule, prompt}) + 60s ticker +
  headless `Agent::run` per fire. ~120 LOC in Rust.
- **Delegate kernel:** child `Agent::run` with filtered registry (no recursive
  delegate/memory/cron), final text returned as tool result; optional git
  worktree isolation.

## Scale warning (headline)

hermes-agent is ~409k+ LOC across agent/tools/gateway alone. Every mechanism
gray cares about has a small kernel wrapped in 10–100x product plumbing.
Steal kernels only.
