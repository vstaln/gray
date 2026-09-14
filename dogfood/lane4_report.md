# lane4 provider-path analysis (feat/batch-reunite, analysis-only, no fixes)

Scope: read-only on `crates/gray-provider`, `crates/gray-core`, `setup/context`.
No commits/branches/stash. No `cargo test`. No build run (static only).
Real `~/.gray` untouched (`GRAY_HOME=/tmp/lane4_grayhome` for probes).
tmux `lane4_probe` used for probe session, killed after.
Secrets redacted (fake keys only, shown as `[REDACTED]`).

## Per-path table (only 2 wire paths exist)

| | Chat completions (default) | Responses (narrow) |
|---|---|---|
| Routing | everything except below → `POST {base}/chat/completions` (`crates/gray-provider/src/openai.rs:2647-2670`) | only `is_muse_model(model) && base.contains("opencode.ai/zen")` → `POST {base}/responses` (`openai.rs:395-398`, `2618-2646`). `is_muse_model` = lower contains `muse`/`spark`/`glimmer`. |
| Models served | all 171 catalog providers via one `OpenAiProvider` (`crates/gray-plugin/src/builder.rs:614-619`, `crates/gray-provider/src/lib.rs:3-10`); default base `https://openrouter.ai/api/v1` (`crates/gray/src/config.rs:8`) | Muse-family on Zen only (`https://opencode.ai/zen/v1`, `.../zen/go/v1` in `crates/gray/assets/providers.json:880-889`). Muse on OpenRouter/other base stays on chat. GPT-on-Zen stays on chat. |
| Chat body | `map_chat_request` (`openai.rs:460-750`): system→`system`, history→roles, images→`image_url`, `reasoning_content` replay, orphan tool-call stub (`675-718`), `stream:true` + `include_usage:true` | n/a (Responses mapper instead) |
| Responses body | n/a | `map_chat_to_responses` (`openai.rs:893-1065`): `instructions`, `input` items (`function_call`/`function_call_output`/`reasoning` verbatim same-model only `973-978`), orphan stub (`1018-1037`), `store:false` (`1060`), `prompt_cache_key=session_id` (`1059`), `previous_response_id` only when set (`2995-3010` test) |
| Reasoning mapping | `off` → `thinking:{type:disabled}` only; else `reasoning_effort=eff` + `reasoning:{effort:eff}` + `thinking:{type:enabled,budget}` with low=1024, medium=4096, max=32768, else=16384 (high lands in 16384) (`openai.rs:720-749`); `None` omits all three | `None`/`off` → no `reasoning`, no `include`; else `reasoning:{effort:<verbatim>,summary:auto}` + `include:[reasoning.encrypted_content]` (`openai.rs:1067-1079`, `1047-1052`); `max` forwarded verbatim (documented for GPT-5.6) |
| 400 strip-and-retry | once: 400 naming reasoning params + params were sent → strip all three, retry (`openai.rs:1719-1748`, predicate `1113-1145`, strip `1092-1096`); terminal annotated with `/thinking` hint (`1149-1172`, applied `1774-1775`) | same shape for Responses reasoning+include (`1849-1878`, `1098-1107`, applied `1905-1906`) |
| Pre-stream retry | `request_max_retries=3` default (`openai.rs:159`, `2777-2782`); 50ms exp+jitter (`1358-1372`); 429 floor `5s<<n` cap 2^6 (`1378-1380`); `Retry-After`/`retry-after-ms`/HTTP-date capped 60s (`1282-1354`); one `Reconnecting... n/m` notice per burst (`1763-1770`, notice capped 200 chars `1268-1271`); snippet for classifier capped 4000 chars (`1441-1445`) | same pre-stream bounds; resume path below is extra |
| Mid-stream | NO resume: idle timeout (if set) or transport error → terminal (`openai.rs:1944-1957`, `2069-2075`); default idle=None so 120s reqwest read_timeout governs (`143-148`, `160-161`) | resume via `previous_response_id=last_response_id` while `stream_attempt<stream_max_retries=3` (`2102-2601`); tool-arg prefix carried on `Some(id)`, dropped on `None` (`1632-1649`); idle arm same bound (`2150-2197`); transport arm (`2507-2548`); KNOWN RISK with `store:false`: strict backend may 400 unknown id (terminal) or ignore id and duplicate text (`873-879`) |
| Error taxonomy | `classify_http_error` (`1174-1253`): 401/403→Auth; 402→Auth; 429+quota-phrase→Auth terminal; 429→RateLimited retryable; 413→ContextOverflow; 400/404→BadRequest; 500+unsupported/model-not-found→BadRequest else ServerError; context-overflow + content-filter hints narrowed to avoid false positives (`1192-1215`); `is_retryable` = RateLimited/Server/Stream/Timeout/Connection (`1255-1264`) | identical classifier (shared); Responses `response.completed` usage parsed via `OpenAiUsageChunk` with fallback manual `input/output/cached` parse (`2354-2404`) |
| Session/cache | `Authorization: Bearer <key>` omitted when empty (keyless/local ok) (`1407-1413`); `x-opencode-session` header on chat (`1416-1419`); Anthropic cache_control breakpoints (system + last tool + last msg) only when model matches `claude`/`anthropic` (`667-669`, `405-431`) | `prompt_cache_key` = stable session id (per-process UUID fallback, clamped 64, never random per build) (`builder.rs:505-534`, `gray/src/lib.rs:119-121`, `openai.rs:1059`); `include` encrypted-content rides with reasoning; foreign-model thinking dropped (`973-978`) |
| Expected bad-key card | 401/403 → `ProviderError::Auth` → REPL `✗ Auth failed (not retryable) … Check API key or run /connect` (`crates/gray/src/repl/format.rs:142-149`); missing key (no header) expected same 401 from upstream | same Auth card by status code even if body names a model error (see probe) |
| Observed (FAKE key, 1 probe per path) | `POST openrouter /chat/completions` model `openai/gpt-4o-mini`: HTTP 401, 0.12s, 64B, `{"error":{"message":"Missing Authentication header","code":401}}` → maps to Auth terminal, no retry POSTs. tokens 0/0/0, 429=0, 400=0 | `POST opencode.ai/zen/go/v1/responses` model `muse-spark`: HTTP 401, 0.66s, 92B, `{"type":"error","error":{"type":"ModelError","message":"Model muse-spark is not supported"}}` → still Auth by status (terminal, correct to not retry) but body hides key-vs-model cause; REPL card will say Auth while real issue may be model id. tokens 0/0/0, 429=0, 400=0 |
| Missing-key (simulated, no probe) | empty `api_key` → no header (`openai.rs:1407-1413`); config allows `None` until first use (`config.rs:66-69`, `lib.rs:138-144`); upstream 401 expected, same Auth card | same (session header still sent; `prompt_cache_key` unaffected) |

## Effort lists per family (offline fallback vs online)

Offline fallback `supported_efforts` (`crates/gray/src/setup/context/providers.rs:138-284`), gated by `THINKING_LEVELS` (`crates/gray/src/setup/mod.rs:59-67`: off/minimal/low/medium/high/xhigh/max):

- OpenAI GPT: `deep-research`→[medium]; `-chat`→[medium]; `pro` unversioned→[high], 5.1-5.6→[medium,high,xhigh]; `codex` (non-max, pre-5.2)→[low,medium,high], `codex-max`/5.2+→[low,medium,high,xhigh] (+`off` unless max/5.2); 5.6→[off,low,medium,high,xhigh,max]; 5.2-5.5→[off,low,medium,high,xhigh]; 5.1→[off,low,medium,high]; `gpt-5`→[off,minimal,low,medium,high]; future GPT→[off,low,medium,high,xhigh].
- Anthropic: modern (4.7/4.8/sonnet-5/opus-5/fable/mythos)→[low,medium,high,xhigh,max]; older→[low,medium,high,max] (no xhigh, no off in table — `off` re-added by `supported_thinking_levels` filter).
- Gemini: `pro-image`→[high]; `flash-image`→[minimal,high]; `gemini-3`→[minimal,low,medium,high]; else→[low,high].
- xAI: 4.6→[low,medium,high,xhigh]; mini→[low,high]; else→[low,medium,high].
- Kimi/moonshot→[low,medium,high,xhigh,max]; deepseek-reasoner→[low,medium,high]; deepseek-v4→[low,medium,high,max]; muse/spark/glimmer→[minimal,low,medium,high,xhigh] (no max — provider 400s it).
- Unknown family→`None`→full catalog; `model_supports_reasoning==Some(false)`→[off] only (`providers.rs:289-301`).

Online (automatic, wins when present): models.dev `reasoning_options` effort arrays (`null`→`none`→`off`), filtered to `THINKING_LEVELS` (`providers.rs:781-804`, `139-151`); live `/models` `supported_parameters` containing `reasoning` or `reasoning:bool` → `cache_model_reasoning` (`providers.rs:384-398`); reasoning flag is the same source opencode maps to `capabilities.reasoning`.

## Retry/backoff (from code + gray.log taxonomy, no live load run)

- Bounds: `MAX_ATTEMPTS=3` legacy (`openai.rs:19-20`); wired as `request_max_retries=3`, `stream_max_retries=3`, `stream_idle_timeout=None` (`openai.rs:159-161`, `builder.rs:614-619`).
- Backoff: `50ms * 2^(attempt-1)` + subsec-nanos jitter (`1358-1372`); 429 without header uses `5s<<n` floor (`1378-1394`); header wins via max (`1385-1394`); server delays capped 60s (`1276`); `retry-after-ms` preferred over seconds (`1283-1289`, `3491-3495` test).
- Log targets to grep (no counters aggregated): `gray_provider` (retry/strip/idle/stream-error `1691-2549`, `2629`), `gray_agent` (run start/end with in/out/cached/hit% `agent_loop.rs:99,471`; stall nudge `861`), `gray_config` (model/base/key-set `config.rs:122`), `gray_session` (append failures). File `$GRAY_HOME/logs/gray.log` (`logging.rs:128`), rotate 10MB → `.1`/`.2` (`gray-supervise/src/rotation.rs:4-18`, wired `logging.rs:129`), level `GRAY_LOG` default Info (`logging.rs:43-48`), redacts `Bearer`, `sk-` (8+ chars), `x-api-key` (`logging.rs:50-119`).
- 429/400 counts: not counted anywhere; measure by `rg -c "429|RateLimited" $GRAY_HOME/logs/gray.log*` per run. This lane's live probes: 429=0, 400=0 (both 401).

## Cache-hit progression (simulated — needs real keys to measure)

- Shard pin: `provider_cache_key(session_id)` stable across rebuilds/resume (`builder.rs:526-534`, `lib.rs:157-159`, `handlers.rs:245-246`); regression test pins reload path (`lib.rs:405-410`). Chat sends it only as `x-opencode-session` header; Responses sends as `prompt_cache_key`.
- Prefix stability: `prompt/context` hooks fetched once per turn (`agent_loop.rs:129-139`); system+hook concatenated per request (`183-199`, duplicated block — see H5); threshold-reminder note appended per window (`200-210`).
- Warmth: Responses replays encrypted reasoning verbatim same-model only (`openai.rs:973-978`, `agent.rs:528-548`, `message.rs:43-58`); `include` omitted when reasoning off (`2982-2993` test). Chat replays `reasoning_content` text (`549-553`, `240`).
- Expected progression with real keys: turn1 `cached≈0`; turn2+ `cache_read` jumps if prefix byte-stable; `store:false` breaks exact-prefix on turn3+ per code comment (`openai.rs:860-862`) — predicted turn3+ miss regression on Responses. Verify live by comparing `StepUsage`/`TurnEnd` `cache_read_input_tokens` across turns (`agent_loop.rs:362-379`) and footer `· N cached (P%)` (`format.rs:287-301`).
- Anti-footguns: orphan tool-call stubs on both mappers (`openai.rs:675-718`, `1018-1037`); empty-name tools dropped with warn (`437-458`); tool index caps 4096 on both ingest paths (`openai.rs:2013`, `agent_loop.rs:271-277`) with different boundary (`>=` vs `>`, see H6).

## Usage accounting math vs gateway reports

- Wire parse `map_usage` (`openai.rs:770-831`): reasoning subset of output; cache_read = max(details.cached, details.cache_read, top-level cached/read); cache_write from creation tokens; Anthropic shape (creation/read nonzero) → `input_inclusive = prompt + read + write`, `non_cached = prompt`; OpenAI shape → `non_cached = miss_tokens` else `prompt - read - write`; total = provider total else in+out; `normalize()` aliases legacy `cached_tokens` (`event.rs:71-89`); hit rate = `read/input` (`event.rs:62-68`).
- Turn accumulation (`agent_loop.rs:362-379`): input/cached/read/write **overwritten** with latest round, output/reasoning **summed**, total zeroed then normalized. Multi-round tool turns therefore report last-round input, not summed input. Session totals (`status.rs:34-51`) then `+=` that per-turn input/output and price via `turn_cost` (`providers.rs:626-640`: fresh+read+write split when `has_cache_prices`, else all fresh; `None` when unpriced).
- Display: TUI context `used=u.total()`, `hit%` (`composer/draw/mod.rs:320-330`); footer `⬡ N tok · time · $cost ($session)` (`status.rs:67-86`); `/usage` shows in/out/total + rate (`status.rs:90-149`); persisted per-turn usage only on last message (`repl/session.rs:219-235`), rebuilt via `SessionTotals::from_entries`.
- Gateway gap: `crates/gray-gateway/src/daemon_agent.rs` has no usage/cost/token wiring (only cancel tokens matched); no `turn_cost`/`SessionTotals` import under `crates/gray-gateway/src`. Gateway session reports will diverge from REPL (cost/hit% unavailable there). See H9.

## Model catalog gaps

- Bundled `providers.json`: 171 providers, **0** with a `models` key (all entries are `{name,base_url,env_key,featured}` + occasional `no_auth`); `load_catalog` just parses it (`catalog.rs:39-45`).
- `get_provider_models` is a stub returning `[]` (`providers.rs:304-306`); the picker uses `get_provider_models_with_live` → `fetch_live_provider_models` only (`model_modal.rs:9-22`).
- Live fetch: 3s timeout, tries `{base}/models`, `{base}/v1/models`, `/api/tags`, `/api/v1/models` (OpenRouter pinned to `/api/v1/models`), parses `data`/`models`/array shapes, caches context + reasoning + OpenRouter pricing, persists to disk (`providers.rs:309-419`, `851-894`). Offline/empty → picker shows "No models listed — press Enter" / custom-id path (`connect_models.rs:128-149`); direct `/model` validates against that same live list (`model_modal.rs:426+`, `handlers.rs:280-289`).
- Context fallback when nothing cached: hardcoded `fallback_context_length` (`context/mod.rs:265-341`), default guess **256k** (`339`); source tags `override/live/litellm/models.dev/disk/guess` (`providers.rs:550-559`); LiteLLM + models.dev + OpenRouter rates are gap-fill (`470-477`, `666-717`, `749-808`, `851-894`).

## Holes (fix nothing — file:line + severity)

| # | Hole | Location | Severity |
|---|---|---|---|
| H1 | One `OpenAiProvider` for all 171 providers; no native Anthropic/Gemini transports (Anthropic caching only emulated via `cache_control` on chat) | `gray-provider/src/lib.rs:3`, `gray-plugin/src/builder.rs:614-619`, `gray-provider/src/openai.rs:667-669,405-431` | medium |
| H2 | Responses reach is muse-on-Zen only; Muse elsewhere + GPT-on-Zen silently use chat (no `prompt_cache_key` shard, only header) | `openai.rs:395-398,2618-2646` | medium |
| H3 | Responses probe conflates causes: HTTP 401 body was `ModelError: not supported` (fake-key + approx model id); classifier + REPL card report Auth, hiding model-id errors | probe § + `openai.rs:1236-1237`, `repl/format.rs:142-149` | medium |
| H4 | `store:false` + `previous_response_id` resume is a documented contradiction (turn3+ prefix miss; strict backends may 400/duplicate on resume id) | `openai.rs:860-862,873-879` | high |
| H5 | Duplicated `hook_context→system` block (second `let mut system` shadows the first; dead code, confusing for prefix-stability audits) | `gray-core/src/agent_loop.rs:183-199` | low |
| H6 | Tool-index caps differ by one: provider drops `>=4096`, agent errors `>4096` (index 4096 accepted in one, dropped in other) | `openai.rs:2013`, `agent_loop.rs:271-277` | low |
| H7 | Chat reasoning budget table is implicit (`high` falls into `_ =>16384`; `minimal`/`xhigh` send no budget signal on chat while Responses forwards verbatim) | `openai.rs:723-733`, `1074-1079` | low |
| H8 | Effort fallback over-offers: unknown families + `deepseek-chat`-style non-reasoners get full catalog incl `max`; xhigh/max may 400 → strip-and-retry silently changes behavior with label untouched | `providers.rs:272-284,294`, `openai.rs:1722-1724` | medium |
| H9 | Gateway has no usage/cost accounting (`turn_cost`/`SessionTotals` absent); gateway reports vs REPL `/usage`/footer diverge | `gray-gateway/src/daemon_agent.rs` (no hits), vs `repl/status.rs:34-51,67-86` | medium |
| H10 | Turn input accounting overwrites per round (last-round input only) while output accumulates; multi-round turns under-report input vs gateway/provider sums | `agent_loop.rs:362-379` | medium |
| H11 | `cumulative_usage` is actually latest (`set_usage` overwrites both); TUI `latest.or(cumulative)` never sums | `composer/mod.rs:376-381`, `composer/draw/mod.rs:320` | low |
| H12 | Catalog ships zero models; offline = empty picker + guess 256k window; `get_provider_models` stub dead | `assets/providers.json` (171/0), `providers.rs:304-306`, `context/mod.rs:339` | medium |
| H13 | Chat mid-stream stall/idle is terminal (no resume primitive) while Responses resumes; long chat generations on flaky links fail where Responses would heal | `openai.rs:1944-1953,2069-2075` vs `2150-2197,2507-2548` | high |
| H14 | Quota-vs-ratelimit relies on narrow English phrases; non-English/upstream-variant quota 429 falls through to retryable RateLimited and re-burns quota behind 5s floor | `openai.rs:1221-1242,1378-1380` | medium |
| H15 | 429/400/401 counts not aggregated; only raw grep over `gray.log` (rotation 10MB×3) | `logging.rs:128`, `rotation.rs:4-18` | low |
| H16 | `x-opencode-session` required by Console Go (400 `MissingSessionID` without it) but sent only when session non-empty; `None` path depends on fallback key existing | `openai.rs:1415-1419`, `builder.rs:526-534` | low |

## What needs real keys (not done)

Cache-hit progression across turns, LiteLLM/models.dev/OpenRouter effort+rate fills, and provider-total vs local-sum reconciliation all need real keys + TTY/CI (`cargo test` banned under X; several relevant tests marked UNRUN in-tree, e.g. `openai.rs:3380+`). This lane proves auth cards + routing + taxonomy with FAKE keys only; no quota spent (both probes 401, 0 tokens).

## Repro (isolated)

```sh
export GRAY_HOME=/tmp/lane4_grayhome
# chat path (expect 401, no retry):
curl -m 20 -X POST https://openrouter.ai/api/v1/chat/completions \
 -H 'Content-Type: application/json' -H 'Authorization: Bearer [REDACTED]' \
 -d '{"model":"openai/gpt-4o-mini","stream":false,"messages":[{"role":"user","content":"hi"}]}'
# responses path (expect 401; body may name model, still terminal Auth):
curl -m 20 -X POST https://opencode.ai/zen/go/v1/responses \
 -H 'Content-Type: application/json' -H 'Authorization: Bearer [REDACTED]' \
 -H 'x-opencode-session: lane4-probe' \
 -d '{"model":"muse-spark","input":[{"role":"user","content":"hi"}],"stream":false,"store":false}'
rg -n "RateLimited|429|Auth|401" /tmp/lane4_grayhome/logs/gray.log*
```

→ skipped: live multi-turn cache measurement, per-171-provider probing, gateway cost parity. Add when real keys + TTY/CI available.
