# lane8 DIG robustness + speed — analysis only (feat/batch-reunite @ ab5e6a9)

Isolated `GRAY_HOME=dogfood/lane8_home`, `GRAY_NO_UPDATE_CHECK=1` unless noted.
Binary `target/debug/gray` (298M, Sep 8). No commits, no `cargo test`, no real `~/.gray` writes.
429 avoidance: zero LLM calls — dummy provider `http://127.0.0.1:9` / `10.255.255.1:9`, local commands only.

## Numbers

| probe | result |
|---|---|
| cold-boot piped `echo /quit \| gray` (fresh home) ×3 | 30 / 38 / 36 ms |
| warm-boot piped ×3 | 30 / 23 / 27 ms |
| fresh TUI boot → provider picker | 217 ms, RSS ~31 MB |
| seeded TUI boot → banner ×3 | 360 / 602 / 495 ms |
| RSS idle seeded (banner, bg fetches done) | ~102 MB (range seen 62–111 MB depending on fetch state) |
| RSS after `/help` | 114 MB (+10 MB) |
| keystroke `hello` → visible | ~31 ms (20 ms poll granularity; single-frame echo) |
| paste 10k lines / 860 KB (`dogfood/lane8_paste10k.txt`) | visible 385 ms, RSS +4.5 MB (62.1→66.7 MB) |
| resume 2000 msgs / 669 KB session | 7317 ms to `⬢ Resumed`, RSS 119 MB (+18 MB vs idle) |
| idle capture diff 500 ms (big session) | 0 bytes (stable, no flicker) |
| idle capture diff 300 ms (paste resident) | 33 bytes (status/ticker repaint only) |
| resize storm (6 rapid `resize-window`) | survived, ~2.1 s; scrollback re-emitted |
| rapid Ctrl-C ×2 at prompt | exits (still alive at 0.8 s, dead by ~2 s — slow exit, see H9) |
| `kill -9` mid-turn (hanging provider) | no torn file; in-flight prompt lost; `resume --all` rc 0 |
| `-p` failed turn (dead port, 3 retries) | 1433 ms, +1838 B (gray.log 698 B + AGENTS.md 1140 B, **no session file**) |
| TUI failed turn | duration_ms 627 recorded; +289 B session + ~550 B log ≈ **~840 B/turn** (plus one-time ~313 KB `models.json` on fresh home) |
| real `~/.gray` (read-only `du`) | 225 M total: shell 208 M, sessions 16 M / 270 files, logs 592 K (`gray.log` 596 K), `models.json` 308 K, `openrouter_models.json` 96 K |

Notes: fresh-home TUI lands on the onboarding picker, not the banner — banner timing needs a seeded `config.json`.
`shell/out-2c-*` dirs (1.3 MB `t1.log` each, from Sep 5) dominate real shell usage; age-sweep keeps them until 7 days.

## Holes (file:line + severity, fix nothing)

- **H1 (high) `history_entries` unbounded; `transcript` capped — reflow/replay cost O(all history).**
  Cap applies to `transcript` only: `crates/gray/src/composer/transcript/mod.rs:216`,
  `crates/gray/src/composer/transcript/boxes.rs:9,19,157` (`drain(0..100)` past 1000 lines).
  `history_entries` is pushed everywhere and never trimmed:
  `boxes.rs:37` (ToolBox), `boxes.rs:151` (StyledLines), `transcript/mod.rs:50` (Gap),
  `transcript/mod.rs:185` (UserPrompt). `reflow_on_resize` re-emits **all** entries
  (`crates/gray/src/composer/mod.rs:263`) and `replay_session_history` (`boxes.rs:258`)
  replays all — measured 7.3 s for 2000 msgs. Every resize pays the same.
- **H2 (med) `gray.log` rotation checked once at boot only.**
  `crates/gray/src/logging.rs:129` calls `rotate_if_needed` (`crates/gray-supervise/src/rotation.rs:8`,
  cap `LOG_MAX_BYTES` 10 MB → `.1`/`.2`, 30 MB max) inside `init()`. A long-lived TUI session
  writing >10 MB never rotates until next boot.
- **H3 (low) `tool-stats.jsonl` has no rotation at all.**
  `crates/gray-tools/src/stats.rs:51` appends per tool call (gated by `GRAY_TOOL_STATS=1`);
  no cap/trim unlike update receipts (200 lines, `crates/gray/src/update.rs:108`).
- **H4 (high) shell `tN.log` files have no size cap; sweep is age-only (7 days).**
  Pump writes the full redacted stream: `crates/gray-tools/src/shell/pump.rs:266-276`.
  Sweep deletes `t*.log` older than 7 days only: `crates/gray/src/shell_drain.rs:193-228`.
  A chatty task under the 610 s tool timeout (`shell_drain.rs:32`) can write GBs;
  memory view is bounded (16 KB head + 48 KB tail, `contract.rs:24-25`) but the file is not.
  Real `~/.gray/shell` at 208 M is the shape of this.
- **H5 (med) `WAKE_QUEUE` unbounded.**
  `crates/gray/src/shell_drain.rs:34,45` — `Vec<String>` push with no cap; drained only at
  loop-top, and mid-turn exits stay queued. A chatty task during a long turn grows it.
  (Broadcast itself is capped 256 with a drop-line, `registry.rs:27` + `shell_drain.rs:121` — that half is fine.)
- **H6 (med) sessions dir never pruned; `list()` is O(all sessions, full reads).**
  `crates/gray-session/src/lib.rs:513` reads every `.jsonl` fully to build summaries
  (header + first-user scan); runs on the resume picker and `boot -c`. 270 files / 16 M today;
  growth is silent. Per-turn `append` (`lib.rs:371`) re-reads the whole file to compute
  `max_id` + fsyncs twice (`lib.rs:352,435`) — O(session length) per turn.
- **H7 (low) `models.json` rewritten on every boot with network, no TTL.**
  Spawned unconditionally: `crates/gray/src/repl/mod.rs:291-294` (+ `status.rs:320`, `handlers.rs:310,337`
  re-fetch on model change). Fetchers: `providers.rs:722,813,898`. Each success calls
  `save_models_cache_to_disk` (`providers.rs:952`, read-modify-write, no lock) — ~313 KB
  write per boot; concurrent gray processes can interleave.
- **H8 (info) `kill -9` mid-turn loses the in-flight user prompt by design.**
  Persist happens after the run: `crates/gray/src/repl/prompt_turn.rs:259-270` via
  `persist_turn_messages` (`repl/session.rs:190`). No torn file (verified), but the typed
  prompt is gone. Torn-final-line tolerance exists on load (`gray-session/src/lib.rs:492`).
- **H9 (low) two-press Ctrl-C exit is slow (>0.8 s observed).**
  Policy: `crates/gray/src/repl/mod.rs:54-81` (`CTRL_C_EXIT_WINDOW_MS` 5000, `:29`).
  Exit path does raw-mode teardown + shell shutdown (3 s deadline, `shell_drain.rs:28,182`)
  — fine, just don't mistake the pause for a hang.
- **H10 (low) `startup_check` awaits up to 1500 ms before the REPL starts, once/24 h.**
  `crates/gray/src/main.rs:66` → `update.rs:203-220`. Print mode skips it; measured boots
  used `GRAY_NO_UPDATE_CHECK=1`, so real first-boot-of-day is slower than the table.

## Not checked (out of scope / too dangerous)

Full-disk behavior intentionally not simulated — only code paths inspected (H2–H4).
No LLM-backed turns (no prompt-memory pressure beyond the 2000-msg synthetic resume).
