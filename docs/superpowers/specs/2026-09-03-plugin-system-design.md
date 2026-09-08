# Gray Plugin System — Design

Date: 2026-09-03. Status: approved in chat. Both phases in scope.

## Goal

Everything is a plugin. Best of both worlds:
- pi: one typed `Plugin` API (tools + providers + events + commands).
- dsh: profile-ordered composition with inspectable merged manifest.

## Phase 1 — In-process plugins (now)

New crate `gray-plugin` owning:

```rust
trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
    fn provider(&self) -> Option<Arc<dyn Provider>>;
    async fn on_event(&self, e: &CoreEvent) -> Option<CoreEvent>;
}
```

- `async on_event` from day one so phase 2 sidecars share the signature.
- Reuse existing seams as-is: `gray-tools::Tool`, `gray-core::{Provider, ToolExecutor}`.
- Built-ins (`read/write/edit/bash/grep/find/ls`, `openai` provider) become
  `Plugin` impls; `Registry::builtin()` becomes "load profile order, collect".
- `gray.yml` profile: ordered list of builtin plugin names only.
- `gray --dump-manifest` prints the merged tree.

Out of scope phase 1: sidecars, WASM, hot-reload, patch files.

## Phase 2 — Sidecar plugins (same spec, second runtime)

- `SidecarPlugin` implements the same `Plugin` trait; the loop never knows.
- Protocol: JSON-RPC over stdio with `plugin/manifest`, `tool/list`,
  `tool/call`, `event/notify`. Any language can ship `gray-plugin-*`.
- `gray.yml` gains `sidecar:` entries (path or argv).
- Hook timeout default 5s; timeout/crash degrades to skip + warning.
  Plugin crash = failed tool call, never a dead CLI.

## Composition

Boot: read profile, load plugins in order, merge tool defs + providers +
event subscriptions. Later entries win on name conflict. Load failure
fails boot naming the entry. No live reload (restart picks up changes).

## Events

`pre-step` + `pre-tool` are waterfalls (may rewrite/reject/deny);
`post-tool` + `turn-end` are notifies. Sync-looking hooks are all async.

## Errors

Tool failures stay data (`ToolOutput::error`). Hook failures skip.
Boot failures are fatal with the entry named.

## Testing

- Phase 1: manifest ordering/override unit tests, fake-plugin registry test.
- Phase 2: stub sidecar script for list/call, timeout-kill, crash-continue.
- `cargo check` clean; no new test framework.

## Phase 2 upgrade path (deferred, not designed)

WASM runtime behind the same trait if sandboxing ever matters.
