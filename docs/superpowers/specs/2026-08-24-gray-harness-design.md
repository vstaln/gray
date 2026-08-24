# gray — minimal modular agent harness (design)

Date: 2026-08-24
Status: approved direction, awaiting implementation plan

## Surface

**Default interface: a local web app.** The `gray` binary starts an axum
server on `127.0.0.1:<port>` and serves an embedded single-page chat UI.
You run `gray`, open a browser tab, talk to it. No accounts, no auth beyond
loopback binding, sessions on disk. Cloud hosting / multi-user is a later
product decision — architecture allows it (SessionStore trait, stateless
events) but nothing in v0 assumes it.

The chat UI is **lifted from grayweb/gray-app** (`src/components/gray/chat/*`):
messages list, composer, markdown rendering, thinking block, auto-scroll,
streaming hook — stripped of Supabase auth, reminders, grounding panels,
profiles, and payments. It talks to the Rust backend over SSE instead of the
FastAPI backend.

## What gray is

A minimal, modular agent harness written in Rust. Surface strategy:
**terminal-first** (developers now), consumer chat surfaces later as a
thin gateway — the core loop is audience-agnostic by design. — the philosophy of
[pi](https://github.com/badlogic/pi-mono) with the crate discipline of
[oh-my-pi](https://github.com/can1357/oh-my-pi)'s Rust core. One binary, a
handful of small crates, an event-driven agent loop, and clean seams for
future growth (skills, MCP, subagents). It is **not** a companion product;
the mentor/companion features from grayweb are out of scope.

References studied: pi-mono (architecture), oh-my-pi (Rust workspace layout),
openai/codex (Rust exec loop), block/goose (provider traits),
superagent-ai/grok-cli (proof of how small a v0 can be).
Papers anchoring the loop design: ReAct (2210.03629), Toolformer (2302.04761),
CodeAct (2402.01030), SWE-agent (2405.15793).

## Non-goals for v0

- No TUI polish — terminal is the dev backdoor (`--print` one-shot + basic REPL)
- No MCP, no subagents, no skills directory
- No chat-app gateways (Telegram/WhatsApp/etc.) — ever, until asked again
- No cloud/hosted/multi-user deployment — localhost only
- No embeddings/memory
- No provider SDKs — hand-rolled HTTP/SSE against two wire protocols

## Workspace layout

```
gray/
├── Cargo.toml              # [workspace] members = ["crates/*"]
└── crates/
    ├── gray-provider/      # LLM wire protocols + streaming
    ├── gray-core/          # agent loop, event stream, message model
    ├── gray-tools/         # Tool trait + 5 builtins
    ├── gray-session/       # SessionStore trait + JSONL impl
    └── gray/               # binary: axum web server + embedded UI,
                            #   plus --print one-shot mode and bare REPL
```

Dependency rule (enforced by structure): `core` depends only on its own
types; `provider`, `tools`, `session` do not depend on each other; the
`gray` binary wires them together.

## Crate contracts

### gray-core

Owns the message model and the loop.

- Types: `Message { role, content }`, `ContentBlock::{Text, ToolUse, ToolResult}`,
  `ToolDef { name, description, parameters: JsonSchema }`, `Usage`.
- `Agent::run(input)` drives: build request → `provider.stream()` → yield events →
  on complete tool calls, execute via injected `dyn ToolExecutor` → append results
  as messages → repeat until a turn ends with no tool calls.
- Event enum (serde-tagged): `Start`, `TextDelta(String)`, `ToolCallStart { id, name }`,
  `ToolCallEnd { id, args }`, `ToolResult { id, output, is_error }`,
  `TurnEnd { stop_reason, usage }`.
- The core never spawns processes, opens files, or reads env. Provider and
  tool executor are injected traits.

### gray-provider

Two hand-rolled protocol clients behind one trait:

```rust
trait Provider {
    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>>;
}
```

- OpenAI-compatible: `POST {base_url}/chat/completions`, SSE. Covers OpenAI,
  OpenRouter, Groq, local llama.cpp/vLLM/Ollama (`--base-url`).
- Anthropic: `POST {base_url}/v1/messages`, SSE, `anthropic-version` header.
- Normalizes both into `StreamEvent::{TextDelta, ToolCallDelta, MessageComplete}`.
- SSE parsing: `eventsource-stream` over `reqwest::Response::bytes_stream()`.
  OpenAI streams tool-call arguments as JSON fragments across chunks keyed by
  `index` — accumulate into a per-tool-call string buffer; parse the completed
  JSON exactly once at message end, never per-chunk.
- Retries 429/5xx with exponential backoff + jitter (3 attempts default);
  auth errors fail fast.
- Keys from env: `GRAY_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`;
  base URL override per config.

### gray-tools

```rust
#[async_trait::async_trait]
trait Tool: Send + Sync {
    fn def(&self) -> ToolDef;
    async fn execute(&self, ctx: &ToolContext, args: Value) -> ToolOutput;
}

pub struct ToolContext {
    pub cwd: PathBuf,
    pub cancel: tokio_util::sync::CancellationToken,
    pub session_id: String,
}
```

Async from day one (bash timeouts, future web/MCP tools). Tools observe
cancellation — killing a turn kills its subprocess groups, no orphans.
`async_trait` over native AFIT so `dyn Tool` stays object-safe.

**Tool policy hook:** before each execute, core calls an injected
`ToolPolicy::check(def) -> Allow | Deny(reason)`. v0 default: allow-all
(`--yolo` makes it explicit). This is the seam for human-in-the-loop
confirmation and the consumer gateway's permission prompts later.

Builtins v0: `bash` (with timeout, configurable allowlist later), `read`
(offset/limit), `write`, `edit` (exact-match replace, must-match-or-fail),
`grep` (delegates to `rg` binary when present, falls back to std regex walk).

Tool failures return `ToolOutput::error(msg)` — the model sees the error and
can recover. A tool crashing the process is a bug.

### gray-session

- Storage behind `trait SessionStore: Send + Sync { append(id, entry); load(id) }`.
  Default impl: `JsonlSessionStore` writing `~/.gray/sessions/<id>.jsonl`, one
  entry per line. Gateways can inject Postgres/whatever without touching core.
- Sessions record **turn-level normalized `Message`s only** — never raw stream
  deltas or partial tool calls. A mid-turn abort is flushed as a coherent partial
  assistant message (text so far + complete tool_use blocks) so replay always
  yields provider-valid history (Anthropic requires matched
  `tool_use`/`tool_result` pairs). First line: metadata (model, cwd, ts, version).
- `resume(id)` replays into a fresh `Agent` message list.
- Context-window management is out of scope for v0, but the request builder in
  core takes a `token_budget: Option<usize>` — the compaction seam.
- No SQLite, no indexes in v0.

### gray (binary) — web-first

- `gray` → binds `127.0.0.1:7654` (configurable), serves the embedded UI and
  the JSON API below, opens nothing else. `--print "prompt"` stays for
  scripting/dev; a bare stdin REPL exists as fallback when no browser is around.
- HTTP API (all under `/api`):
  - `POST /api/chat` → SSE stream of core `AgentEvent`s (serde-json lines),
    request body `{session_id?, message}`; creates session when id omitted.
  - `GET /api/sessions` → list (id, title-ish first message, timestamp).
  - `GET /api/sessions/:id` → replayed normalized messages.
  - `DELETE /api/sessions/:id`.
  - `GET /api/config` → model name + provider (read-only, for the UI header).
- Static UI embedded via `rust-embed` from `web/dist` (Vite build output);
  `gray` ships as ONE self-contained binary with zero runtime assets.
- Flags: `--model provider/model-id`, `--port`, `--base-url`, `-p/--print`.
- Config precedence: flags > env > `~/.gray/config.toml`.

### web/ (frontend, extracted from gray-app)

- Vite + React + TypeScript, lifted component-by-component from
  `grayweb/gray-app/src/components/gray/chat/`: `view/ChatMessagesList`,
  `view/ChatMessageEditor`, `view/markdown/*`, `view/ThinkingBlock`,
  `view/useChatViewScroll`, `provider/useAutoStreamState`.
- Rewire data layer: delete Supabase/auth/reminders/payments modules;
  `lib/api/chatStream.ts`'s async-generator pattern is kept but pointed at
  `/api/chat` SSE with no auth header. Session CRUD hits `/api/sessions*`.
- Sidebar = flat session list (from gray-app's sidebar, simplified).
- Build: `npm run build` → `web/dist`, embedded by the binary at compile time.

## Data flow

```
stdin ──▶ Session.append(user)
      ──▶ Agent.loop:
             request = system_prompt + messages + tool_defs
             Provider.stream(request) ──▶ events ──▶ collector
             if tool_calls: Tools.execute() ──▶ Session.append(assistant+results) ──┐
             else: done ◀──────────────────────────────────────────────────────────┘

collector ──▶ two thin surfaces off the same event stream:
               • axum SSE handler   → browser UI (default)
               • stdout printer     → --print / REPL (dev)
```

## Error handling

| Failure | Behavior |
|---|---|
| Tool error | `ToolResult{is_error:true}` back to model |
| 429 / 5xx | provider retries w/ backoff, then surfaces `ProviderError::RateLimited` |
| 401 / bad key | fail fast with clear message |
| Malformed tool args from model | error result quoting the validation failure |
| Ctrl-C mid-turn | cancel token → abort stream, kill bash process group (`kill_on_drop(true)` + `process_group(0)`), flush coherent partial assistant msg to session |
| Destructive tool w/o approval | n/a in v0 (allow-all policy default); `ToolPolicy` seam exists |

## Error-type discipline

Library crates (`core`, `provider`, `tools`, `session`) use `thiserror`
enums only — no `anyhow`. The `gray` binary may use `anyhow`/`miette` for
CLI reporting. Keeps `dyn` boundaries typed and matchable.

## Testing strategy

- `gray-provider`: recorded SSE fixtures replayed via wiremock; both protocols;
  retry logic tested against a flaky mock.
- `gray-core`: scripted `FakeProvider` + `FakeTools` — full loop scenarios incl.
  multi-tool turns, tool-error recovery, max-turns guard.
- `gray-session`: golden-file round-trips; resume correctness.
- `gray` bin: clap arg parsing unit tests; REPL smoke test with piped stdin.

## Phase 2 seams (designed for, not built)

Priorities informed by hermes-agent analysis (see `docs/hermes-recon.md`,
`docs/hermes-steal-agy.md`). Steal the kernels, never the plumbing.

### v0 already carries three hermes-born details
- **3-tier system prompt** in core's request builder: stable (identity+tools)
  / context (memory, skills index) / volatile (session) sections.
- **Tool output spillover**: outputs over ~8KB go to
  `~/.gray/spill/<hash>.txt`; model gets `<persisted-output path=...>` + head/tail.
- **Tool error cap**: error text bounded (~2KB) before it enters history.

### Phase 2 (each is a small kernel)
- **MCP client:** just another `Tool` source feeding the registry — unchanged from earlier draft.
- **Skills:** `.gray/skills/<name>/SKILL.md` with YAML frontmatter; index
  (name+description) into system prompt; one `skill_view(name)` tool loads
  body. `/learn` command = static authoring-standards prompt as a normal turn;
  agent authors skills with ordinary file tools. This IS the self-improvement
  loop — no curator, no usage ledgers.
- **Memory:** `.gray/MEMORY.md` + `.gray/USER.md` injected verbatim (§-delimited
  sections); `memory(action=add/replace/remove)` tool; a 6-line memory-guidance
  paragraph in the system prompt. No prefetch layer.
- **Session search:** `rg` over session JSONL exposed as `session_search(query)`
  tool; optional single-table FTS5 sidecar only if rg proves too slow.
- **Subagents:** `delegate(task)` tool = child `Agent::run` with filtered
  registry (no delegate/memory/cron), final text as result; optional git
  worktree isolation per task.
- **Cron:** `~/.gray/jobs.json` + tokio interval ticker → headless Agent::run,
  output appended to a log. ~120 LOC.

### Deliberate skips (hermes evidence)
- Messaging gateways: hermes' gateway/run.py is 31k LOC for what gray's axum
  SSE API already does. Any future surface = new adapter normalizing to the
  same event stream; port nothing.
- SQLite state DB, provider plugin zoo, fuzzy patching, compaction machinery:
  all 10x-heavier than their ideas. Compaction when needed = threshold-triggered
  summarize-oldest-half call.
- **Consumer surface:** the local web UI is already the consumer face. When
  cloud hosting becomes affordable, put a reverse proxy + auth layer in front
  and swap `JsonlSessionStore` for a hosted one — no core changes. Swap
  `gray-tools` builtins for life tools (web, email, calendar) as needed.
