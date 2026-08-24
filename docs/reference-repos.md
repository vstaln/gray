# Reference repos worth stealing from

Verified 2026-08-24 via GitHub API: licenses read from `Cargo.toml` where
GitHub's UI shows "Other" for dual licenses. Stars approximate.

## Provider / SSE layer

| Repo | License | ⭐ | Steal |
|---|---|---|---|
| [64bit/async-openai](https://github.com/64bit/async-openai) | MIT | 2.0k | Canonical serde structs; chunked tool-call argument concatenation across frames |
| [cortesi/misanthropy](https://github.com/cortesi/misanthropy) | MIT | 34 | Lean handwritten Anthropic Messages SSE parser, no OpenAPI-generator bloat |
| [jeremychone/rust-genai](https://github.com/jeremychone/rust-genai) | Apache-2.0/MIT | 859 | Unified StreamEvent mapper translating both protocols' deltas into common text/tool events |
| [YumchaLabs/siumai](https://github.com/YumchaLabs/siumai) | Apache-2.0/MIT | 29 | Workspace split per-protocol crates with typed SSE delta accumulators |
| [jpopesculian/eventsource-stream](https://github.com/jpopesculian/eventsource-stream) | MIT/Apache-2.0 | 39 | The SSE adapter we already specced — byte stream → SSE frames |

## Agent loop / harness

| Repo | License | ⭐ | Steal |
|---|---|---|---|
| [openai/codex](https://github.com/openai/codex) (`codex-rs`) | Apache-2.0 | 90.6k | Typed ClientEvent/ServerEvent split between tool runner and planner; sandboxing module (Seatbelt/bwrap/seccomp) when gray grows one |
| [sigoden/aichat](https://github.com/sigoden/aichat) | Apache-2.0/MIT | 10.4k | JSON schema generation from Rust types; stdio capture; inline crossterm streaming renderer that doesn't hijack the alternate screen |
| [tenxhq/tenx](https://github.com/tenxhq/tenx) | MIT | 39 | Minimal ReAct loop w/ transactional patches, git checkpoints, non-blocking subprocess tools |
| block/goose | Apache-2.0 | ~20k | Cancellation-token-aware step execution; session metadata envelope |

## TUI rendering (phase 2)

| Repo | License | ⭐ | Steal |
|---|---|---|---|
| [Canop/termimad](https://github.com/Canop/termimad) | MIT | 1.2k | Terminal markdown styling without full ratatui runtime |
| [joshka/tui-markdown](https://github.com/joshka/tui-markdown) | Apache-2.0/MIT | 115 | pulldown-cmark → ratatui Text/Span conversion |

## Sessions

| Repo | License | ⭐ | Steal |
|---|---|---|---|
| badlogic/pi-mono (`pi-agent-core`) | MIT | 96k (monorepo) | JSONL log schema with id/parentId pointers — gives branching/compaction later without format migration |

## Sandbox / approval (phase 2+)

| Repo | License | ⭐ | Steal |
|---|---|---|---|
| [landlock-lsm/rust-landlock](https://github.com/landlock-lsm/rust-landlock) | MIT/Apache-2.0 | 304 | Unprivileged Linux fs scoping — restrict writes to workspace dir |
| [bytecodealliance/cap-std](https://github.com/bytecodealliance/cap-std) | Apache-2.0/MIT | 815 | Capability-based fs preventing symlink traversal escapes in tools |
| containers/bubblewrap | LGPL (pattern only) | 5.3k | The bwrap flag recipe to shell out to, don't link |

## Excluded on license

- zed-industries/zed assistant crate — GPL-3.0

## Added 2026-08-24 (second recon round)

| Repo | License | ⭐ | Steal |
|---|---|---|---|
| deepseek-ai/deepseek-harness | MIT | 189k | One-line tool registration; ordered PromptSection vec; 1-line default identity; `isConcurrencySafe` flag; spill policy as separable policy |
| badlogic/pi-mono (core) | MIT | 96k | ~350-token system prompt pattern; in-place head/tail truncation (2000 lines/50KB); id/parentId session tree; flat hook seams |

Full analyses: `docs/pi-recon.md`, `docs/dsh-recon.md`, `docs/hermes-recon.md`.
