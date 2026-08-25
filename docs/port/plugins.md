# Port recon: `plugins/` subsystem of hermes-agent

Sources: direct reads of `plugins/plugin_utils.py`, `plugins/plugin_storage.py`, `hermes_cli/agent_plugins.py` (grep-level), `hermes_cli/plugins.py` (grep-level), directory listings, LOC counts via `wc -l`. Labels used: PROVEN (read directly) / CONJECTURED (inferred from names/grep).

## 1. How the plugin system works

PROVEN from reads:

**There is no heavyweight plugin base class.** The system is a *manifest + register-function + context-object* pattern:

- Each plugin is a **directory containing a `plugin.yaml` manifest** (fields: `name`, `kind`, `version`, `description`, `author`) **plus an `__init__.py` with a `register(ctx)` function** (stated verbatim in `hermes_cli/plugins.py` docstring).
- Discovery scans **four sources** (docstring line 5): the bundled plugins dir (`get_bundled_plugins_dir()`), pip entry points (`importlib.metadata.entry_points()`, group-filtered, import-free source-scan classification at `_classify_entrypoint_value_kind`), and two more paths not yet enumerated (CONJECTURED: user plugin home + agent-plugin packages).
- Loading is **lazy**: entry-point discovery "scans for provider markers ... instead of being eagerly imported" — manifests are classified without executing code.
- On load, each plugin's `register(ctx)` receives a giant **`PluginContext`** object (~2,000 lines in plugins.py, lines 1400–3310+) with typed registration methods:
  - `register_tool`, `register_cli_command`, `register_command`, `register_context_engine`, `register_context_reference`, `register_memory_provider`, `register_image_gen_provider`, `register_dashboard_auth_provider`, `register_video_gen_provider`, `register_web_search_provider`, `register_browser_provider`, `register_secret_source`, `register_tts_provider`, `register_transcription_provider`, `register_platform`, `register_slack_action_handler`, `register_auxiliary_task`, `register_redaction_patterns`, `register_hook`, `register_system_prompt_section`, `register_middleware`, `register_approval_transport`.
- Enable/disable is config-driven: allow-list/deny-list read from `config.yaml` (`_get_enabled_plugins` / `_get_disabled_plugins`). `HERMES_PLUGINS_DEBUG=1` enables verbose discovery logging.
- A separate, parallel system exists: **"agent plugins"** (`hermes_cli/agent_plugins.py`) — JSON-manifest packages bundling skills + MCP server configs, validated statically (path-escape checks, remote URL validation). This is data-driven, not code-driven; trivially portable as serde structs.
- Hook system: normalized event envelopes dispatched via `invoke_hook`; hooks only fire when subscribed (payloads not built otherwise).

**The two files I was told were "the loading machinery" are NOT.** PROVEN:
- `plugins/plugin_utils.py` = thread-safe lazy-singleton helpers for plugin authors (`lazy_singleton` decorator with double-checked locking, `SingletonSlot` keyed slot). In Rust: `OnceLock` / `LazyLock` covers both; ~0 lines to port.
- `plugins/plugin_storage.py` = per-plugin durable storage convention: `<hermes-home>/plugin-data/<name>/` + SQLite db helper (WAL mode, `check_same_thread=False`, name validation regex against traversal). In Rust: small module around rusqlite; port the name-validation guard verbatim.

### Rust translation verdict

Python's mechanism is runtime `importlib` + duck-typed registration objects. For a 1:1 Rust port:
- **Bundled providers → traits + compile-time registration.** Every provider class (e.g. `AnthropicProfile(ProviderProfile)` from shared `providers/base.py`) maps to a trait; each plugin becomes a crate module registering into a registry at startup. Feature flags per provider/platform give the Python enable/disable semantics without dynamic loading.
- **No dylibs/WASM needed** unless third-party plugin installation is in scope. The vendored repo ships all plugins in-tree; nothing in the core flow requires post-compile loading. If install/update/remove (`hermes plugins remove|update` mentioned in plugin_storage.py) must work for user-written plugins, that's the one argument for dylib/WASM — flag as open product decision, don't build speculatively.
- The registry pattern (`PluginContext.register_*`) ports directly to a builder struct holding `Vec<Box<dyn Trait>>` slots.

## 2. Category inventory (LOC = measured `wc -l`)

| Category | LOC | Plugins |
|---|---|---|
| platforms/ | 50,651 | 22 dirs |
| memory/ | 23,994 | 8 dirs |
| model-providers/ | 27,666* | 37 dirs |
| image_gen/ | 4,763 | 7 |
| web/ | 4,278 | 9 |
| google_meet/ | 3,440 | node+realtime subdirs |
| kanban/ | 3,016 | dashboard+systemd |
| teams_pipeline/ | 2,675 | — |
| dashboard_auth/ | 2,315 | 4 (basic, drain, nous, self_hosted) |
| video_gen/ | 1,988 | 3 (deepinfra, fal, xai) |
| observability/ | 1,801 | langfuse only |
| hermes-achievements/ | 1,263 | dashboard/docs/tests bundled |
| disk-cleanup/, spotify/, cron_providers/ | ~900 each | singletons |
| browser/ | 870 | 3 (browserbase, browser_use, firecrawl) |
| security-guidance/ | 627 | singleton |
| context_engine/ | 285 | singleton |

*model-providers top-level shows 2,766 but that's only root files; the 37 subdirs weren't individually summed — treat ~40 providers × ~500–800 LOC as estimate. CONJECTURED on exact split.

**Big adapters (flagged cost drivers):** telegram (~11k), discord (~10.6k), slack (~9.6k) dominate platforms/. Together with matrix, feishu, dingtalk, teams, whatsapp they are most of the subsystem's mass.

Per-plugin one-liners (from dir names + kind fields; one-liners are name-derived, CONJECTURED where noted):
- **model-providers/**: thin API-profile adapters (fetch models list, auth header shape, OAuth flows for codex/qwen/copilot/kimi/alibaba-coding-plan). All follow `ProviderProfile` subclass + `register_provider()` (PROVEN pattern from anthropic + openai-codex). Notable: bedrock & vertex (cloud SDK auth — heavier), custom (user-defined), ollama-cloud, openrouter.
- **platforms/**: full bidirectional chat-platform clients (telegram, discord, slack, matrix, feishu, dingtalk, teams, whatsapp, irc, email, google_chat, line, sms, a2a, mattermost, ntfy, simplex, wecom, homeassistant + internal ones buzz/photon/raft).
- **memory/**: mem0, honcho, hindsight, holographic, supermemory, byterover, openviking, retaindb — pluggable long-term-memory backends via `register_memory_provider`.
- **image_gen/**, **video_gen/**: per-vendor generation backends (deepinfra, fal, krea, openai, openai-codex, openrouter, xai / same trio for video).
- **web/**: search backends (brave_free, ddgs, exa, firecrawl, keenable, parallel, searxng, tavily, xai) via `register_web_search_provider`.
- **browser/**: browserbase, browser_use, firecrawl automation providers.
- **google_meet/**: Meet bot integration incl. a Node component (node/ + realtime/) — non-Python dependency, special-case in port.
- **kanban/**: dashboard + systemd unit management (3016-line plugin_api.py).
- **teams_pipeline/**, **dashboard_auth/**, **hermes-achievements/**: support surfaces for Teams media pipeline, dashboard auth strategies, gamification.
- **observability/langfuse/**: LLM tracing export (1801 lines, single file).
- **cron_providers/chronos**, **disk-cleanup**, **spotify**, **security-guidance**, **context_engine/**: small utility plugins.

## 3. Deps reaching OUTSIDE plugins/

PROVEN (from imports seen):
- `providers` / `providers.base.ProviderProfile` — shared provider registry lives OUTSIDE plugins/ (top-level `providers` package).
- `tools.registry` — tool registration target of `ctx.register_tool`.
- `hermes_constants.get_hermes_home` — profile-aware home path resolution.
- `hermes_cli.config` (cfg_get/load_config) — enable/disable lists, secrets via `.env` / `agent.secret_scope`.
- `utils` (env_var_enabled, fast_safe_load), `agent.skill_utils.yaml_load`.
- `hermes_cli.urllib_security.open_credentialed_url` (anthropic provider).
CONJECTURED (implied by PluginContext surface): platform message bus, hook dispatcher, middleware chain, auxiliary task scheduler — all consumed but defined outside plugins/.

## 4. External PyPI deps → Rust crates

| Python dep | Rust crate |
|---|---|
| discord.py | serenity (+ songbird if voice) |
| python-telegram-bot | teloxide |
| slack_sdk | slack-morphism |
| matrix-nio | matrix-sdk |
| telethon (sms/telegram alt?) | grammers |
| google api clients (meet/google_chat) | google-drive*/google-apis crates or raw REST via reqwest |
| botox/boto3 (bedrock) | aws-sdk-bedrockruntime |
| google-auth (vertex) | gcp-auth / google-cloud-auth |
| requests/httpx | reqwest |
| pyyaml | serde_yaml |
| sqlite3 stdlib | rusqlite |
| langfuse sdk | none mature — hand-rolled REST client (it's already mostly hand-rolled in Python too) |

Not verified per-file which platforms use which libs (would need per-plugin imports grep); table is the standard mapping set.

## 5. Port order & load-bearing assessment

Load-bearing (core product value, port first):
1. **Plugin machinery itself** (registry + traits replacing PluginContext) — everything hangs off it. Small: plugin_utils/plugin_storage ≈ 215 lines total.
2. **model-providers/** — without an active provider the agent can't run. Thin adapters; port anthropic first as the template (it's ~100 lines of profile logic), then batch the REST-only ones. bedrock/vertex last (SDK auth complexity).
3. **web/ search + image_gen + video_gen** — thin REST wrappers, high value/effort ratio.
4. **memory/mem0 + one other** — memory is a headline feature; the rest are swappable vendors.

Peripheral (defer):
5. **platforms/** — biggest LOC, but each is independent; port telegram/discord/slack only when multi-platform is actually needed. Each is essentially its own project (SDK surface area).
6. google_meet (has a Node component — needs separate decision), kanban/dashboard_auth/achievements/teams_pipeline (dashboard support tooling), spotify, disk-cleanup, cron_providers.

## 6. Rust risks

- **Entry-point discovery does not translate.** `importlib.metadata.entry_points()` + import-free source scanning has no Cargo equivalent. Bundled plugins → static registry (easy). Third-party installed plugins → needs dylib ABI or WASM host or a config-declared process-outside model. Biggest architectural fork in the whole port; decide before porting anything else. (PROVEN problem, solution undecided.)
- **Hot reload / update**: Python gets this free (re-import); Rust doesn't. `hermes plugins update` git-pulls live code. Either drop hot reload (restart to apply) or pay subprocess/isolation cost. Recommend: drop it, restart-based.
- **Duck typing vs traits**: Python plugins implement informal interfaces (any object with the right methods registered via ctx). Rust forces trait definitions up front — every `register_*` family needs its trait spelled out. This is upfront design cost concentrated in PluginContext (~2000 lines of registration surface to mirror).
- **Platform SDK surface area is the real cost driver**: telegram/discord/slack each embed connection management, reconnect logic, file upload, slash-command sync, webhook verification. The Rust crates (teloxide/serenity/slack-morphism) cover much but not 1:1 — expect per-platform shims. Budget platforms/ at roughly its Python LOC, not less.
- **lazy_singleton semantics**: Python caches "first config wins" (SingletonSlot ignores later args). Rust `OnceLock` matches, but the reset() paths used by tests need `RwLock<Option<T>>` instead — pick one convention repo-wide.
- **plugin-data storage**: easy port (rusqlite WAL), keep the name-validation regex exactly.
