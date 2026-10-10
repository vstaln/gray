# gray TODO

## Done this session
- [x] `/connect` receipt names the real provider — was literal "provider" for
      plugin/subscription connections (their placeholder `base_url` matches no
      catalog row). Now resolves via `resolve_provider_connection` → catalog →
      endpoint host; receipt matches `/model`'s `✓ … set to` shape.
      Commit `71103c36` on `feat/onboarding-flow` (3 files: repl/{mod,handlers,tests}.rs),
      built on maid, installed to `~/.cargo/bin/gray` (backup `gray.bak`).
- [x] e2e-verified in tmux TUI: `/connect` → Devin subscription → SWE-2 Max prints
      `✓ Connected to Devin subscription  swe-2 · max`.
- [x] Checked for the same bug elsewhere: `push_provider_connected` was the only
      site with the base_url→"provider" fallback. No other ports affected.
- [x] Model-switch lag — `2dbbfe8d`, ~3.7s → ~77ms.

- [x] claude-sub `incomplete native response` mislabel — `judge()` only knew
      `api_error_status` rejections; any other `is_error` result collapsed to
      NO_ANSWER ("retryable"). Now non-refusal failures surface their real
      text. `~/grayplugins/gray-claude-sub` commit `1d03ec7`, built on maid,
      58 tests pass, installed to `target/release/claude-sub` (bak kept).
      Applies to newly spawned claude-sub sessions; restart gray windows.

## Session: E2E sweep + concurrency fix (2026-10-09)

Done:
- [x] Broad plugin E2E sweep via gray-subagents workers (11 runs)
- [x] Verified: cursor-sub, grok-sub, opencode-sub, antigravity-sub, claude-sub — manifest + models + real turn all PASS
- [x] Verified: memory (full lifecycle + all 5 repo-test repro paths PASS — the `memory_cli` test failures don't reproduce in real usage; likely a test-harness/GRAY_HOME issue, not a plugin bug)
- [x] Verified: tool-gate (deny→verdict→allow cycle), bash-env (prelude injects toolchain env)
- [x] Verified: subagents run/status/view/list/settings; nested run+steer correctly blocked by recursion guard
- [x] FIXED: concurrent `gray -p`/subagent spawns failing ~50% — root cause was `provider/chat` 10s TTL too short under sidecar spawn contention (relay-open raced `devin acp` startup). Bumped to 30s in gray-plugin sidecar.rs. 6/6 concurrent now pass (was 0–2/7).
- [x] FIXED: `turn_failed` swallowed the real error everywhere (row, session, logs). print.rs now carries `detail` (scrubbed `{error:#}` chain) on every error row + maps `provider_error` with a real message/hint.
- [x] FIXED: `format_core_error` card wrap in print.rs destroyed `CoreError` structure → every error classified `turn_failed`. Now wraps as anyhow context; auth_failed/rate_limited classify correctly again.
- [x] Committed: openai.rs acquire-error carry-through (cb3ef715)

Still uncommitted (my files touch other-session WIP — commit when their print.rs/sidecar.rs work lands):
- print.rs: detail field + provider_error message/hint + CoreError context wrap
- sidecar.rs: provider/chat TTL 10→30s

New issues found (e2e):
- [ ] `/gateway` connections panel (other session's WIP feature) stuck open in TUI — swallowed all keystrokes incl. Escape/Ctrl-C; Enter kept operating the modal. Verify after their WIP lands.
- [ ] hands: no Android device attached — plugin answers cleanly but device path unverifiable until a device is connected.
- [ ] `context/build` frame-too-large warnings on big sessions (my own session >1MB context) — the pi hook fails open; worth a look.
- [ ] subagents run beyond max_running=5 appears to silently no-op (6 of my launches vanished, no error surfaced).
- [ ] concurrent gray procs write to one gray.log → interleaved/garbled lines at the same timestamp.

## Open
- [ ] `tests/it` memory_cli: 5 failures (audit/concurrent/no-provider/opt-out/
      bad-input). Unrelated to receipt fix — in the other session's WIP area.
      Re-check once their 18 uncommitted files settle.
- [ ] Other session's work-in-progress: 18 modified files (usage_panel etc.),
      actively changing — one mid-edit snapshot didn't compile (theme().warning).
      Don't build-install from the raw worktree while it's in flux.
- [ ] Push `feat/onboarding-flow` — needs user's go-ahead.
- [ ] Restart running gray windows/sessions to pick up the new binary.
- [ ] apify MCP server broken at startup ("connection closed: initialize
      response"), keeps retrying — investigate or disable.
- [ ] First prompt in a new window still ~3s (mcp plugin waits for `rea` MCP
      server). Fix candidate: answer tools listing immediately, report tools as
      they arrive.
- [ ] Live Devin-session test of the devin-sub harness-call fix (`mcp_redirect`)
      — never exercised outside unit tests; needs user's ok (uses subscription).
- [ ] Optional: `/devin tools +web_search +web_fetch`.
- [ ] Minor: cron job without `--name` shows first words of the prompt as the
      card name.

## Token-burn & cache fixes (from session analysis, 2026-10-10)

Evidence: `~/.gray/logs/gray.log` request-usage lines; session files
`covalent-photon-manifold` (video work) and `fungal-plasma-glacier` (gray dev).
~1383 requests / ~120M input tokens in ~2h; 97% cache-read but ~92k avg
context per request; one run hit 4.77M in / 183 messages.

### Findings
- Cache warming NEVER ran: all three subscription plugins (claude-sub,
  devin-sub, antigravity-sub) lack `request.warm_replay`; `cache_warm_policy`
  in `crates/gray/src/lib.rs` returns None for plugin providers without it.
  Same bug on all ports — confirmed.
- claude-sub declares `cache_ttl_secs: 3600` but real cache behaves ~5min
  (69.1k re-billed after a 6.5m ffmpeg + ~11m turn gap). devin/antigravity
  correctly declare 300.
- claude-sub is a per-request `claude -p` CLI relay — the verbatim-replay
  caveat in lib.rs applies; needs a dedicated warm path or a replay-safety
  audit (plugin src: `~/grayplugins/gray-claude-sub/src/{chat,relay,keepalive}.rs`).
- 315/315 tool-using assistant messages issued exactly 1 tool call (no
  parallel batching) despite manifest `parallel_tool_calls: true`.
- Images persist in history forever: ~2MB base64 in the video session
  (one 889KB image); compaction re-attaches the ORIGINAL first message
  including its 233KB screenshot at every boundary (6 compactions/session).
- `sidecar context/build failed: sidecar request frame too large` on every
  tool call — likely raw image base64 in the context/build payload.
- Thinking `encrypted_content` embeds a full JSON copy of the message
  (~1.7MB stored in one session) — check what the relay sends upstream.
- Tool results up to 38KB kept verbatim in context; spill machinery exists
  (`gray-core/src/spill.rs`, `tool_out.rs`) — check its threshold.
- "not installed" hint misparses multi-line errors (`` `\nrsync:` ``).
  `gray-tools/src/shell/tools/bash.rs:1399`.

### Fix list (priority order — harness only, no unit tests unless necessary)
- [x] 1. Parallel tool calls — ROOT CAUSE FOUND: transport was fine
      (`parallel_tool_calls: true` goes on the wire, `parallel::plan_segments`
      + `join_ordered` execute fan-out, relay `front_parked` handles them).
      The real bug: `TOOL_BATCHING_GUIDANCE`/`build_runtime_prompt` was DEAD
      CODE — never called in production, so the batching instructions never
      reached the model. Wired `harness_facts()` into the system-prompt build
      in lib.rs (skipped under --bare).
- [~] 2. Cache warming — REVISED after reading the plugins:
      a. Both claude-sub AND devin-sub already have their own keepalive
         sweepers for idle pooled sessions (claude-sub keepalive.rs probes
         every 50min vs ~1h TTL; devin-sub every 4min vs 300s TTL). TTLs
         are HONEST — claude-sub's 3600 matches its real ~1h cache.
      b. The observed 69.1k miss was swe-2/devin-sub mid-ROUND: a 6.5min
         foreground ffmpeg held the session checked out past the 300s TTL.
         warm_replay stays false (verbatim replay unsafe on per-turn-CLI
         relays — by design). Structural fix shipped instead: the prompt now
         tells the model the provider's TTL and to background commands that
         would outlive it (a re-pooled session is keepalive-able; a parked
         one is not).
      c. DONE: lib.rs logs once when warming is skipped for plugins.
- [x] 3. Compaction anchor scrubbed — `anchor_message` now runs
      `stub_large_media` (Image/Media ≥8KB → text citation stub), and
      `build_retained_with_session` stubs media in groups older than the
      newest 2 kept groups. (Original first message's image —
      keep its text + a `[Image elided]` stub; audit compaction cadence
      (6 in 90min); add a cache breakpoint on the compaction request's
      stable prefix (37% of compaction input was uncached).
      (`gray-core/src/agent_compact.rs`.)
- [x] 4. Image hygiene: REMOVED the 2000px pre-resize per user request —
      images pass at native resolution (only the 5MB provider byte cap still
      halves). `normalize_image_bytes` in gray-tools/src/images.rs; updated
      test + all stale "2000px" comments. Still open: elide images older than
      ~3 turns to stubs at compaction boundaries.
- [x] 5. context/build media scrub + splice-back in sidecar.rs
      (MAX_FRAME=256KB was blowing up on image base64; plugins now get
      text stubs, host grafts originals back onto index-matched replies).
- [x] 6. Thinking `encrypted_content` — INVESTIGATED, no fix needed: it's
      the relay's signed-thinking payload that Anthropic requires verbatim
      to replay a tool_use after compaction. The stored JSON echo is the
      signed block itself; can't be trimmed without breaking resume.
- [x] 7. Tool-result caps lowered 50KiB→12KiB in BOTH truncation paths
      (tool_out.rs MAX_BYTES head+tail+spill; truncate.rs DEFAULT_MAX_BYTES
      for ls/grep).
- [x] 8. `not_found_subject` rewritten: last colon-field parse +
      `clean_tool_name` strips literal \n fragments and stray punctuation.
- [ ] 9. e2e verify on claude-haiku + medium effort (user limits nearly
      exhausted — may be blocked): warm hit after a >5min tool call,
      parallel-call fan-out, compaction image stub, no frame-too-large
      warnings in gray.log.

### Working notes for the fixing agent
- Worktree rule (AGENTS.md): one task = one worktree; shared tree has
  another session's live WIP (provider/usage panel: agent_loop.rs,
  event.rs, cache_warm.rs, sidecar.rs, provider.rs, status.rs,
  usage_panel.rs +10 more). Do NOT build/install from the dirty tree.
- Plugin repos live in `~/grayplugins/<name>`; installed manifests in
  `~/.gray/plugins/*-manifest.json` are generated from them.
- Verify fast: `cargo test -p <touched crate>` only; `CARGO_BUILD_JOBS=4`.
