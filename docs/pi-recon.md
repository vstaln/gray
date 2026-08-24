# pi-mono Recon — architecture notes for gray

Repo: `/home/vstaln/.cache/checkouts/github.com/pi-mono` (checked out without the `badlogic/` path segment). Monorepo packages: `agent`, `ai`, `client`, `coding-agent`, `evals`, `protocol`, `server`, `session-backends`, `telemetry`, `tui`.
Core agent engine = `packages/agent` (2,368 LOC in src/*.ts); coding-agent product layer = `packages/coding-agent`.

## 1. Default system prompt

**Location:** `packages/coding-agent/src/core/system-prompt.ts` — `buildSystemPrompt()` (options interface at lines 9–26; template literal starts ~line 108).

**Verbatim base template** (before substitution; placeholders `{toolsList}`, `{guidelines}`):

```
You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.

Available tools:
{toolsList}

In addition to the tools above, you may have access to other custom tools depending on the project.

Guidelines:
{guidelines}

Pi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):
- Main documentation: {readmePath}
- Additional docs: {docsPath}
- Examples: {examplesPath} (extensions, custom tools, SDK)
[... 5 more bullets mapping each doc topic to its file ...]

Current working directory: {cwd}
```

**Size:** base template body is **174 words / 1,355 chars (~350 tokens)** (CHECKED NUMERICALLY via wc). Rendered with the default 4 tools + guidelines + cwd it lands around 220–260 words / ~450 tokens. Everything else (project context `<project_instructions>` blocks, skills section) is appended conditionally.

**How tools are described:** each tool contributes a ONE-LINE `promptSnippet` plus optional `promptGuidelines` bullets (`ToolDefinition.promptSnippet?` / `promptGuidelines?`, extensions/types.ts:453–460). Only tools WITH a snippet appear under "Available tools" as `- name: snippet`. Actual snippets (tools/*.ts):
- read: "Read file contents"
- bash: "Execute bash commands (ls, grep, find, etc.)"
- edit: "Make precise file edits with exact text replacement, including multiple disjoint edits in one call"
- write: "Create or overwrite files"
- grep: "Search file contents for patterns (respects .gitignore)"
- find: "Find files by glob pattern (respects .gitignore)"
- ls: "List directory contents"

Guidelines are conditional on the selected tool set (e.g. "Use bash for file operations like ls, rg, find" only when bash present and grep/find/ls absent), plus two always-on: "Be concise in your responses", "Show file paths clearly when working with files". A custom prompt replaces everything but still gets appended context/skills/cwd.

## 2. Core loop

**Files:** `packages/agent/src/agent-loop.ts` (796 lines — THE loop), `agent.ts` (Agent class/state), `stream-fn.ts`, `types.ts`. Product wiring: `coding-agent/src/core/agent-session.ts`.

Pseudocode of `runLoop()` (agent-loop.ts:166–278):

```
pending = getSteeringMessages()            # user typed while waiting
loop:                                       # outer: follow-ups keep agent alive
  while hasToolCalls or pending:
    emit turn_start
    inject pending messages into context
    msg = streamAssistantResponse(ctx, config)   # AgentMessage[] -> LLM Message[] at boundary only
    if stopReason in {error, aborted}: emit turn_end, agent_end; return
    if msg.stopReason == "length": fail all tool calls (truncated args)
    results = executeToolCalls(...)          # parallel or sequential per executionMode
    append results to context
    emit turn_end { message, toolResults }
    apply prepareNextTurn snapshot (model/thinking can change mid-run)
    if shouldStopAfterTurn(): emit agent_end; return
    pending = getSteeringMessages()
  followUps = getFollowUpMessages()
  if none: break else pending = followUps
emit agent_end { messages }
```

**Streaming/event model** (`packages/agent/src/types.ts:428–443`, `AgentEvent` union):
`agent_start`, `agent_end{messages}`, `turn_start`, `turn_end{message,toolResults}`, `message_start{message}`, `message_update{message,assistantMessageEvent}`, `message_end{message}`, `tool_execution_start{toolCallId,toolName,args}`, `tool_execution_update{...,partialResult}`, `tool_execution_end{...,result,isError}`.
Events flow through `EventStream<AgentEvent, AgentMessage[]>` (from pi-ai); loop terminates when `event.type === "agent_end"`.

## 3. Tool set

Built-ins (`packages/coding-agent/src/core/tools/`): **read, bash, edit, write, grep, find, ls** (`allToolNames`, tools/index.ts:73). Default coding set = read/bash/edit/write (`createCodingToolDefinitions`); read-only set = read/grep/find/ls. Schema sizes are small TypeBox objects — most are ~4–13 lines of schema code; edit is the outlier (~149 lines incl. multi-edit array shape).

**Truncation** (`tools/truncate.ts`): `DEFAULT_MAX_LINES = 2000`, `DEFAULT_MAX_BYTES = 50KB`; outputs are head/tail-truncated IN PLACE with a size annotation (`truncateHead`/`truncateTail`, formatSize helper). Grep caps match lines at 500 chars (`GREP_MAX_LINE_LENGTH`). No disk spillover — truncation only.

## 4. Session format

`coding-agent/src/core/session-manager.ts`: **append-only JSONL tree**, `~/.pi/.../<timestamp>_<id>.jsonl` (line 953).
- First line: `SessionHeader { type:"session", version:3, id, timestamp, cwd, parentSession? }` (lines 32–39).
- Each entry: `{ type, id, parentId, timestamp, ... }` — entries form a TREE via id/parentId with a leaf pointer; branching = move leaf to an earlier entry, no history rewrite (docstring lines 845–852).
- Entry types (`SessionEntry` union, lines 144–156): `message` (full AgentMessage incl. assistant tool calls + toolResults — not just user/assistant turns), `thinking_level_change`, `model_change`, `compaction`, `branch_summary`, `custom` (extension state, NOT sent to LLM), `custom_message` (extension content, IS sent to LLM as user message), `label` (bookmarks), `session_info` (display name).
- Write discipline: nothing hits disk until the first assistant message exists (avoids empty sessions); then pure `appendFileSync` per entry; `_rewriteFile()` only for migrations/branch copies.

So: pi stores **messages AND non-message events** side by side in one log; streaming deltas are never persisted (only completed messages).

## 5. Extension model

pi stays minimal because `packages/agent` knows nothing about coding tools; ALL product behavior lives in `coding-agent` behind these seams:

- **`ToolDefinition`** (extensions/types.ts:449–517): `{ name, label, description, promptSnippet?, promptGuidelines?, parameters (TypeBox), execute(toolCallId, params, signal, onUpdate, ctx), renderCall?, renderResult?, executionMode?: "sequential"|"parallel", prepareArguments? }`. Tools self-describe their prompt presence.
- **`ExtensionContext`** (types.ts:307–352): what an extension sees — ui, mode ("tui"|"rpc"|"json"|"print"), sessionManager (readonly), modelRegistry, abort(), compact(), getSystemPrompt(), getContextUsage().
- **Hook events** (types.ts:562–806): lifecycle hooks — `SessionStartEvent`, `SessionBeforeSwitch/Fork/Compact/Tree`, `BeforeProviderRequestEvent`, `AfterProviderResponseEvent`, `BeforeAgentStart/AgentStart/AgentEnd/AgentSettled`, `TurnStart/TurnEnd`, `MessageStart/Update/End`, `ToolExecutionStart/Update/End`, plus `ProjectTrustEvent`. Extensions observe/intervene at every seam without touching the loop.
- **Custom session entries** (`custom`, `custom_message`) let extensions persist state and inject context through the same JSONL log.
- **System-prompt seams**: `appendSystemPrompt`, `promptGuidelines`, per-tool snippets, skills section — growth happens by appending small sections, never rewriting the core prompt.

Loader/runner: `extensions/loader.ts` (806 lines), `runner.ts` (1236 lines).

## 6. Contradictions vs gray spec (docs/superpowers/specs/2026-08-24-gray-harness-design.md)

1. **Session linearity vs tree.** Gray spec: linear JSONL, normalized Messages only. Pi: id/parentId tree supporting branch/fork/labels. Not fatal for v0, but gray's flat `append/load` SessionStore forecloses branching later; consider giving entries a stable `id` now (cheap) even if you never branch.
2. **What gets stored.** Gray: "turn-level normalized Messages only". Pi also persists *state-change* entries (model_change, thinking_level_change, compaction records, labels) in the same log so resume reconstructs exact conditions. Gray's metadata-first-line covers some of this; mid-session model switches would be lost on resume.
3. **Tool output handling.** Gray: >8KB spills to disk w/ `<persisted-output path>` pointer (hermes-born). Pi: no spillover — in-place head/tail truncate at 2000 lines/50KB. Two different philosophies; pi's is simpler and avoids littering ~/.gray/spill. Consider pi-style truncation as v0 default, spill as opt-in later.
4. **Steering / follow-up queues.** Gray's loop has no seam for injecting user messages mid-turn or queuing follow-ups (only abort). Pi treats this as first-class (`getSteeringMessages`/`getFollowUpMessages` config callbacks). If the web UI ever allows typing while streaming, gray will need this seam — worth reserving a hook now.
5. **System prompt structure.** Gray plans a fixed 3-tier (stable/context/volatile) builder. Pi has NO tiers — a flat template whose *content* is conditional (tools present → guidelines present) plus appended sections. Pi's evidence supports gray's minimalism instinct: identity paragraph + one-line-per-tool + 2–4 guidelines is enough; the tier machinery may be over-engineering for v0.
6. **Event granularity.** Gray's `AgentEvent` enum lacks `message_update` (partial assistant deltas as structured updates) — gray streams TextDelta directly, which is fine, but note pi separates provider StreamEvents from agent AgentEvents; gray's single enum merges both layers. Acceptable, just be deliberate.
7. **No contradiction found on**: injected provider/tool traits, error-results-to-model, deferred compaction with a token-budget seam, minimal builtin set (pi ships 7, gray 5 — fine).

Aligned takeaways for gray: tiny system prompt with per-tool one-liners; messages-only-at-boundaries; events as serde-tagged enum mirrors pi's discriminated union; trait-injected executor == pi's config callbacks.
