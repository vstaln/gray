# Plan: add `/acp` to gray (turn gray into any ACP agent, t3code-style)

> **Audience:** an AI coding agent working inside `github.com/vstaln/gray` (Rust workspace).
> **Goal:** typing `/acp` opens a picker of external coding agents; `/acp <agent>` makes gray
> spawn that agent over the **Agent Client Protocol (ACP)** and route every subsequent prompt
> through it, rendering its output in gray's normal TUI. `/acp off` returns to gray's native loop.
> This is the same mechanism T3 Code (`github.com/pingdotgg/t3code`) uses to drive Cursor,
> Grok Build, Codex, Claude Code, OpenCode and Antigravity.

---

## 0. Mental model (read this first)

### What ACP is
ACP (https://agentclientprotocol.com) is JSON-RPC 2.0 over **stdio, newline-delimited JSON**.
The *client* (an editor, T3 Code, or — after this work — gray) spawns the *agent* as a child
process and calls:

| Direction | Method | Purpose |
|---|---|---|
| client → agent | `initialize` | negotiate `protocolVersion: 1`, exchange capabilities, get `authMethods` |
| client → agent | `authenticate` | only if the agent requires it (`methodId` from `authMethods`) |
| client → agent | `session/new` `{cwd, mcpServers}` | returns `sessionId` (+ optional `modes`, `models`, `configOptions`) |
| client → agent | `session/load` / `session/resume` | reopen an existing agent session (if `agentCapabilities.loadSession`) |
| client → agent | `session/prompt` `{sessionId, prompt: ContentBlock[]}` | run one turn; resolves with `{stopReason}` when the turn ends |
| client → agent | `session/cancel` (notification) | interrupt the running turn; the pending prompt resolves with `stopReason: "cancelled"` |
| client → agent | `session/set_mode`, `session/set_config_option` | change permission mode / model / thought level |
| agent → client | `session/update` (notification) | streaming events: `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`, `available_commands_update`, `current_mode_update`, `config_option_update`, `usage_update` |
| agent → client | `session/request_permission` | agent asks before a risky tool call; client answers with an `optionId` |
| agent → client | `fs/read_text_file`, `fs/write_text_file` | agent delegates file I/O to the client (only if client advertised `fs` capability) |
| agent → client | `terminal/create|output|wait_for_exit|kill|release` | agent delegates shell to the client (only if client advertised `terminal: true`) |

### How t3code does it (what we are copying)
| t3code file | Responsibility | gray equivalent we will create |
|---|---|---|
| `packages/effect-acp/src/{protocol,client,rpc,_internal/stdio}.ts` | generic ACP client: JSON-RPC framing, request/notification routing, typed schema | `crates/gray-acp/src/{transport,client}.rs` (or the official `agent-client-protocol` crate) |
| `apps/server/src/provider/acp/AcpSessionRuntime.ts` | spawn process, `initialize` → `session/new|load`, prompt queue, cancel with timeout, stderr draining (bounded 32 KiB chunks), startup metadata capture | `crates/gray-acp/src/session.rs` |
| `apps/server/src/provider/acp/AcpRuntimeModel.ts` | parse `session/update` into normalized events; merge `tool_call_update` into tracked tool-call state; throttle progress emission | `crates/gray-acp/src/events.rs` (→ `gray_core::event::AgentEvent`) |
| `apps/server/src/provider/acp/AcpAdapterSupport.ts` | map approval decision → `allow-always` / `allow-once` / `reject-once`; map process-exit vs request errors | `crates/gray-acp/src/permission.rs` |
| `apps/server/src/provider/acp/CursorAcpSupport.ts`, `GrokAcpSupport.ts` | per-agent spawn command/args/env + `authMethodId` | `crates/gray-acp/src/registry.rs` (table of known agents) |
| `apps/server/scripts/acp-mock-agent.ts` | fake ACP agent for tests | `crates/gray-acp/tests/mock_agent/` |

Concrete spawn recipes lifted from t3code (verify against each CLI's `--help` before shipping):

| agent key | command | args | auth method id | notes |
|---|---|---|---|---|
| `cursor` | `cursor-agent` | `[-e <endpoint>] [--auto-review \| --force] acp` | `cursor_login` | `--auto-review` = auto mode, `--force` = full access |
| `grok` | `grok` | `[--permission-mode default\|acceptEdits\|auto] agent stdio` or `agent --always-approve stdio` | `xai.api_key` if `XAI_API_KEY` set, else `cached_token` | sets env `GROK_OAUTH2_REFERRER=gray` |
| `codex` | `npx` | `-y @zed-industries/codex-acp` (or `codex-acp` binary if on PATH) | from `authMethods` | uses `codex login` credentials |
| `claude` | `npx` | `-y @zed-industries/claude-code-acp` (or `claude-code-acp` on PATH) | from `authMethods` | uses Claude Code login |
| `gemini` | `gemini` | `--experimental-acp` | from `authMethods` | reference ACP implementation |
| `opencode` | `opencode` | `acp` | from `authMethods` | |
| `copilot` | `copilot` | `--acp` | from `authMethods` | |
| `goose` / `kimi` / `kiro` | `<bin>` | `acp` | from `authMethods` | |
| `<custom>` | user-defined | user-defined | user-defined | read from `~/.gray/acp.json` in Zed's `agent_servers` format |

### The one architectural decision
gray's `gray_core::agent::Agent` owns the *loop*: it calls a `Provider` for tokens and a
`ToolExecutor` for tools. An ACP agent owns **its own loop and its own tools**. So ACP must
**not** be implemented as a `Provider` (gray would try to execute the agent's tool calls, and
`AgentEvent::ToolCall*` are emitted by gray's loop, not by providers).

Instead introduce a **backend seam one level up**, at the point where the REPL calls
`agent.run_streaming(user_msg, ctx, &mut on_event)`:

```rust
// crates/gray/src/backend.rs (new)
pub enum Backend {
    Native(gray_core::agent::Agent),
    Acp(gray_acp::AcpSession),
}

impl Backend {
    pub async fn run_streaming(
        &mut self,
        input: gray_core::message::Message,
        ctx: gray_core::agent::ToolContext,
        on_event: &mut dyn FnMut(&gray_core::event::AgentEvent),
    ) -> Result<Vec<gray_core::event::AgentEvent>, gray_core::error::CoreError> { /* delegate */ }

    pub fn label(&self) -> String { /* "openai/gpt-5" or "acp:claude" */ }
    pub fn is_acp(&self) -> bool { … }
}
```

Both arms emit the **same `AgentEvent` enum** (`gray-core/src/event.rs`), so
`dispatch_agent_event(...)` in `crates/gray/src/repl/mod.rs`, the transcript renderer, usage
accounting and session persistence keep working untouched.

---

## 1. Deliverables checklist

- [ ] New crate `crates/gray-acp` (ACP client, registry, event mapping, permission mapping, mock agent for tests)
- [ ] `Backend` enum in `crates/gray/src/backend.rs`; REPL holds `Option<Backend>` instead of `Option<Agent>`
- [ ] `/acp` command: registry row, `ReplCommand::Acp`, arg completion, handler, picker UI
- [ ] `--acp <agent>` CLI flag (works with `-p` print mode too)
- [ ] Permission requests → gray's existing question/allow UI; `fs/*` callbacks → local fs
- [ ] Ctrl-C → `session/cancel`; process exit → error + auto-fallback to native
- [ ] Session persistence: ACP `{agent, session_id}` stored in the JSONL session so `/resume` can `session/load`
- [ ] Status line / prompt shows `acp:<agent>` instead of model
- [ ] `/model`, `/thinking`, `/compact`, `/new`, `/usage` adapted while in ACP mode
- [ ] Tests: unit (parsing/mapping), integration (mock agent), `commands.rs` registry tests updated
- [ ] README: new `/acp` row in the Commands table + "ACP agents" section

---

## 2. Phase 0 — Spike (30 min, no code committed)

1. Install one ACP agent locally, e.g. `npm i -g @zed-industries/codex-acp` (or `gemini`).
2. Speak the protocol by hand to confirm framing:
   ```bash
   printf '%s\n' \
     '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":false},"clientInfo":{"name":"gray","version":"dev"}}}' \
     '{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"'"$PWD"'","mcpServers":[]}}' \
   | codex-acp
   ```
3. Note: one JSON object per line, no `Content-Length` headers. stderr is free-form logging
   and **must be drained** or the child blocks.
4. Decide SDK vs hand-rolled transport (see §3.1). Record the decision in the PR description.

---

## 3. Phase 1 — `crates/gray-acp`

### 3.1 Dependencies
Add to workspace `Cargo.toml` members: `"crates/gray-acp"`, and `gray-acp = { path = "crates/gray-acp" }`.

Two options for the protocol layer — pick **A** unless it fights the runtime:

**A. Official Rust SDK** — `agent-client-protocol = "2.1"` (Zed's crate; `Client` trait +
`ClientSideConnection`, all schema types generated). **Gotcha:** the SDK's connection futures are
`!Send` (it uses `Rc`/`LocalSet`). gray runs a multi-thread tokio runtime, so run the SDK on a
dedicated OS thread with `tokio::runtime::Builder::new_current_thread()` + `tokio::task::LocalSet`,
and bridge to the rest of gray with `tokio::sync::mpsc` / `oneshot` channels. Wrap that in
`AcpSession` so the REPL never sees the `!Send` types.

**B. Hand-rolled** — depend only on `agent-client-protocol-schema = "2.1"` for the serde types
and write ~300 lines of JSON-RPC: a writer task (mpsc → stdin), a reader task (stdout lines →
route by `id` to pending `oneshot`s, or by `method` to notification/request handlers). Fully
`Send`, no LocalSet dance. t3code's `packages/effect-acp/src/_internal/stdio.ts` is the reference.

Either way also add: `tokio` (process, io-util, sync), `serde`, `serde_json`, `thiserror`,
`futures`, `log`, `which = "7"` (binary discovery), `dirs` or reuse gray's `GRAY_HOME` helper.

### 3.2 Module layout
```
crates/gray-acp/
├── Cargo.toml
├── src/
│   ├── lib.rs           # pub use AcpSession, AgentSpec, registry, AcpError
│   ├── registry.rs      # known agents table + ~/.gray/acp.json overrides + `which` probing
│   ├── transport.rs     # (option B only) JSON-RPC over child stdio
│   ├── client.rs        # ClientHandler: request_permission, fs/*, terminal/* (stubbed), session/update fan-out
│   ├── session.rs       # AcpSession: spawn → initialize → authenticate? → session/new|load; prompt(); cancel(); shutdown()
│   ├── events.rs        # SessionUpdate → Vec<AgentEvent>; tool-call state merge; stopReason → StopReason
│   ├── permission.rs    # PermissionRequest → gray question; decision → optionId (allow-always/allow-once/reject-once)
│   └── error.rs         # AcpError { Spawn, ProcessExited{code, stderr_tail}, Request{method, code, message}, AuthRequired{methods}, Timeout, Cancelled }
└── tests/
    ├── mock_agent/main.rs   # tiny ACP agent binary (see §3.7)
    ├── session.rs           # spawn mock, prompt, assert event sequence
    ├── cancel.rs
    └── permission.rs
```

### 3.3 `registry.rs`
```rust
pub struct AgentSpec {
    pub key: &'static str,            // "claude"
    pub display: &'static str,        // "Claude Code"
    pub command: String,              // resolved binary or "npx"
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub auth_method_hint: Option<&'static str>, // t3code's authMethodId
    pub install_hint: &'static str,   // shown when `which` fails
}
pub fn builtin() -> Vec<AgentSpec>;                     // table from §0
pub fn load_user_agents(gray_home: &Path) -> Vec<AgentSpec>; // ~/.gray/acp.json, Zed `agent_servers` shape
pub fn resolve(name: &str) -> Option<AgentSpec>;         // user overrides win, then builtin, case-insensitive
pub fn installed(spec: &AgentSpec) -> bool;              // which::which(&spec.command).is_ok()
```
Prefer a native binary when present (`codex-acp`, `claude-code-acp`) and fall back to `npx -y …`;
if neither node nor the binary exists, mark not-installed with `install_hint`.

### 3.4 `session.rs` — `AcpSession`
```rust
pub struct AcpSessionOptions {
    pub spec: AgentSpec,
    pub cwd: PathBuf,
    pub resume_session_id: Option<String>,
    pub auto_approve: bool,                 // GRAY_ACP_AUTO_APPROVE=1 or `/acp <agent> --yolo`
    pub permission_prompt: Arc<dyn PermissionPrompt>, // implemented by the REPL
    pub session_load_timeout: Duration,     // t3code: 90 s
    pub cancel_timeout: Duration,           // t3code: 15 s
}

pub struct AcpSession { /* child, handles, session_id, capabilities, modes, models, available_commands, usage */ }

impl AcpSession {
    pub async fn start(opts: AcpSessionOptions) -> Result<Self, AcpError>;
    pub async fn prompt(&mut self, blocks: Vec<ContentBlock>, on_event: &mut dyn FnMut(&AgentEvent)) -> Result<StopReason, AcpError>;
    pub async fn cancel(&self);                       // sends session/cancel; prompt() then returns Cancelled
    pub async fn new_session(&mut self) -> Result<(), AcpError>;   // for /new while in ACP mode
    pub async fn set_mode(&mut self, mode_id: &str) -> Result<(), AcpError>;
    pub async fn set_config_option(&mut self, id: &str, value: serde_json::Value) -> Result<(), AcpError>;
    pub fn session_id(&self) -> &str;
    pub fn agent_key(&self) -> &str;
    pub fn available_commands(&self) -> &[AvailableCommand];
    pub fn current_mode(&self) -> Option<&str>;
    pub fn models(&self) -> &[ModelInfo];   // from session/new response if agent provides
    pub async fn shutdown(self);              // close stdin, SIGTERM, kill after 5 s
}
```
Start sequence (mirror `AcpSessionRuntime.ts`):
1. Spawn with `cwd`, inherit env + `spec.env`, `stdin/stdout` piped, `stderr` piped and drained
   into a ring buffer (last ~8 KiB kept for error messages; bound each chunk at 32 KiB).
2. `initialize` with `protocolVersion: 1`, `clientCapabilities: { fs: { readTextFile: true, writeTextFile: true }, terminal: false }`,
   `clientInfo: { name: "gray", version: env!("CARGO_PKG_VERSION") }`.
3. If `authMethods` is non-empty and `session/new` later fails with `auth_required` (code `-32000`),
   call `authenticate { methodId }` using `spec.auth_method_hint` or the first method; if that fails,
   return `AcpError::AuthRequired` with the method names so the REPL can print "run `claude auth login`".
4. `session/load` if `resume_session_id` is set **and** `agentCapabilities.loadSession`, else `session/new`.
   During load, replayed `session/update`s arrive; treat them as history (t3code waits for a 2 s idle gap)
   and do **not** render them as a live turn.
5. Buffer any `session/update` that arrives before the REPL attaches a listener (t3code: `maxStartupMetadataUpdates = 32`).
6. Persist `available_commands_update`, `current_mode_update`, `config_option_update` into session state.

Only **one prompt in flight** per session (guard with a `Mutex`/flag; reject a second call).

### 3.5 `events.rs` — mapping to `gray_core::event::AgentEvent`

| ACP `session/update.sessionUpdate` | `AgentEvent` |
|---|---|
| `agent_message_chunk` `{content: text}` | `TextDelta { delta }` |
| `agent_message_chunk` with image/resource content | `TextDelta` with a short placeholder like `[image]` |
| `agent_thought_chunk` | `ThinkingDelta { delta }` |
| `tool_call` `{toolCallId, title, kind, rawInput, status}` | `ToolCallStart { id: toolCallId, name: kind-or-title }` then `ToolCallEnd { id, args: rawInput.unwrap_or(json!({"title": title})) }` |
| `tool_call_update` `{status: in_progress}` | nothing (or throttle progress; t3code emits at most every N chars) |
| `tool_call_update` `{status: completed \| failed, content, rawOutput}` | `ToolResult { id, output: flattened content/diff text, is_error: status == failed }` |
| `plan` | render as a single `TextDelta` with a markdown checklist (v1); optional `AgentEvent::Plan` in v2 |
| `usage_update` (unstable feature) | `StepUsage { usage }` |
| `available_commands_update`, `current_mode_update`, `config_option_update` | state only, no event |
| prompt response `stopReason` | `TurnEnd { stop_reason, usage }` where `end_turn→EndTurn`, `max_tokens→MaxTokens`, `cancelled→Cancelled`, `refusal→Error`, `max_turn_requests→EndTurn` |

Keep a `HashMap<toolCallId, ToolCallState>` and merge updates like `mergeToolCallState` in
`AcpRuntimeModel.ts` (later updates may carry only the changed fields). Emit `Start` at prompt
begin so the TUI shows the spinner.

### 3.6 `client.rs` — handling agent → client requests
- `session/request_permission { toolCall, options[] }` → build a gray question
  (reuse the same UI path as the bash destructive-command allow-prompt / `request_user_input` tool in
  `crates/gray/src/composer/question.rs`). Options are `{optionId, name, kind}` with kinds
  `allow_once | allow_always | reject_once | reject_always`. If `auto_approve`, pick the first
  `allow_always` else `allow_once`. If the turn was cancelled meanwhile, answer `{outcome: "cancelled"}`.
- `fs/read_text_file { path, line?, limit? }` → read from disk (reject paths outside `cwd`
  unless an env override is set), return `{content}`.
- `fs/write_text_file { path, content }` → write; same path guard.
- `terminal/*` → return JSON-RPC `-32601 method not found` in v1 (we advertise `terminal: false`).
  v2: implement with `tokio::process` + output ring buffer.
- Unknown `_ext/*` requests → `-32601`; unknown notifications → ignore + debug log.

### 3.7 Mock agent (`tests/mock_agent/main.rs`)
Same idea as t3code's `acp-mock-agent.ts`: a `[[bin]]` (test-only) that reads lines from stdin and
- answers `initialize` with capabilities, `authMethods: []`;
- answers `session/new` with a fixed `sessionId`, one `mode`, one `availableCommand` (`/compact`);
- on `session/prompt`: emits `agent_thought_chunk`, three `agent_message_chunk`s, a `tool_call` +
  `tool_call_update(completed)`, and — if the prompt text contains `PERMISSION` — first sends a
  `session/request_permission` and echoes the chosen `optionId`; if text contains `SLOW`, sleeps
  until `session/cancel` arrives then replies `stopReason: cancelled`; if text contains `CRASH`,
  `process::exit(3)`.
Integration tests spawn it via `env!("CARGO_BIN_EXE_mock_agent")`.

---

## 4. Phase 2 — REPL integration (`crates/gray`)

### 4.1 `crates/gray/src/repl/commands.rs`
1. Add to `REGISTRY` (keep alphabetical-ish grouping near `model`/`connect`):
   ```rust
   CmdDef { name: "acp", desc: "run as an external ACP agent (claude, codex, cursor…)", aliases: &[], args_hint: "" },
   ```
2. Add variant `ReplCommand::Acp(Option<String>)` with doc comment
   `/// External ACP agent: /acp (picker), /acp <agent> [--yolo], /acp off|status|list`.
3. In `parse_command`: `Some("acp") => ReplCommand::Acp(opt(rest)),`.
4. In `complete_command_args`: `"acp" => complete_acp_args(arg_text),` returning
   `off`, `status`, `list` plus every `gray_acp::registry` key (mark installed ones with a ✓ in the description).
5. Update tests: `registry_help_covers_all_commands`, `registry_completion_covers_aliases`,
   `registry_parse_uses_canonical`; add `acp_parse_variants` (`/acp`, `/acp claude`, `/ACP Codex --yolo`, `/acp off`).

### 4.2 `crates/gray/src/backend.rs` (new)
Implement the `Backend` enum from §0. `Native` delegates to `Agent::run_streaming`; `Acp` converts the
user `Message` into ACP `ContentBlock`s (text → `{type:"text"}`; image attachments from
`repl/attachments.rs` → `{type:"image", mimeType, data}` only if `promptCapabilities.image`), calls
`AcpSession::prompt`, and returns the collected events. Map `AcpError::Cancelled → CoreError::Cancelled`
and everything else to a `CoreError` variant that renders as a red error line.

### 4.3 `crates/gray/src/repl/mod.rs`
The REPL currently holds `let mut agent: Option<Agent>` and calls
`agent.run_streaming(user_msg, ctx, &mut on_event)` in two places (initial prompt and the
overflow-retry path). Do this:

1. Change to `let mut backend: Option<Backend>`; every `agent = Some(built...)` site wraps in `Backend::Native(...)`.
   Sites: `build_agent(...)` results (~lines 1730, 2151, 2183, 2455, 2688, 2886) and `handle_model`
   (~line 321/328, which rebuilds the agent on model switch).
2. Replace the two `agent.run_streaming(...)` calls with `backend.run_streaming(...)`.
   Skip the **overflow-compact-and-retry** branch when `backend.is_acp()` (the agent manages its own context).
3. Add `ReplCommand::Acp(arg) => { handle_acp(config, &cwd, arg, &mut backend, tui.as_ref().map(|(s,_)| s), &session_store).await; continue; }`.
4. `handle_acp` behaviour:
   - `None` (bare `/acp`) and interactive → open the picker (same widget as `/model` in `setup/ui.rs`):
     rows = registry entries, subtitle "installed" / "not found — <install_hint>", plus a final
     "gray (native)" row. Non-interactive → print the list.
   - `Some("list")` → print table. `Some("status")` → print agent, session id, mode, available commands.
   - `Some("off" | "native" | "gray")` → `shutdown()` the ACP session, rebuild native via `build_agent`, print "back to native".
   - `Some(name [--yolo])` → `registry::resolve`; if not installed print hint and return; else show a
     dim "starting <display>…" line, `AcpSession::start`, on success replace `backend` with
     `Backend::Acp(session)`, print `switched to acp:<key> (session <id-prefix>)`, and write the session
     meta record (§5.1). On `AuthRequired` print the login hint. On any error keep the native backend.
   - If already in ACP mode with a different agent → shut the old one down first.
5. **Status line**: wherever `config.model` is displayed (`~583`, `~1789`, `~1832`, `dispatch_agent_event` arg
   `config.model.as_deref()`), use `backend.label()` instead so the prompt shows `acp:claude`.
6. **Ctrl-C** while a turn runs: the existing `cancel` token path → in `Backend::Acp` also call
   `session.cancel()`; if the prompt hasn't resolved within `cancel_timeout`, kill and surface an error.
7. **Slash commands forwarded to the agent**: when `backend.is_acp()` and the input starts with `/` and is
   `ReplCommand::Unknown`, check `session.available_commands()`; if it matches, send it as a normal prompt
   (that is exactly what t3code does). Otherwise keep gray's "unknown command" message.
8. **Commands while in ACP mode**:
   - `/new` → `session.new_session()` (keep the agent), start a fresh gray JSONL session.
   - `/model` → if `session.models()` non-empty, show them and call `set_config_option("model", …)`;
     else print "model is controlled by <agent>; use its own /model command if it has one".
   - `/thinking <lvl>` → `set_config_option("thought_level", …)` when such an option exists, else notice.
   - `/compact` → forward `/compact` as a prompt if in `available_commands`, else notice.
   - `/context`, `/provider`, `/key` → notice: not applicable in ACP mode.
   - `/usage` → whatever `usage_update` provided; label it "reported by agent".
   - `/quit` → `shutdown()` before exit.
9. **Process death mid-turn** (`AcpError::ProcessExited`): print the stderr tail (redact obvious tokens),
   drop to native backend automatically, and tell the user.

### 4.4 `crates/gray/src/lib.rs` / `main.rs`
- Add `#[arg(long)] pub acp: Option<String>` to `Cli`; at REPL start, if set, run the same path as `/acp <name>`.
- Print mode (`-p`): if `--acp` is set, build `Backend::Acp` and stream text deltas to stdout exactly like the native path in `print.rs`.
- Persist last-used ACP agent in `~/.gray/config.json` (`"acp_agent": "claude"`) only when the user
  passes `--acp-default` / picks "make default" in the picker — do **not** auto-persist, otherwise
  gray would silently boot into an external agent.

---

## 5. Phase 3 — persistence, resume, polish

### 5.1 Session JSONL (`crates/gray-session`)
- Add a meta record type `{"type":"backend","kind":"acp","agent":"claude","session_id":"…","cwd":"…"}` written when
  switching agents / creating a new ACP session. Keep writing `user` / `assistant` text records as today so
  transcripts, `gray resume` pickers and `/usage` still work.
- `/resume` / `gray -c`: if the latest `backend` record is ACP, call `handle_acp` with
  `resume_session_id` → `session/load` (if supported) else `session/new` and print "agent could not
  restore its context; history shown is from gray's log only".

### 5.2 Config file `~/.gray/acp.json`
```json
{
  "agent_servers": {
    "my-agent": { "command": "/usr/local/bin/foo", "args": ["acp"], "env": { "FOO_TOKEN": "…" } }
  },
  "auto_approve": false
}
```
Same shape as Zed's settings so users can copy configs. File mode `0600` like `gateway.yaml`.

### 5.3 Docs
- README Commands table: `| /acp [agent] | become an external ACP agent (claude, codex, cursor, grok, gemini, opencode…) |`
- New README section "ACP agents": what it does, install hints per agent, `--acp` flag, `~/.gray/acp.json`,
  safety note (**the external agent's own permission model applies; gray's bash guard does not run**).
- `/help` output gets the new row automatically via `REGISTRY`.

---

## 6. Testing matrix

| Test | Where | Asserts |
|---|---|---|
| registry parsing/aliases/help/completion | `crates/gray/src/repl/commands.rs` | existing tests still pass with the new row; new `/acp` cases |
| `events.rs` mapping | `crates/gray-acp/src/events.rs` unit tests | each `sessionUpdate` → expected `AgentEvent`s; tool-call merge across partial updates; stopReason map |
| happy path | `crates/gray-acp/tests/session.rs` | spawn mock → `start()` ok → `prompt("hi")` yields `Start, ThinkingDelta, TextDelta×3, ToolCallStart, ToolCallEnd, ToolResult, TurnEnd(EndTurn)` |
| permission | `tests/permission.rs` | mock sends `request_permission`; fake `PermissionPrompt` returns allow-once; assistant echoes chosen optionId; reject path yields `ToolResult{is_error:true}` |
| cancel | `tests/cancel.rs` | prompt `SLOW`, call `cancel()` after 200 ms → returns `Cancelled` within `cancel_timeout` |
| crash | `tests/session.rs` | prompt `CRASH` → `AcpError::ProcessExited{code:3}` and stderr tail present |
| auth required | mock flag env `MOCK_ACP_REQUIRE_AUTH=1` | `session/new` → `-32000` → `authenticate` called → success |
| REPL smoke | `crates/gray` integration (piped stdin, non-interactive) | `/acp mock` (registered via `~/.gray/acp.json` pointing at the mock bin) → `hello` → text appears → `/acp off` → native again |
| manual | real agents | run `/acp codex`, `/acp claude`, `/acp cursor`, `/acp grok`, `/acp gemini`; confirm streaming, tool calls, permission prompt, Ctrl-C, `/new`, `/quit` cleanly kills child (`pgrep`) |

Run `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.

---

## 7. Gotchas & constraints (do not skip)

1. **Framing:** newline-delimited JSON, UTF-8, one message per line. No LSP-style headers.
2. **Drain stderr** always; agents log there and will deadlock if the pipe fills.
3. **Early notifications:** some agents emit `session/update` before `session/new` resolves; queue them.
4. **`!Send` SDK:** if using `agent-client-protocol`, isolate it on a `LocalSet` thread (§3.1 A).
5. **One prompt at a time** per session; gray's composer already blocks input while a task runs, keep it that way.
6. **Don't run gray's tools/hooks/system prompt in ACP mode.** The agent has its own; gray is only a UI.
7. **Cancellation:** send `session/cancel`, then *wait for the prompt response* (`cancelled`) — do not kill immediately (t3code `cancelBehavior: "wait-for-prompt"`, 15 s timeout).
8. **Auth:** never store agent credentials; rely on each CLI's own login. Surface `install_hint` and login hints.
9. **npx cold start** can take 10–30 s; show a spinner and use the 90 s load timeout.
10. **Path safety:** `fs/*` handlers must canonicalize and refuse paths outside `cwd` unless `GRAY_ACP_ALLOW_ANY_PATH=1`.
11. **Windows** is unsupported in gray anyway (WSL only) — no need for `.cmd` shims.
12. **Kill on exit:** `Drop`/`shutdown` must SIGTERM then SIGKILL the child; test with `pgrep` after `/quit`.
13. **Protocol version:** request `1`; if the agent answers a lower version, abort with a clear error.

---

## 8. Suggested commit sequence

1. `feat(gray-acp): scaffold crate, registry, error types, mock agent`
2. `feat(gray-acp): stdio JSON-RPC transport + AcpSession start/prompt/cancel/shutdown`
3. `feat(gray-acp): session/update → AgentEvent mapping with tests`
4. `feat(gray-acp): client handlers (request_permission, fs/*)`
5. `feat(gray): Backend enum; REPL uses Backend instead of Agent`
6. `feat(gray): /acp command, picker, completion, status label`
7. `feat(gray): --acp flag, print mode, session meta + resume`
8. `docs: README /acp section`

Each step must compile and pass `cargo test --workspace` on its own.
