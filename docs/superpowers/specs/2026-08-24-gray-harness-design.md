# gray — minimal modular agent harness (design)

Date: 2026-08-24
Status: approved direction, awaiting implementation plan

## What gray is

A minimal, modular coding-agent harness written in Rust — the philosophy of
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

- No TUI beyond a clean streaming REPL (ratatui is phase 2+)
- No MCP, no subagents, no skills directory, no gateway
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
    ├── gray-session/       # JSONL session persistence
    └── gray/               # binary: CLI + REPL
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
- Retries 429/5xx with exponential backoff + jitter (3 attempts default);
  auth errors fail fast.
- Keys from env: `GRAY_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`;
  base URL override per config.

### gray-tools

```rust
trait Tool {
    fn def(&self) -> ToolDef;                       // name + json schema
    fn execute(&self, args: Value) -> ToolOutput;   // never panics; errors are data
}
```

Builtins v0: `bash` (with timeout, configurable allowlist later), `read`
(offset/limit), `write`, `edit` (exact-match replace, must-match-or-fail),
`grep` (delegates to `rg` binary when present, falls back to std regex walk).

Tool failures return `ToolOutput::error(msg)` — the model sees the error and
can recover. A tool crashing the process is a bug.

### gray-session

- Append-only JSONL at `~/.gray/sessions/<id>.jsonl`; one event per line;
  first line records metadata (model, cwd, timestamp, version).
- `resume(id)` replays into a fresh `Agent` message list.
- No SQLite, no indexes in v0.

### gray (binary)

- Streaming REPL: prints deltas live, tool calls rendered compactly.
- Flags: `--model provider/model-id`, `-c/--continue` (latest session),
  `-r/--resume <id>`, `--base-url`, `-p/--print` (one-shot, non-interactive).
- Config precedence: flags > env > `~/.gray/config.toml`.

## Data flow

```
stdin ──▶ Session.append(user)
      ──▶ Agent.loop:
             request = system_prompt + messages + tool_defs
             Provider.stream(request) ──▶ events ──▶ terminal + collector
             if tool_calls: Tools.execute() ──▶ Session.append(assistant+results) ──┐
             else: done ◀──────────────────────────────────────────────────────────┘
```

## Error handling

| Failure | Behavior |
|---|---|
| Tool error | `ToolResult{is_error:true}` back to model |
| 429 / 5xx | provider retries w/ backoff, then surfaces `ProviderError::RateLimited` |
| 401 / bad key | fail fast with clear message |
| Malformed tool args from model | error result quoting the validation failure |
| Ctrl-C mid-turn | abort stream, keep partial assistant msg in session |

## Testing strategy

- `gray-provider`: recorded SSE fixtures replayed via wiremock; both protocols;
  retry logic tested against a flaky mock.
- `gray-core`: scripted `FakeProvider` + `FakeTools` — full loop scenarios incl.
  multi-tool turns, tool-error recovery, max-turns guard.
- `gray-session`: golden-file round-trips; resume correctness.
- `gray` bin: clap arg parsing unit tests; REPL smoke test with piped stdin.

## Phase 2 seams (designed for, not built)

- Skills: `.gray/skills/*.md` appended to system prompt (hermes-style learning loop lives here later)
- MCP client: just another `Tool` source feeding the registry
- Subagents: an `agent` tool wrapping a nested `Agent::run`
- Gateway/TUI: consume `Agent`'s event stream instead of stdout
