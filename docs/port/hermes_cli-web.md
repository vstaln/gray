# Port recon: `hermes_cli/` WEB CLUSTER

Status: PARTIAL (recon agent, first pass). Labels: PROVEN = read directly; CONJECTURED = inferred.

## 1. Framework & shape

PROVEN: **FastAPI** on Starlette (`web_server.py` imports `FastAPI`, `APIRouter`, `CORSMiddleware`, `StaticFiles`; comment "single-process FastAPI"). One global `app = FastAPI(title="Hermes Agent", version=..., lifespan=_lifespan)` at line 492 of `web_server.py` (19,651 lines).

### Startup / lifecycle vs routing in web_server.py
- `_lifespan(app)` (line 354, PROVEN contents): initializes `app.state.event_channels` (dict[str,set] pub/sub fan-out), `event_lock`, `pty_active_session_files`, `chat_argv_lock` (serializes npm install/build across pty connects); spawns daemon thread to eagerly reconcile session `state.db` schema before first poll; warms gateway module; records boot code-skew fingerprint; desktop-mode cron firing.
- App-state accessors: `_get_event_state`, `_get_chat_argv_lock` (asyncio.Lock), `_get_pty_active_session_files` (~lines 452–492).
- Middleware registration ~lines 829–1012: auth gating middleware registered LAST so it runs FIRST ("Starlette middleware is outermost-last"); CORS; token middleware.
- `mount_spa(application)` (line 17702): static Vite/React frontend mount + catch-all.
- Route-order sensitivity is explicit: `{session_id}` path templates must register before later generic routes (comment at 12273); custom range-handling FileResponse wrapper (line 2781).
- WebSocket routes are defined directly on `app`, NOT via routers.

### HTTP route inventory (decorator counts from grep)

**web_server.py (app-level, ~110+ endpoints)** by area:
- media/chat-image-upload: `/api/media`, `/api/chat/image-upload`
- files/fs: `/api/files*` (read, download, stream, upload, upload-stream, mkdir, DELETE), `/api/fs/*` (list, read-text, write-text, read-data-url, download, git-root, default-cwd)
- status/ops: `/api/ssh/ownership`, `/api/health`, `/api/status`, `/api/system/stats`, `/api/curator*`, `/api/learning/*`, `/api/portal`, `/api/ops/{prompt-size,dump,config-migrate,debug-share,doctor,security-audit,backup,backup/download,import,import-upload,hooks,checkpoints,checkpoints/prune}`
- gateway/update: `/api/gateway/{restart,drain,start,stop}`, `/api/hermes/update{,/check,/receipt}`
- audio: `/api/audio/transcribe`, `/api/audio/voice-config`, `/api/audio/elevenlabs/voices`, `/api/audio/speak`
- actions: `/api/actions/{name}/status`
- memory: `/api/memory`, `/api/memory/provider`, `/api/memory/reset`, `/api/memory/providers/{name}/{config,setup}`
- config/env/model/providers: `/api/config{,/defaults,/schema,/raw}` (GET/PUT), `/api/egress/status`, `/api/model/{info,options,recommended-default,auxiliary,moa,set}`, `/api/env` GET/PUT/DELETE + `/reveal`, `/api/providers/{custom-endpoints...,validate,oauth...}`
- messaging onboarding: whatsapp/telegram onboarding start/status/apply/delete, `/api/messaging/platforms...`
- logs/pairing/webhooks/credentials: `/api/logs`, `/api/pairing*`, `/api/webhooks*`, `/api/credentials/pool*`
- analytics: `/api/analytics/{usage,models}`
- dashboard prefs/plugins: `/api/dashboard/{themes,theme,font,plugins,...}`, `/api/dashboard/plugins/{name:path}/visibility`, `/dashboard-plugins/{plugin_name}/{file_path:path}` (static plugin assets)

**WebSocket endpoints (all app-level, PROVEN):**
| Endpoint | Purpose |
|---|---|
| `/api/audio/speak-stream` | streaming TTS |
| `/api/console` | interactive console |
| `/api/pty` | PTY sessions |
| `/api/ws` | general ws |
| `/api/pub` | publish |
| `/api/events` | event stream |

WS auth note (line 16235): "FastAPI HTTP middleware does not run for WebSocket routes" — hence `dashboard_auth/ws_tickets.py` (mint/consume short-lived tickets) and `internal_ws_credential`.

**web_routers/ (each exposes `router = APIRouter()`):**
- cron.py (14 routes): `/api/cron/jobs` CRUD + pause/resume/trigger/runs, `/api/cron/delivery-targets`, `/api/cron/fire`, `/api/cron/blueprints(/instantiate)`
- git.py (20 routes): `/api/git/{status,gh-auth,worktrees,branches,base-branches,file-diff,branch/switch}`, `/api/git/review/*` (list, diff, commit-context, rev-parse, ship-info, pr-list, stage, unstage, revert, commit, push, create-pr), `/api/git/worktree/{add,remove}`
- mcp.py (11 routes): `/api/mcp/servers` CRUD/test/auth/enabled, `/api/mcp/oauth/flows/{flow_id}`, `/api/mcp/oauth/callback/{server_name:path}`, `/api/mcp/catalog(+/install)`
- profiles.py (2 routers, 15 routes): `/api/profiles` CRUD + soul/description/model/setup-command/open-terminal/describe-auto/export/import/desktop-overlay, `/api/profiles/active`
- sessions.py (3 routers, PROVEN): list_router `GET /api/sessions`; search_router `GET /api/sessions/search`; manage_router 12 routes incl. `/api/sessions/{session_id}` (GET/DELETE/PATCH), `/messages`, `/latest-descendant`, `/export`, bulk-delete, import, empty count/delete, stats, prune
- skills.py (hub_router + router, 5 explicit routes): `/api/skills`, `/toggle`, `/content`, POST/PUT
- tools.py (12 routes): `/api/tools/toolsets*` (config/models/model/provider/env/post-setup), `/api/tools/terminal/backends|backend`, `/api/tools/computer-use/{status,permissions/grant}`

**dashboard_auth/routes.py**: `/login`, `/auth/login`, `/auth/native/authorize`, `/auth/callback`, `/api/auth/providers` (+ token routes registered dynamically via `token_auth.register_token_route(path)`).

## 2. Deps reaching OUTSIDE the cluster

PROVEN (from module names + grep hits):
- hermes_cli core: `config.py`, `profiles.py`, `cron.py`, `mcp_config.py`, `kanban_db.py`(?), `session_*`, `hooks.py`, `security_audit.py`, `backup.py`, `agent_plugins.py`, `model_catalog.py`, `voice.py`, `gateway.py`, `web_deps.py`, `web_git.py`, `web_models.py`, `dashboard_procs.py`, `webhook.py`
- Agent runtime layer (the big one): chat/streaming engine behind `/api/chat` family and `/api/console` — lives outside this cluster (console_engine.py etc.)
- Process management: `dashboard_procs.py` spawns/supervises child processes (gateway, profiles)
- Git plumbing: `web_git.py` wraps git CLI for web_routers/git.py

CONJECTURED: exact import list per module not yet enumerated (refinement pass pending).

## 3. PyPI → Rust crates

| Python | Rust |
|---|---|
| fastapi + uvicorn | axum (tower-http for CORS/static/ranges); actix-web acceptable but axum's extractor model maps closer to FastAPI DI |
| starlette middleware | axum middleware (`tower::Layer`) or `axum::middleware::from_fn` |
| WebSockets (fastapi websocket routes) | axum::extract::ws (tokio-tungstenite underneath) |
| SSE/event streams | axum responses with `Body::from_stream` / async-stream |
| itsdangerous-style cookie signing | no direct stdlib; use `itsdangerous` crate (exists on crates.io) or hand-roll HMAC-SHA256 signed cookies with `hmac`+`sha2`+`base64` |
| jinja templates (login_page.py renders HTML — appears to be f-string/templated HTML, not full Jinja app) | askama if real templating needed; else format!() — login page is one self-contained string builder |
| Pydantic models (web_models.py) | serde structs + validator/garde where validation matters |
| python-multipart uploads | axum multipart extractor |
| psutil-style system stats (/api/system/stats) | sysinfo crate |

## 4. Port order

1. **Skeleton**: axum app, config loading, `/api/health`, `/api/status`, static SPA mount (`mount_spa`).
2. **Auth stack**: cookies.rs, prefix.rs, registry.rs (provider trait = `DashboardAuthProvider` ABC → trait), middleware.rs (gated_auth_middleware as tower layer), token_auth.rs, audit.rs. Everything else sits behind it.
3. **Read-only CRUD routers** (low risk, pure JSON): git, cron (read paths), skills, tools, mcp, profiles, sessions listing.
4. **Config/env/model mutation endpoints** (careful: they mutate shared config files).
5. **WebSockets**: `/api/events` (pub/sub fan-out) → `/api/console` → `/api/pty` (hardest; needs portable PTY story) + ws_tickets.
6. **Long-poll/streaming**: file stream/upload-stream, audio speak-stream, chat streaming bridge to the agent runtime (depends on runtime port landing first).

## 5. Rust risks

- **Dynamic route registration**: `register_token_route(path)` mutates a global set consulted by middleware; profiles/sessions expose multiple routers assembled at import time; plugin visibility uses `{name:path}` wildcards. Axum needs all routes known at Router build time → collect dynamic sets BEFORE building the router, keep the token-route set as shared state (RwLock) checked in middleware rather than as routes.
- **Registration-order routing semantics**: Starlette matches in registration order; the code deliberately exploits it (line 12273). Axum matches most-specific-first — port must re-verify every case that leaned on order.
- **Middleware state**: gated_auth_middleware reaches into provider registry, refresh flows, SSO auto-login; it's imperative and stateful (in-memory pending OAuth states, native_flow._Pending/_IssuedCode dicts with locks). Port as an extension/trait-object map in axum State.
- **App-state pattern**: `_get_event_state(app)`, per-app asyncio.Lock — becomes Arc<State> fields; asyncio.Lock → tokio::sync::Mutex.
- **WS without HTTP middleware**: Python solves ws-auth via tickets; keep same design (don't try to force axum middleware onto ws upgrades).
- **Streaming uploads/downloads with range handling**: custom FileResponse wrapper reimplements Starlette range logic (line 2781) — tower-http `ServeFile`/`Range` covers part; chat image/import-stream need careful body handling.
- **Single-process in-memory assumption**: sessions kept in-memory only (line 11213) — fine for 1:1, blocks any multi-worker story later.

## 6. Load-bearing vs peripheral

LOAD-BEARING (port fidelity matters):
- web_server.py middleware ordering + auth gating + lifespan state
- dashboard_auth whole directory (cookies, ws_tickets, native_flow PKCE-ish flow, registry, token_auth) — security surface
- WS endpoints console/pty/events (core UX)
- chat/streaming bridge into the agent runtime
- config/env/model PUT endpoints (mutate live system)

PERIPHERAL (thin wrappers, port late or trivially):
- web_routers/git.py (subprocess git wrapper), cron.py, skills.py, tools.py, mcp.py — mostly JSON shims over existing CLI-layer functions
- dashboard themes/font/plugin-preferences endpoints (cosmetic)
- `/dashboard-plugins/{path}` static asset serving
- login_page.py (one HTML string builder)
re ordering + auth gating + lifespan state
- dashboard_auth whole directory (cookies, ws_tickets, native_flow PKCE-ish flow, registry, token_auth) — security surface
- WS endpoints console/pty/events (core UX)
- chat/streaming bridge into the agent runtime
- config/env/model PUT endpoints (mutate live system)

PERIPHERAL (thin wrappers, port late or trivially):
- web_routers/git.py (subprocess git wrapper), cron.py, skills.py, tools.py, mcp.py — mostly JSON shims over existing CLI-layer functions
- dashboard themes/font/plugin-preferences endpoints (cosmetic)
- `/dashboard-plugins/{path}` static asset serving
- login_page.py (one HTML string builder)
odel PUT endpoints (mutate live system)

PERIPHERAL (thin wrappers, port late or trivially):
- web_routers/git.py (subprocess git wrapper), cron.py, skills.py, tools.py, mcp.py — mostly JSON shims over existing CLI-layer functions
- dashboard themes/font/plugin-preferences endpoints (cosmetic)
- `/dashboard-plugins/{path}` static asset serving
- login_page.py (one HTML string builder)
