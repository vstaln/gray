# Port Recon: `agent/` subsystem (hermes-agent → Rust)

Source: `/home/vstaln/gray/reference/NousResearch/hermes-agent/agent/`
~151k LOC Python, ~200 modules. Labels: PROVEN = read/grep'd directly; CONJECTURED = inferred.

## 1. Module inventory by responsibility

### Core conversation engine
- `conversation_loop.py` — **8,590 LOC MONSTER** — main agent turn loop; heart of subsystem.
- `tool_executor.py` — **2,922 LOC** — tool dispatch/execution.
- `agent_runtime_helpers.py` — **4,541 LOC** — helpers around the loop.
- `agent_init.py` — **3,068 LOC** — agent construction/config wiring.
- `turn_context.py` (1,508), `turn_finalizer.py` (837), `turn_retry_state.py`, `turn_summary.py`, `oneshot.py`, `moa_loop.py` (**2,453** mixture-of-agents loop), `subagent_lifecycle.py`, `background_review.py` (1,663).

### Context / compaction engine
- `context_compressor.py` — **8,211 LOC MONSTER** — compaction.
- `conversation_compression.py` — **4,465 LOC** — compression pipeline.
- `context_engine.py`, `context_breakdown.py`, `context_references.py` (720), `native_compaction.py`, `compaction_display.py`, `manual_compression_feedback.py`.

### Prompt assembly & caching
- `prompt_builder.py` — **2,598 LOC**; `system_prompt.py` (1,028); `prompt_caching.py`, `prompt_cache_boundary.py`, `prompt_cache_scope.py`; `learn_prompt.py`; `skill_preprocessing.py`.

### LLM transports (provider adapters)
- `transports/base.py`, `transports/types.py` — traits/types.
- `anthropic_adapter.py` — **3,284 LOC** (lazy-imports `anthropic` SDK).
- `chat_completion_helpers.py` — **5,364 LOC** + `transports/chat_completions.py` (1,042) — OpenAI-compatible surface.
- `codex_runtime.py` (1,766), `codex_responses_adapter.py` (1,701), `transports/codex*.py` (app_server 1,292 LOC session, projector).
- `bedrock_adapter.py` — **1,780 LOC** (lazy-imports boto3); `vertex_adapter.py` (google.auth); `gemini_native_adapter.py` (1,252, httpx-native).
- Supporting: `model_metadata.py` (3,767), `models_dev.py` (1,550), `usage_pricing.py` (1,565), `reasoning_*.py`, `lmstudio_reasoning.py`, `moonshot_schema.py`, `gemini_schema.py`, `azure_identity_adapter.py`, `backend_identity.py`, `image_routing.py` (923).

### Credentials / secrets / proxy
- `credential_pool.py` — **3,258 LOC**; `credential_sources.py`, `credential_persistence.py`.
- `secret_sources/`: base, registry, `_cache`, bitwarden (1,055), onepassword (682), command.
- `proxy_sources/iron_proxy.py` — **2,494 LOC**; `proxy_sources/__init__.py`; `aux_accounting.py`, `account_usage.py` (902), `credits_tracker.py` (860), `billing_*.py`, `nous_rate_guard.py`, `rate_limit_tracker.py`.

### Auxiliary LLM client
- `auxiliary_client.py` — **10,831 LOC BIGGEST MONSTER** — side-channel LLM calls (titles, summaries, insights...). Consumers: `title_generator.py` (762), `insights.py` (1,212), `curator.py` (2,034) + `curator_backup.py`, `memory_manager.py` (1,291), `memory_provider.py`, `learning_graph*.py`.

### Tools-adjacent
- `tool_guardrails.py` (855), `tool_dispatch_helpers.py` (814), `tool_result_classification.py`, `file_safety.py` (746), `shell_hooks.py` (1,155), `verification_evidence.py` (800), `verify/` dir, `bounded_response.py`, `empty_response_guard.py`.

### Streaming / errors / display
- `display.py` (1,589), `error_classifier.py` (**2,187**), `errors.py`, `error_surface.py`, `stream_single_writer.py`, `stream_diag.py`, `retry_utils.py`, `deadline.py`, `interrupt_compat.py`, `estop.py`, `thinking_timeout_guidance.py`, `reasoning_timeouts.py`, `think_scrubber.py`.

### Message plumbing
- `message_content.py`, `message_metadata.py`, `message_sanitization.py` (925), `redact.py` (1,427), `markdown_tables.py`, `portal_tags.py`, `reactions.py`, `thread_scoped_output.py`.

### Subsystems (dirs)
- `lsp/` — client (1,029), servers (1,187), manager (744), protocol, workspace, range_shift, reporter, eventlog, install, cli. Self-contained-ish.
- `monitoring/` — emitter, events, policy, redaction, otlp_exporter, gateway_health*, cron_health. Late-imports `gateway.status`.
- `pet/` — state, store, render (682), manifest, constants, generate/atlas.py (1,183). Peripheral toy feature.
- `secret_sources/`, `proxy_sources/`, `transports/`, `verify/` as above.

### Misc/peripheral
Providers-registries (`*_provider.py`/`*_registry.py`: browser, image_gen, video_gen, web_search, tts, transcription), `plugin_llm.py` (1,217), `relay_llm.py` (1,357) + `relay_runtime.py` (2,073), `outbound_webhooks.py`, `trace_upload.py`, `trajectory.py`, `i18n.py`, `battery.py`, `ssl_guard.py`/`ssl_verify.py`, `jiter_preload.py`, `kanban_stop.py`, `onboarding.py`, `subscription_view.py`, `delegation_context.py`, `coding_context.py` (916), `skill_utils.py` (1,249)/`skill_commands.py` (904), `process_bootstrap.py`, `command_token_source.py`, `copilot_acp_client.py` (840), `acp`-related.

## 2. External dependency edges (define port order)

PROVEN via grep of top-level imports across agent/:
- Root-level modules: `hermes_constants` (28 files), `utils` (20 files: atomic_json_write, safe_json_loads, base_url_host_matches, env_int/env_float, normalize_proxy_env_vars...), `hermes_logging`, `hermes_time`. **No direct imports of hermes_state.py or model_tools.py found in agent/** (CONJECTURED clean, checked top-50 frequency list only).
- `tools/` package (14 files): registry.tool_error, threat_patterns, todo_tool.TODO_INJECTION_HEADER, terminal_tool, budget_config, daemon_pool, thread_context, skill_provenance, skill_usage, environments.local, tool_result_storage.
- `hermes_cli/` (22 files): timeouts, _subprocess_compat, auth, sizefmt, runtime_provider, route_identity.
- `gateway/` — only via **late/lazy imports**: gateway.session_context (set_current_session_id, get_session_env), gateway.status (health parsing), gateway.run config load. ⇒ circularity avoided in Python by lazy import; Rust port needs session_context extracted to a shared crate or callback.

Port order implication: hermes_constants/utils/hermes_time/hermes_logging first, then tools/registry+budget_config stubs, then agent/.

## 3. External PyPI deps → Rust crates

Surprisingly light (PROVEN):
| PyPI | Used in | Rust |
|---|---|---|
| httpx (async HTTP) | ~23 files, everywhere | reqwest (+tokio) |
| anthropic SDK | anthropic_adapter only, LAZY import | none — hand-write REST on reqwest |
| openai SDK | video_gen_provider only (lazy) | reqwest |
| boto3 | bedrock_adapter only (lazy) | aws-sdk-bedrockruntime (or skip phase 1) |
| google.auth | vertex_adapter only | skip/late phase |
| pyyaml | 2 files | serde_yaml |
| wcwidth | 1 file | unicode-width |
| requests | 1 file | fold into reqwest |

No pydantic, no rich, no tiktoken at module level. Most "SDK" usage is hand-rolled JSON over httpx — good for Rust.

## 4. Port order WITHIN agent/

1. Leaf utilities: errors.py, deadline.py, retry_utils.py, ssl_verify, message_content/message_metadata, i18n-free leaves.
2. Transports layer: transports/types.py + base.py, then chat_completions (most generic), then anthropic_adapter, codex, bedrock last.
3. Context engine: context_engine → context_compressor/conversation_compression (needed before loop can run long).
4. Prompt layer: prompt_builder/system_prompt/prompt_caching.
5. Core loop: agent_init → turn_context → tool_executor → agent_runtime_helpers → conversation_loop.
6. Auxiliary client (auxiliary_client.py) + its consumers (memory_manager, curator, title_generator).
7. Credentials/secrets/proxy pool.
8. Peripherals: lsp/, pet/, monitoring/, provider registries.

## 5. Risks for literal Rust translation

- **Monster files** (auxiliary_client 10.8k, conversation_loop 8.6k, context_compressor 8.2k): single-file god modules; expect module splitting in Rust; borrow-checker friction from shared mutable conversation state (Python passes big dicts/lists freely).
- **Lazy imports everywhere** as API/circularity mechanism (anthropic, boto3, gateway.session_context): must be resolved into explicit crate deps or trait objects at port time.
- **Dynamic dispatch via string keys**: provider/model routing, registries (`*_registry.py`) are dict-of-factories; map to trait objects + HashMap registries.
- **Threading**: 52 files use `threading`; PROVEN only 12 of ~199 files contain `async def` at all — this subsystem is overwhelmingly SYNCHRONOUS/thread-based with locks, not asyncio-first. Rust: std::thread + Mutex/channels is a faithful translation; tokio only where transports do async streaming HTTP (reqwest blocking client is an option).
- **Monkey-patching**: PROVEN limited and mostly test-seams (`patch("agent.model_metadata.requests.get")`, monkeypatch fetch_models_dev in models_dev.py) ⇒ Rust: make fetch fns injectable/closure fields rather than globals.
- **Global/session state**: contextvars (13 files) + gateway.session_context — task-local state; Rust needs task_local! or explicit context struct threading.
- **Decorators-as-API**: 3,724 defs total; decorator-heavy API not confirmed (INCONCLUSIVE — not censused); risk low given sync style.
- **JSON-shaped data**: most messages are loose dicts (message_content.py normalizes); Rust will force serde enums — biggest semantic-risk area, message schema must be pinned first.

## 6. Load-bearing vs peripheral

LOAD-BEARING: conversation_loop, tool_executor, context_engine/compressor, prompt_builder, transports/*, chat_completion_helpers, agent_init/runtime_helpers, turn_*, errors/error_classifier, credential_pool, auxiliary_client (everything calls it for side-LLM work).

PERIPHERAL (port last or drop): pet/, battery.py, billing_view, kanban_stop, onboarding, reactions.py, portal_tags.py, moa_loop (feature loop), relay_* (alt runtime), copilot_acp_client, image/video/tts/web_search registries, curator/insights (nice-to-have memory features).

---
*Recon by adventurer sub-agent; method: ls/wc -l inventory + grep import census (6 tool calls at commit). Section 5 decorator/monkey-patch rows INCONCLUSIVE.*
