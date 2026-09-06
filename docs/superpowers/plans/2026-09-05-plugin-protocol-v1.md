# Plugin protocol v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make gray sidecars actually work: real tool schemas, three new host→plugin hooks, out-of-order replies — plus a 30-line shell reference plugin. No memory, no new plugins, no gateway changes.

**Architecture:** Task 1 rebuilds the transport inside `gray-plugin` (reader task + ToolDefs + 3 methods). Task 2 wires the three hooks into `gray-core`/`gray` behind the exact method contract below. File-disjoint; may run in parallel.

**Tech Stack:** Rust edition 2024, tokio (oneshot + spawn already in tree), NDJSON over stdio (unchanged).

**Spec:** `/tmp/opencode/hermes-gray/src/data.ts` (protocol + PR 1 sections) and App.tsx "Sidecar protocol v1" section. Verified against repo 2026-09-05: `run_inner` builds `ChatRequest{system}` at `gray-core/src/agent.rs:418-419`; REPL unknown-command at `gray/src/repl/mod.rs:2771`; sidecar state is one `Mutex<State>` (`gray-plugin/src/sidecar.rs:36-44`); `SidecarTool::def()` returns `json!({})` (`sidecar.rs:201`).

## Global Constraints

- NDJSON-over-stdio transport stays — a plugin must remain writable as a shell script.
- No new dependencies. No behavior change for builtin tools or existing sidecars that only use `tool/call` + `event/notify`.
- `cargo test --workspace --quiet` + `cargo clippy --workspace --quiet` green, zero new warnings.
- Work on branch `fix/plugin-protocol-v1`, never `main`. No pushes/tags without human approval.

## Shared contract (both tasks — exact, do not renegotiate)

- `plugin/manifest` result: `{"name","version","tools":[{"name","description","parameters":{JSON schema},"snippet"}],"commands":["/x"],"hooks":["prompt/context","tool/before","turn/end"]}`. `Manifest.tools` becomes `Vec<ToolDef>`, not `Vec<String>` (see `gray-plugin/src/lib.rs:25`).
- `prompt/context` req `{"id":N,"method":"prompt/context","params":{"cwd":"…"}}` → `{"id":N,"result":{"text":"…"}}`. Called once per turn before the request is built; all replies concatenated onto the system prompt.
- `tool/before` req `{"id":N,"method":"tool/before","params":{"name":"…","args":{…}}}` → `{"decision":"allow"}` | `{"decision":"deny","reason":"…"}` | `{"decision":"modify","args":{…}}`. Deny pushes an `is_error` tool result with the reason (the loop already handles that path).
- `command/run` req `{"id":N,"method":"command/run","params":{"name":"/x","argv":[…]}}` → `{"id":N,"result":{"text":"…"}}`. REPL forwards unknown `/x` to whichever manifest claimed it.
- Transport: one reader task per sidecar parses lines into `HashMap<u64, oneshot::Sender<Value>>`; writers take a short stdin lock only; per-request timeouts stay; `event/notify` keeps its fire-and-forget shape.

---

### Task 1: Sidecar transport — schemas, 3 methods, reader task, echo plugin

**Files:**
- Modify: `crates/gray-plugin/src/sidecar.rs` (SidecarTool::def schemas+snippet, 3 request methods, reader task, drop single RPC mutex)
- Modify: `crates/gray-plugin/src/lib.rs` (`Manifest.tools: Vec<ToolDef>`, merge_manifests owner logic per tool name — keep "later manifests win")
- Create: `plugins/echo/` — 30-line POSIX shell reference plugin answering `plugin/manifest` + `command/run` (`/echo`)
- Test: extend `crates/gray-plugin` tests (sidecar harness with script fixture)

**Interfaces:**
- Consumes: shared contract above.
- Produces: `SidecarPlugin` speaks all 6 methods; `plugins/echo` passes a manifest round-trip test.

- [ ] **Step 1: Manifest carries ToolDefs**

Change `Manifest.tools` to `Vec<ToolDef>` (with `prompt_snippet` so sidecar tools appear in the Available-tools block — the current gap). Fix `merge_manifests` + sidecar manifest parsing accordingly. Failing test first: manifest JSON with a tool schema parses and its def round-trips name/description/parameters.

- [ ] **Step 2: Reader task + out-of-order replies**

Spawn one reader task per sidecar resolving `HashMap<u64, oneshot::Sender<Value>>`; remove the single `Mutex<State>` RPC serialization (keep a short stdin lock for writers only). Failing test first: two concurrent `tool/call`s where the first reply arrives second — both resolve correctly (the existing hang-fixture test at `sidecar.rs:277-282` must keep passing: notify never blocks).

- [ ] **Step 3: Add prompt/context, tool/before, command/run**

Implement the three request methods per the shared contract (timeouts per request, same as `tool/call`). Unit-test each against a stub script.

- [ ] **Step 4: plugins/echo reference**

POSIX sh, ~30 lines: read NDJSON lines, answer `plugin/manifest` (one tool `echo` with schema + `/echo` command), answer `command/run` by joining argv. Integration test: boot it as a real sidecar, assert manifest + command round-trip.

- [ ] **Step 5: Verify + commit**

Run: `cargo test -p gray-plugin --quiet && cargo clippy -p gray-plugin --quiet`
```bash
git add crates/gray-plugin/ plugins/echo/
git commit -m "feat(plugin): protocol v1 transport — schemas, 3 hooks, reader task, echo reference"
```

---

### Task 2: Core wiring — prompt concat, tool veto, unknown-/cmd forward

**Files:**
- Modify: `crates/gray-core/src/agent.rs` (`run_inner`: concat `prompt/context` replies onto `self.system` before building `ChatRequest` (~:418); call `tool/before` before the executor call; deny → `is_error` tool result; `modify` → rewritten args)
- Modify: `crates/gray/src/repl/mod.rs` (unknown `/cmd` at ~:2771 → owning plugin's `command/run`; claimed commands appear in `/help`)
- Test: `crates/gray-core` + repl tests with a stub plugin (deny/modify/prompt-text assertions)

**Interfaces:**
- Consumes: shared contract above (method names/shapes — Task 1 implements the same contract; parallel-safe).
- Produces: hooks fire in the loop; existing behavior unchanged when no plugin claims a hook/command.

- [ ] **Step 1: prompt/context concat (failing test first)**

Test: stub plugin returns fixed text; assert the built `ChatRequest.system` contains it. Implement: before `ChatRequest` construction, gather all `prompt/context` replies and append.

- [ ] **Step 2: tool/before veto (failing test first)**

Tests: deny → turn contains `is_error` tool result with the reason and the tool never executes; modify → executor receives rewritten args; no veto configured → zero behavior change. Implement at the pre-executor call site.

- [ ] **Step 3: unknown-/cmd forward (failing test first)**

Test: manifest claims `/echo`; REPL routes `/echo hi` to `command/run` and prints returned text; unclaimed `/nope` keeps the current unknown-command message. Implement at the ~:2771 site + `/help` listing.

- [ ] **Step 4: Verify + commit**

Run: `cargo test -p gray-core -p gray --quiet && cargo clippy --workspace --quiet`
```bash
git add crates/gray-core/ crates/gray/
git commit -m "feat(core): wire prompt/context, tool/before veto, plugin slash commands"
```
