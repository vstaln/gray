# Port recon: hermes-agent `tools/` (~144k LOC, 154 .py files + environments/ + computer_use/)

Sources: `ls -la tools/`, `wc -l` inventory, `grep ^import/^from` dep scan, skim of `registry.py`. Labels: PROVEN = read/grepped directly; CONJECTURED = inferred from names/sizes.

## 1. Module inventory by responsibility

### Core plumbing
- `registry.py` (1,335) — central tool registry; tools self-register at module import time via `registry.register()`; dispatch boundary with error truncation. PROVEN (read head).
- `lazy_deps.py` (2,412) — lazy/optional-dependency import machinery; explains near-zero top-level third-party imports.
- `tool_backend_helpers.py`, `tool_output_limits.py`, `tool_result_storage.py`, `tool_search.py` — result shaping, output caps, spill-to-disk, tool lookup/search over registry.
- `managed_tool_gateway.py`, `plugin_guard.py`, `schema_sanitizer.py`, `interrupt.py`, `spill_safety.py`, `hook_output_spill.py`, `thread_context.py`, `daemon_pool.py`, `debug_helpers.py` — gateway/dispatch support.

### File / code editing
- **`file_tools.py` (2,818)** and **`file_operations.py` (3,410)** — file CRUD/edit tools (two layers; file_tools likely the tool surface, file_operations the ops core).
- `file_state.py`, `patch_parser.py` (30k), `fuzzy_match.py` (~50k), `read_extract.py`, `binary_extensions.py`, `ansi_strip.py`, `shell_heredoc.py`, `working_diff.py`, `self_repo_guard.py`, `osv_check.py`.
- `path_security.py` (1.3k — small!) + `threat_patterns.py`, `url_safety.py`, `website_policy.py` — security guards.

### Terminal / processes
- **`terminal_tool.py` (4,015)**, `process_registry.py` (**3,258**) — persistent shell sessions + process tracking. Subprocess-heavy.
- `code_execution_tool.py` (**2,242**), `env_probe.py`, `env_passthrough.py`, `interpreter_shutdown.py`.

### Browser / computer use
- **`browser_tool.py` (5,601)**, `browser_supervisor.py` (1,518), `browser_camofox.py` (+state), `browser_cdp_tool.py`, `browser_dialog_tool.py`, `browser_extension_router.py`, `browser_use_cli.py` — Playwright/Camoufox/CDP stack.
- `computer_use/` (9 files): **`cua_backend.py` (3,955)**, `tool.py` (1,892), backend routing, permissions, vision_routing, doctor, schema.

### Environments (sandbox execution)
- `environments/base.py` (1,533) abstract env; `docker.py` (**2,060**), `local.py` (1,992), `ssh.py`, `modal.py`+`managed_modal.py`+`modal_utils.py`, `daytona.py`, `vercel_sandbox.py`, `singularity.py`, `file_sync.py`.

### Delegation / subagents
- **`delegate_tool.py` (4,963)**, `async_delegation.py` (1,603), `subagent_worktree.py`, `delegation_live_log.py`, `delegation_output_schema.py`, `clarify_tool.py`+`clarify_gateway.py`.

### Voice stack
- **`tts_tool.py` (4,552)**, `transcription_tools.py` (**3,419**), `voice_mode.py` (**2,379**), `wake_word.py` (1,508), `tts_streaming.py`, `tts_text_normalize.py`, `voice_client_config.py`, `neutts_synth.py`, `audio_container.py`, samples/wakeword assets.

### MCP client
- **`mcp_tool.py` (8,235 — largest file in repo)**, `mcp_oauth.py` (1,956), `mcp_oauth_manager.py`, `mcp_dashboard_oauth.py`, `mcp_schema_cache.py`, `mcp_stdio_watchdog.py`, `setup_mcp_tool.py`.

### Skills system
- **`skills_hub.py` (4,674)**, `skills_tool.py` (**2,173**), `skills_sync_client.py` (**2,187**), `skills_sync.py`, `skill_manager_tool.py` (1,850), `skill_usage.py`, `skill_linter.py`, `skills_guard.py` (~48k chars), `skill_ledger.py`, `skill_provenance.py`, `skills_ast_audit.py`, `skillevaluator_scan.py`.

### Messaging / social / integrations
- `send_message_tool.py` (**2,274**), `discord_tool.py`, `bot_mode_dm.py`, `bot_mode_probe.py`, `bot_relay.py`, `bot_failure_reasons.py`, `react_to_message_tool.py`, `x_search_tool.py`, `homeassistant_tool.py`, `feishu_doc/drive_tool.py`, `microsoft_graph_*`, `yuanbao_tools.py`.

### Media generation
- `image_generation_tool.py` (**2,136**), `vision_tools.py` (**2,223**), `flux3_video_tool.py` (1,249), `video_generation_tool.py`, `fal_common.py`, `image_source.py`, `xai_http.py`, `xai_video_tools.py`, `annotate_preview_tool.py`.

### Kanban / cron / memory / misc agent-facing
- `kanban_tools.py` (**2,480**), `cronjob_tools.py` (**1,872**), `memory_tool.py` (1,394), `todo_tool.py`, `session_search_tool.py` (1,321), `checkpoint_manager.py` (**2,196**), `credential_files.py`, `budget_config.py`, `blueprints.py`, `tour_tool.py`, `project_tools.py`, desktop/preview/window/terminal-viewer small tools (`open/close/read_preview_tool.py`, `*_window_tool.py`, `focus_pane_tool.py`, etc.), `desktop_ui.py`.

## 2. Internal deps reaching OUTSIDE tools/ (port-order drivers)

PROVEN via grep of top-level imports:
- `hermes_constants` (30 files) — root constants module. Port first.
- `hermes_cli.config` (10), `hermes_cli._subprocess_compat` (13) — config + subprocess shims.
- `utils` (14) — sibling util package.
- `agent.*`: `agent.secret_scope` (4), `agent.redact` (5), `agent.skill_utils` (6+), `agent.file_safety`, `agent.browser_registry`, `agent.browser_provider`, `agent.video_gen_provider`, `agent.retry_utils`, `agent.interrupt_compat`, `agent.thread_scoped_output`.
- `plugins.web.{tavily,parallel,firecrawl,exa}.provider`, `plugins.video_gen.xai`, `plugins.browser.{firecrawl,browser_use,browserbase}.provider` — search/browser providers live OUTSIDE tools/.
- `gateway.session_context` (2), `hermes_state_common`, `cron` (package).

Implication: before tools/, port `hermes_constants`, `utils`, `agent/{redact,secret_scope,file_safety,skill_utils}`, plugin provider interfaces (or stub traits for them).

## 3. External PyPI deps → Rust crates

Top-level imports are nearly pure-stdlib (lazy_deps defers heavy ones). Inline imports found:
| Python | Rust |
|---|---|
| httpx (20), requests (12), aiohttp (6) | reqwest |
| websockets (3) | tokio-tungstenite |
| yaml (11) | serde_yaml |
| psutil (8) | sysinfo (or procfs on Linux) |
| numpy (8), sounddevice (7) | ndarray; cpal (voice stack only) |
| modal (2) | modal HTTP API via reqwest (no official crate) |
| mcp (1) | rmcp |
| docker (implied by environments/docker.py) | bollard |
| playwright (browser stack; imported lazily) | chromiumoxide or drive headless Chrome via CDP; camoufox has no Rust port → CDP-only path |
| ssh (environments/ssh.py, paramiko presumed lazy) | russh / openssh |
| PIL/vision, audio codecs | image crate; symphonia/hound as needed |

CONJECTURED beyond grep hits (heavy libs hidden behind lazy_deps): pydantic schemas → schemars/serde_json schema gen.

## 4. Port order within subsystem

1. `registry.py` + `schema_sanitizer.py` + `tool_output_limits/tool_result_storage/tool_backend_helpers` — everything registers through this.
2. Leaf utilities: `fuzzy_match`, `patch_parser`, `ansi_strip`, `shell_heredoc`, `path_security`, `threat_patterns`, `url_safety`.
3. `file_operations` → `file_tools`; `process_registry` → `terminal_tool` (stdlib subprocess→tokio).
4. `environments/base.rs` trait, then `local`, then `docker` (bollard), then remote envs (ssh/modal/daytona/vercel).
5. `mcp_tool` + oauth cluster (rmcp).
6. `delegate_tool`/`async_delegation` (needs agent-core loop trait).
7. Browser stack (biggest external risk), computer_use, media gen, voice, skills hub, kanban/cron, messaging — peripheral, any order after 1–4.

## 5. Rust translation risks

- **Module-level self-registration**: every tool calls `registry.register()` at import time; Rust has no import side effects → need explicit registration table or inventory/linkme crate, or a build-time generated manifest. This is THE structural change.
- **Dynamic JSON tool schemas** (`**kwargs`-style dicts passed to LLM): handlers take untyped dict args validated ad hoc; Rust wants typed structs + serde_json::Value escape hatch + schemars for the schema surface. `schema_sanitizer.py` exists precisely because these schemas drift.
- **Runtime plugin loading**: `importlib.util` used (4 hits); plugins/web & plugins/browser providers resolved at runtime → Rust needs trait objects registered statically, or dylib scripting (recommend: static trait registry, drop dynamic loading).
- **Subprocess patterns everywhere**: pty-based terminal sessions, process_registry tracking, daemon_pool, atexit cleanup, signal handling → portable but must be designed around tokio::process + portable-pty; `_subprocess_compat` shows platform shims already needed.
- **Lazy dependency imports**: optional features degrade gracefully when a lib is missing (lazy_deps.py is 2.4k LOC of this). In Rust, cargo features replace it — decide feature flags per integration early.
- **Sheer size**: mcp_tool.py alone is 8.2k lines; approval.py 5.6k. Budget per-file ports, not per-module.
- **Global mutable state**: registries, process tables, checkpoint manager use module-level singletons w/ threading locks → Rust statics/OnceLock or pass an AppState explicitly.

## 6. Load-bearing vs peripheral

LOAD-BEARING (agent cannot run without): registry.py, lazy_deps semantics, file_tools/file_operations, terminal_tool/process_registry, environments/base+local(+docker for parity), approval/write_approval, delegate_tool, path_security, schema_sanitizer, tool_output_limits/result_storage.

PERIPHERAL (feature-gated, port later or skip): voice/TTS/transcription/wake_word, browser/computer_use (huge, external-browser dependent), media generation (image/video/xai/fal), messaging/social (discord/bot modes/feishu/yuanbao/microsoft_graph/homeassistant), skills_hub cluster (could be data-dir scripts instead), kanban/cronjob, memory/session_search/tour/blueprints, preview/desktop UI tools.

Skipped: reading large module bodies; inventory from sizes+names+import graphs only. Deep-dive individual monsters (mcp_tool, approval, delegate_tool) before porting each.
