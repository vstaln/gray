# System One ecosystem — what exists, so gray doesn't rebuild it

Snapshot: 2026-10-05. Sources: [cobanov/awesome-jev](https://github.com/cobanov/awesome-jev)
(155 entries, source-reviewed 2026-09-20) and [madewithjev.com](https://madewithjev.com) (755
builds). Descriptions, benchmarks, and stars are the authors' claims; licenses are unverified
unless noted. Re-check a repo before vendoring code from it.

**The key fact:** `POST /v1/systemone` (TypeSafe's wire: `{state, model, questions}` →
`{answers, usage}`) is the de facto standard. Most open models and local servers below accept it
unchanged, so this plugin is one client with a configurable base URL — not a backend zoo.

## 1. Backends

### Hosted decision APIs

| Backend | Wire | Notes |
|---|---|---|
| TypeSafe Jev | `api.typesafe.ai/v1/systemone` | Text only. Model `jev-latest`. |
| OpenRouter | `typesafe/jev-1.13`, `typesafe/jev-latest` | Jev via OpenRouter billing. |
| Cloudflare Workers AI | `typesafe/jev` | Provider-maintained integration. |
| Vercel AI Gateway | `typesafe-ai/jev` | Via AI SDK's experimental `evaluate`. |
| Netlify AI Gateway | `@typesafe-ai/sdk` | Zero-config from Netlify Functions. |
| Perplexity Decisions | `POST api.perplexity.ai/v1/decisions` | `pplx-decider-v1-27b`, images (base64), 1–128 questions. **Different shape — needs an adapter.** |
| Cloudflare Clef | Workers AI | Multimodal, Apache-2.0 open weights on HF, up to 64 questions. |
| OpenAI Decisions | unpublished | Limited preview (GPT-6 Luna). |
| CUA-S1 ([trycua/cua](https://github.com/trycua/cua)) | — | Open System One family for computer use; CUA-S1-FORMS first. |

### Local servers / open models speaking `/v1/systemone`

| Repo | Notes |
|---|---|
| [ollaya-dev/ollaya](https://github.com/ollaya-dev/ollaya) | **Default backend.** Rust, Apache-2.0 (verified). Ollama-style daemon on `:11435`: `pull`/`run`/`serve` laya, winnow:e4b, kev, decider. Serves `/v1/systemone`, `/v1/decisions`, `/v1/models`. `laya` runs on CPU. Reports winnow:e4b 0.722 vs Jev 0.738. Also `ollaya mcp` + an `ollaya-decisions` skill. |
| [ARCJ137442/jev-switch](https://github.com/ARCJ137442/jev-switch) | Rust gateway routing `/v1/systemone` to Vercel or Laya by configured routes. |
| [bladedevoff/stuntd](https://github.com/bladedevoff/stuntd) | Proxy: serves from Laya; in front of Jev, records answers and trains per-question heads. |
| [bradAGI/ruling](https://github.com/bradAGI/ruling) | Serves `/v1/systemone` from an MLX model **or any OpenAI-compatible endpoint** via option logits — the "use the LLM you already have" fallback. |
| [MorrisZJ/AnyJev](https://github.com/MorrisZJ/AnyJev) | Any transformers/vLLM model → Choice/Score/Noul from one prefill, no training. |
| [zhengxuyu/litjev](https://github.com/zhengxuyu/litjev) | Any Qwen → `/v1/systemone`, no training. |
| [ekzhang/openjev-sglang](https://github.com/ekzhang/openjev-sglang) | Jev-compatible API on SGLang, prefill-only. |
| [amyrmahdy/decima](https://github.com/amyrmahdy/decima) | Apache-2.0 122M multilingual, int8 ONNX, ~20 ms/4 options on one CPU core. |
| [getainode/jebadiah](https://github.com/getainode/jebadiah) | Apache-2.0 27B/9B/4B (bf16, GGUF, MLX); standalone server. |
| [lawrence3699/jev-style](https://github.com/lawrence3699/jev-style) | 0.8B, `pip install "jev-style[torch]"`; includes a Claude Code guard. |
| [feder-cr/jev](https://github.com/feder-cr/jev) (jevos) | 619 MB GGUF, CPU-only, noul only. |
| [OmniJev/OneJev](https://github.com/OmniJev/OneJev) | Multimodal 0.8B–27B; reports 27B 89.3 vs Jev 1.13 89.0 on 354 questions. |
| [Xiaooolong/vev](https://github.com/Xiaooolong/vev) | Vision 4B/9B; weights CC BY-NC (non-commercial). |
| [SamratDuttaOfficial/WaterSheep](https://github.com/SamratDuttaOfficial/WaterSheep) | Apache-2.0; official TypeSafe SDK works unchanged. |
| [Shanghua-Gao/RSI-Jev](https://github.com/Shanghua-Gao/RSI-Jev) | 2B, text + up to 4 images. |
| [rupeshpoojary9/poorjev](https://github.com/rupeshpoojary9/poorjev) | NLI-based, offline, no key, conformal abstention. |
| [hotchpotch/bekko-system-one](https://github.com/hotchpotch/bekko-system-one) | 17M–400M English models + training code. |
| [r33drichards/laya-vision](https://github.com/r33drichards/laya-vision) | Laya fork with SmolVLM image input. |

Rust client crate: `rig-typesafeai` in [0xPlaygrounds/rig](https://github.com/0xPlaygrounds/rig).

## 2. Logic to lift, by gray feature

### Skill routing (needs turn text in `prompt/context` — host change)
- [shimo4228/jev-skill-router](https://github.com/shimo4228/jev-skill-router) — UserPromptSubmit hook: one Choice over the skill roster + Noul gates; shadow mode logs only.
- [Dicklesworthstone/skillranker](https://github.com/Dicklesworthstone/skillranker) — Rust CLI, ranks skills against session context, can abstain.
- [deyna256/langchain-skill-router](https://github.com/deyna256/langchain-skill-router) — rank then verify top candidates; judge is a protocol.
- TypeSafe cookbook: [skill suggestion](https://docs.typesafe.ai/cookbooks/skill_suggestion.md) — two requests, roughly halves wrong skill loads.

### Model / effort routing
- [xinyao27/jevonian](https://github.com/xinyao27/jevonian) — OpenAI/Anthropic-compatible proxy; `jevonian/auto` picks route + thinking level. **A gray provider can point at it with zero gray code.**
- [ruban-24/switchboard](https://github.com/ruban-24/switchboard) — pins model+effort per conversation to protect the prompt cache.
- [WXK-AI/jev-opus](https://github.com/WXK-AI/jev-opus) — per-message effort statement, cache-safe.
- [Yaxin9Luo/spending-effort-with-jev](https://github.com/Yaxin9Luo/spending-effort-with-jev), [suenot/codex-jev-router](https://github.com/suenot/codex-jev-router), [gargpratyush/jev-router](https://github.com/gargpratyush/jev-router), [BillionsBobby/JevRouter](https://github.com/BillionsBobby/JevRouter).

### Tool-call guard (permissions plugin, `tool/before`)
- [RiskAverseTech/toolgate](https://github.com/RiskAverseTech/toolgate) — 7 Noul risk questions per call, static rules first, fails closed, tracks write-then-execute.
- [eugeniughelbur/jev-engineering](https://github.com/eugeniughelbur/jev-engineering) — hard rules then Jev; ships a 300-call injection kit with results.
- [seb4ez/jevguard-mcp](https://github.com/seb4ez/jevguard-mcp), [leepokai/jev-guard](https://github.com/leepokai/jev-guard).
- Caveat for gray: `tool/before` errors deny the call; a remote judge needs a local fallback.

### Search (`semantic_find`)
- [can1357/jegrep](https://github.com/can1357/jegrep) — Rust semantic grep, no index/daemon.
- [kyu1204/jgrep](https://github.com/kyu1204/jgrep) — one Noul per 5–60 line chunk, 16 chunks per request.
- [bartlomein/oko](https://github.com/bartlomein/oko) — keyword shortlist, Jev judges chunks.
- [ellipsis-dev/blink](https://github.com/ellipsis-dev/blink) — Jev-guided walk over file/dir names.
- [shinpr/jev-reranker](https://github.com/shinpr/jev-reranker) — Rust JSON-in/out reranker.
- [allebee/jevgrep](https://github.com/allebee/jevgrep) — Noul per log line, works on `tail -f`.
- TypeSafe cookbook: [line-by-line search](https://docs.typesafe.ai/cookbooks/semantic_find.md).

### Compaction / context
- [tamaratran/fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction) — score tool-call/result pairs for delete/truncate, keep the rest verbatim.
- [radqnico/opencode-jev-compaction](https://github.com/radqnico/opencode-jev-compaction), [joelhooks/pi-fast-jev-compaction](https://github.com/joelhooks/pi-fast-jev-compaction), [ilkerulusoy/pi-jev-compact](https://github.com/ilkerulusoy/pi-jev-compact), [GhalebDweikat/winnow](https://github.com/GhalebDweikat/winnow), [compozy/yoshi](https://github.com/compozy/yoshi).

### "Done" claims / supervision
- [valentynkit/jev-belay](https://github.com/valentynkit/jev-belay) — Stop hook: checks for evidence before trusting "done", fails open.
- [qkal/Canny](https://github.com/qkal/Canny), [VeridicalTech/Edward](https://github.com/VeridicalTech/Edward), [DevMortimer/pi-warden](https://github.com/DevMortimer/pi-warden), [shitianfang/wakegate](https://github.com/shitianfang/wakegate).

### Browser / computer use
- [browser-use/jev-ultrafast](https://github.com/browser-use/jev-ultrafast) — Jev picks operation + DOM target, small LLM types text.
- [awlevin/typesafe-computer-use](https://github.com/awlevin/typesafe-computer-use) — macOS, OCR + bounded Jev actions, OSWorld bench.
- [GoldenLoaf24h/browserclaw](https://github.com/GoldenLoaf24h/browserclaw) — Chrome MCP over live sessions.
- [chy4pro/jev-for-chrome](https://github.com/chy4pro/jev-for-chrome), [sedum-dev/sedum](https://github.com/sedum-dev/sedum), [droidrun/mobile-jev](https://github.com/droidrun/mobile-jev), [wy-coliney/jev-browser-use](https://github.com/wy-coliney/jev-browser-use), ThinkFlowLab/system1-agents (Jev + Laya + CUA-S1 behind one interface).

### Agent-wide decision layers
- [shitianfang/jev-use](https://github.com/shitianfang/jev-use) — batched questions + tool risk checks, typed escalation back to the LLM.
- [Brainwires/jevwire](https://github.com/Brainwires/jevwire), [parkavenue9639/jevloop](https://github.com/parkavenue9639/jevloop), [y0usaf/pi-jev](https://github.com/y0usaf/pi-jev).

## 3. Fit with gray today
- **MCP servers** (jevwire, jev-mcp, typesafe-mcp, `ollaya mcp`): not drop-in — gray has no MCP client.
- **pi extensions** (pi-jev, pi-heed, pi-jev-compact): not drop-in — `gray plugin install git:`
  skips pi `extensions/*.ts` (`crates/gray-pkg/src/ops.rs`). Port the logic.
- **Skills** install directly: [typesafe-ai/skills](https://github.com/typesafe-ai/skills),
  Ollaya's `ollaya-decisions`.

## 4. More indexes
- [rupeshpoojary9/awesome-open-system-one](https://github.com/rupeshpoojary9/awesome-open-system-one) — open models only.
- [laya.tools](https://laya.tools) — ~950 projects on Laya.
- [punk2898/awesome-jev-verified](https://github.com/punk2898/awesome-jev-verified) — each entry pinned to the line that calls Jev.
- [Jessie-QingYu/jev-in-the-wild](https://github.com/Jessie-QingYu/jev-in-the-wild) — JSON data incl. failed results.
- [vicfei/awesome-jev-prompts](https://github.com/vicfei/awesome-jev-prompts) — 43 question patterns + anti-patterns.
- [OmniJev/awesome-jev](https://github.com/OmniJev/awesome-jev), [AbdelStark/awesome-typesafe](https://github.com/AbdelStark/awesome-typesafe), [yibie/awesome-jev](https://github.com/yibie/awesome-jev).
