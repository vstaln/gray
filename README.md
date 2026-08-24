# gray

A minimal, modular agent harness in Rust. pi's philosophy, hermes' soul.

```
you run `gray`  →  a local web app opens  →  you talk, it works.
```

## Status: design phase

The full design lives in [`docs/superpowers/specs/2026-08-24-gray-harness-design.md`](docs/superpowers/specs/2026-08-24-gray-harness-design.md).
Research notes in `docs/` (`pi-recon.md`, `dsh-recon.md`, `hermes-recon.md`,
`reference-repos.md`, `system-prompt.md`).

## Shape (planned)

```
crates/
├── gray-core        event-driven ReAct loop; no I/O of its own
├── gray-provider    OpenAI-compatible + Anthropic wire protocols, SSE streaming
├── gray-tools       bash / read / write / edit / grep behind an async Tool trait
├── gray-session     SessionStore trait + append-only JSONL tree
└── gray             axum server on localhost, embedded chat UI, --print mode
web/                 Vite+React UI lifted from gray-app's chat components
```

Design principles:

- **Minimal core, hard walls.** Core knows nothing about HTTP, terminals, or files.
- **Tiny prompt.** One identity line + Muse Code engineering conventions (~700 tokens total).
- **Errors are data.** A failing tool is a message to the model, never a crash.
- **Steal kernels, not frameworks.** See `docs/hermes-steal-agy.md`.
