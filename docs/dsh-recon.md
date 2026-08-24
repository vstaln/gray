# DSH Recon — DeepSeek Harness

Repo: /home/vstaln/.cache/checkouts/github.com/deepseek-ai/deepseek-harness (MIT, (c) 2026 DeepSeek).
Tagline "Everything is a plugin" is literal: powered by [Cordis](https://github.com/cordiverse/cordis) — plugins contribute services, typed events, and reversible effects to a shared context. docs/architecture.md: "Every part of the product is a plugin, including the model adapter, the tool registry, the session log, and the agent loop itself... There is no privileged core to patch."

## 1. Architecture
- TypeScript/Node monorepo, pnpm workspace (~55 packages under packages/, plus apps/, python/, native/, vendor/, website/). Developer preview.
- Composition: profile → bundles → cordis.patch.yml overlays. `dsh-base` bundle = first layer (model adapters, tools, persistence, sandbox/approval policy); `dsh-web-app` / `dsh-headless` add UI or headless runner.
- Core agent loop: `packages/core/agent-loop/src/` — only ~1,662 LOC total (agent.ts 515 + index.ts 713 + tool-calls.ts 289 + misc). It is itself a plugin registered at `ctx.agentLoop`, implementing the `Agent` interface from `packages/core/agent/`.
- Core vs plugins: true core = core/{session 3164, tools 5628, agent 1636, system-prompt 605, scope 561, agent-loop 1662} ≈ ~13k LOC of framework; every capability (bash, fs, web, subagent, todo, plan, skill, lsp, mcp, jobs...) is a separate package in packages/<domain>/tool-*.

## 2. Plugin interface
Two levels:
(a) Cordis Service classes on a typed Context (`ctx.tools`, `ctx.sessions`, `ctx.systemPrompt`, `ctx.llm`, `ctx.agents`). Extension points are typed EVENTS with waterfall/emit modes — e.g. `'tools/pre-execute'(...)` / `'tools/execute'(exec, next) => Promise<ToolExecutionResult>` / `'tools/post-execute'` waterfalls, `'tools/result'` emit, `'tools/change'`. Registrations are effects that unwind when their plugin unloads.
(b) Tools are plain values via `defineTool(...)`, registered by a plugin with one line (packages/shell/tool-bash/src/index.ts:242): `ctx.tools.register(defineTool({...}))`.

Key type verbatim (packages/core/tools/src/index.ts):
```ts
/** A registered tool: its schema plus the execution function. */
export interface ToolDefinition extends ToolSchema {
  /** Mandatory canonical output declaration. */
  readonly output: ToolOutputDefinition
  execute(args: unknown, exec: ToolRunContext): Promise<unknown>
  finalizeContent?(exec: Readonly<ToolExecution>, result: Readonly<ToolExecutionResult>): ContentBlock[] | undefined
  timeoutMs?: number          // enforced by a separate tools/execute wrapper policy plugin
  isConcurrencySafe?(args: unknown): boolean   // opts into parallel dispatch
  presentCall?(args: unknown): ToolCallView | undefined
  presentResult?(args: unknown, result: ToolResult): ToolResultView | undefined
}
```
How minimal is kept: the harness owns only schema+execution+waterfall seams; timeouts, spill/truncation, approval, parallelism classification are all SEPARATE policy plugins listening on the waterfall.

## 3. System prompt
Assembled from ordered sections (PromptSection{name, order, text|fn}, order -100 harness identity / 0 persona / 100–199 tool guidance), rendered per-assembly with {{var}} interpolation. Default identity section is ONE line (packages/core/system-prompt/src/index.ts:358-362):
`'You are an AI agent powered by DeepSeek Harness.'`
There is NO large built-in system prompt; each tool package contributes its own guidance section. Persona defaults to ''. So default size ≈ tens of bytes + tool descriptions + optional runtime-context section.

## 4. Tools & truncation
Built-ins shipped in dsh-base bundle (packages/bundle/base/package.json deps): tool-bash (+persistent variants, pwsh), tool-fs (read/write/edit), tool-fs-search (grep/glob), tool-str-replace-editor, tool-web (fetch/search), tool-skill, tool-subagent/-control/-report, tool-todo, tool-goal, tool-jobs, tool-workflow, tool-ralph, tool-lsp, tool-mcp, tool-session-query, tool-ask-user, plus code-mode (`run_code` transport where model writes code that sub-dispatches native tools — only sub-dispatches may call native names).
Truncation: `packages/spill/spill-policy` — a `tools/post-execute` transformer keyed off `maxInlineBytes`; oversized plain-text results saved whole to ctx.spillStore and replaced by bounded head/tail preview + locator notice ("(Omitted N bytes. Full formatted result stored at: … Use read with offset/limit…)"); read results exempted to avoid read→spill→read loops; never exceeds cap. Per-tool provider caps (e.g. web-fetch maxBodyChars, bash stream spill, glob/grep item-level spill) stay separate.

## 5. Session/state
Append-only SessionEvent log per session; backend `dsh-session-persistence-jsonl`: `<root>/<normalized-cwd>--/<encoded-id>/session.jsonl.zstd` (checksummed header frame + append frames; raw .jsonl option). Header stores version/id/cwd/createdAt/parentSession/delegationDepth/agentPreset — agentPreset durable so resume restores exactly the tools/prompt composition the history was made under. Optional lossless chunk packing for assistant deltas. Resume = replay log; every request derived from the log (agent-loop header comment).

## 6. DSH vs pi-mono (inferable)
DSH has: Cordis service/event/waterfall DI with hot unload/reload and config patches (pi has simpler extensions); profiles/bundles layering; code-mode run_code aggregate tool transport; zstd JSONL logs with chunk packing; spill-to-file result policy as a standalone plugin; web UI app; approval/sandbox policy plugins; ACP support. pi has: single-binary simplicity, TUI-first, lighter event surface. DSH's cost: enormous package count and indirection for what gray wants.

## 7. Steal list for gray
1. One-line tool registration from a tiny trait object — STEAL-now: `Registry.register(Tool { name, desc, params_schema, run })`.
2. Ordered system-prompt sections (-100 identity / 0 persona / N tool guidance), assembled not hardcoded — STEAL-now: `Vec<(i32, String)>` sorted at assembly.
3. Tiny default system prompt (one line) with guidance living next to each tool — STEAL-now: put each tool's usage doc in its description/section.
4. Spill policy: oversize tool output → file + head/tail preview + locator — ADAPT-phase2: if body > MAX, write tmp file, return first/last N + path hint.
5. Durable append-only JSONL session log with header recording toolset/prompt version so resume validates composition — ADAPT-phase2: serde lines + header struct with tool-set hash.
6. Waterfall hooks pre/post tool execution (approval, redaction, metrics) — ADAPT-phase2: `Hook fn(&mut Result) -> Decision` list instead of Cordis events.
7. `isConcurrencySafe` opt-in flag for parallel tool calls — STEAL-now: bool field, run flagged calls joinly.
8. Code-mode run_code transport, profiles/bundles, zstd chunk packing, web app — SKIP (complexity far beyond minimal Rust harness).

## License
MIT (LICENSE: "MIT License, Copyright (c) 2026 DeepSeek"); third-party notices in THIRD_PARTY_NOTICES.md.
