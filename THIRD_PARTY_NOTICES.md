# Third-party notices

gray itself is MIT (see `LICENSE`). Cargo dependencies are covered by
`Cargo.lock`, not listed here.

| gray | upstream | license |
|---|---|---|
| `composer/text_area.rs`, `gray-core/event.rs`, `agent.rs`, `agent_loop.rs`, `compact_v2.rs`, `gray-provider/openai.rs`, `repl/format.rs`, `print.rs` | https://github.com/openai/codex | Apache-2.0 |
| `gray/src/compact/`, `composer/input/` (paste-collapse rule) | https://github.com/badlogic/pi-mono and `reference/prime-agent` | MIT |
| `gray-gateway/progress.rs`, `gray-supervise/exit.rs` | https://github.com/NousResearch/hermes-agent | MIT |
| `gray-core/parallel.rs` (parallel batch lane; design/constants parity with Toolrush `MAX_BATCH`/`MAX_WORKERS`, no verbatim code) | https://github.com/OnlyTerp/toolrush | MIT |
| `repl/attachments.rs`, `repl/format.rs`, skills-dir interop (parity targets) | https://github.com/sst/opencode | see upstream |

Codex compaction port: `codex-rs/core/src/compact_remote.rs` +
`compact_remote_v2.rs` + `compact_remote_v2_images.rs` @ upstream commit
`1fb5158b` → `gray-core/compact_v2.rs` (+ pipeline in `agent_compact.rs`).
