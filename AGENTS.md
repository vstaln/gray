# gray — Instructions for AI Agents

## What this project is
- **gray** is a minimal, modular agentic CLI (Rust, OpenAI-compatible). Crates: `gray` (REPL/TUI), `gray-core` (agent loop), `gray-provider` (SSE streaming), `gray-tools` (read/write/edit/bash/…), `gray-session` (JSONL), `gray-markdown` (vendored Grok markdown).
- Interactive REPL via `crates/gray/src/composer.rs` (`Tui` owns the bottom inline viewport, transcript via `Terminal::insert_before` scrollback). User box uses one blank margin top + bottom with `❯` prompt — keep it that way.
- Token footer logic: `gray-core` → `gray` via `AgentEvent::TurnEnd` → `tui.push_usage()` coalesced — don't duplicate gap logic.
- **`web/`** is the marketing + docs site for `gray.alignment.id` (Next.js static export). It is independent of the Rust workspace — `cargo` never builds it.

## When the user asks to "reference codex / pi / grok / prime-agent / whatever"
- They mean: **look at the local reference at `reference/` and copy/steal the relevant pattern into gray.
- Layout:
  - `reference/openai/codex` — Codex RS TUI (`codex-rs/tui`)
  - `reference/pi-mono` — pi `packages/agent` + `packages/ai` (TypeScript agent loop)
  - `reference/xai-org/grok-build` + `reference/gray-build` — Grok (`xai-grok-pager`, `xai-grok-markdown/streaming.rs`)
  - Other dirs under `reference/` are context, not gospel.
- How: read the file directly (e.g. `reference/openai/codex/...`, `reference/pi-mono/packages/agent/src/...`, `reference/xai-org/grok-build/crates/...`). Quote file paths in reasoning.
- Don't propose "search the web" — source is on disk.

## Build & Binary Installation
- After modifying code in `crates/gray` or other crates, always compile the release binary and copy/install it to the user's binary path:
  ```bash
  cargo build --release
  install target/release/gray ~/.local/bin/gray
  ```
- Always commit code changes.

## Website (`web/`)
- Next.js 16 App Router, Tailwind v4, `output: 'export'`. Run `pnpm dev` from `web/`; CI runs `pnpm lint` and `pnpm build`.
- The export in `web/out` is rsynced into `/var/www/gray/`, the **same docroot** that serves `/install.sh`, `/install.ps1` and `/dl/*`. `scripts/deploy-web.sh` excludes and protects those paths and then re-verifies them — never deploy the site any other way, and never add a route or asset that would sit at one of those paths.
- Backgrounds are baked, not live: `web/scripts/process-images.py` writes exactly the files the site references into `web/public/space`. There are no WebGL shaders. If you add or drop an image, update that script so its output set still matches the committed assets.
- Fonts are self-hosted and SIL OFL 1.1 (`web/scripts/fetch-fonts.py`), with licenses committed beside them. Do not hot-link a font CDN or add a face without a redistributable license.
