# Multi-account in gray — design + implementation notes

Status: implemented (MVP) — see "What landed" at the bottom.

## Why

The user holds several accounts per provider (multiple Claude plans, multiple
ChatGPT/Codex logins, …). Today gray can connect **one** credential per
provider. Three plugins (`gray-claude-sub`, `gray-cursor-sub`,
`gray-devin-sub`) already grew private, copy-pasted account pools with a CLI
(`gray <plugin> account add|list|use|remove|status`), but nothing in the host
knows they exist — no UI, no shared contract, and every other `*-sub` plugin
has no pooling at all.

## Prior art (read the code, not the READMEs)

| | 9router (`reference/9router`) | t3code (`research/t3code`) | gray today |
|---|---|---|---|
| Account storage | SQLite `providerConnections` table — one row per account: `id, provider, authType(oauth/apikey), name, email, priority, isActive, data{accessToken,refreshToken,expiresAt,rateLimitedUntil,backoffLevel,modelLock_<m>,consecutiveUseCount,lastUsedAt,…}` (`src/lib/db/schema.js`, `repos/connectionsRepo.js`) | `ServerSettings.providerInstances: Map<instanceId,{driver,config,enabled}>` — user-defined instances (`claude_personal`/`claude_work`); credentials are opaque bytes in an OS secret store keyed by `sha256(driver:bindingId)` (`ProviderCredentialStore.ts`, `ProviderInstanceRegistry.ts`, `contracts/providerInstance.ts`) | Two disjoint worlds: (a) flat `~/.gray/auth.json` — bare API-key strings + exactly ONE `CredentialEnvelope` per `plugin:<lock>:<provider>:<method>`; (b) plugin-private pools `<gray-home>/<plugin-state>/accounts.json` (3 plugins, 3 near-identical forks of the same code) |
| Identity/dedup | OAuth rows deduped by email + workspace id; codex dedupes on `chatgptAccountId` because rotated refresh tokens are single-use (merging a second grant kills the first — documented scar) | instance id IS the identity; credentials never deduped | plugin pools dedupe on verified email+org only (null never proves equality) |
| Selection | pinned (`preferredConnectionId`) > `round-robin` (sticky N uses, LRU) > `fill-first` (priority order); per-provider strategy override | user picks the instance per thread; no automatic rotation | same model already: `selected` pin > `fill_first`/`least_used` > all-cooled error naming earliest reset |
| Failure/cooldown | rule table `ERROR_RULES` (text + status), `checkFallbackError`: 401/402/403/404/429 cool per-**model** (`modelLock_<m>`) with exp. backoff (1s→4min); other 4xx never cool (deliberate: don't mask request bugs); all-locked → `retryAfter` | per-instance `auth.status` + usage windows parsed from SDK `get_usage`/`rate_limit_event` (5h session, 7d weekly) | 401→5m, 429→parsed `retry-after`/`resets …` hint else 1h, capped at 8d, lone account capped at 60s; request-scoped failures never cool |
| Usage | `usageHistory` rows carry `connectionId` | windows per instance | `request_count`/`last_used_at` per account; usage plugin-side |
| Connect UI | Next.js dashboard: `/api/oauth/[provider]/{authorize,exchange,device-code,poll}` + local callback proxies + connection rows with cooldown timers, bulk-import modals | in-app provider panel drives `ProviderAuthService`; `CodexAuthCallback` page | `/connect` modal → `run_plugin_login` → auth_start/poll (OAuth plugins) or external-login probe (CLI plugins); **no account-level UI at all** |

## What's broken now (live evidence)

- `claude-*` chat 401s ("provider rejected the credentials") although
  `claude auth status` says `loggedIn: true` — the ambient access token is
  rejected upstream (expiry was ~5h out at triage; classic rotated-token
  reuse symptom). With a single ambient login there is nowhere to fall back.
- `~/.gray/auth.json` holds **two** envelopes for the same provider:
  `plugin:claude-sub:…` (orphan — old lock name) and `plugin:claude:…`
  (live). auth_ref keys bake in the lock-name, so plugin renames orphan
  credentials. Pool state lives under the *manifest* name
  (`~/.gray/claude-sub/`) while auth_ref uses the *lock* name (`claude`).
- Plugin pools are invisible to the host: no listing, no pin, no add from
  the UI; a new user must discover `gray claude-sub account add` by hand.

## Target design

### Two credential-ownership models (kept distinct on purpose)

1. **CLI-home pools** — providers whose CLI partitions credentials by
   home/env (`CLAUDE_CONFIG_DIR`, staged `HOME`, `XDG_DATA_HOME`):
   claude-sub, cursor-sub, devin-sub, and any future plugin that spawns a
   native CLI. The pool stays **plugin-owned**: the plugin alone knows how
   to spawn a login into a fresh home and how its CLI reports errors.
2. **Vault pools** — plugins that do `provider/auth/start` → `auth_poll`
   and return `CredentialMaterial` (codex-sub today). Materials live in
   `auth.json`, so multi-account = multiple labeled envelopes under one
   `plugin:*` auth_ref. The host picks the serving account and hands the
   chosen envelope to `provider/chat` — **no RPC shape change needed**.

### auth.json v2 (vault pools)

```jsonc
"plugin:<lock>:<provider>:<method>": {
  "version": 2, "plugin": "…", "provider": "…", "auth_method": "…",
  "selected": "work" | null,
  "strategy": "fill_first" | "least_used",
  "accounts": {
    "work": {
      "profile_binding": "…", "credential": {secrets, metadata, expires_at},
      "email": "…", "added_at": 0,
      "cooldown_until": 0, "last_error": null,
      "request_count": 0, "last_used_at": 0
    }
  }
}
```

- v1 entries (`CredentialEnvelope` shape) migrate in memory on load and
  persist as v2 on the next write; the first write that would migrate takes
  `auth.json` → `auth.json.v1.bak` once. `read_plugin(auth_ref)` keeps its
  signature and returns the *serving* envelope — every existing caller is
  transparently pooled.
- `put_plugin` keeps meaning "the login you just completed is the serving
  account" (upserts `selected`/`default`); `put_plugin_account` adds a
  labeled slot for connect-another-account.

### Selection policy (same rulebook both models)

1. `selected` pin: a usable pinned account always serves.
2. Strategy among non-cooled: `fill_first` (insertion order) or
   `least_used` (oldest `last_used_at`).
3. All cooled → error naming the earliest reset ("all N accounts cooling;
   next at …"), never a silent hang.
4. Cooldowns only for account-scoped verdicts: 401→5m, 429→parsed hint else
   1h; other 4xx / transport errors never cool. Lone account caps at 60s.
5. Per-account `request_count`/`last_used_at` written through on every pick.

### Plugin contract — `account` verb family

Every provider plugin that pools implements (Claude/Cursor/Devin already
do; the `--json` flag is new):

```
<plugin> account list    --json → {version,plugin,provider,strategy,selected,
                                   ambient:bool,
                                   accounts:[{label,home?,email,org,added_at,
                                              cooldown_until,last_error,
                                              request_count,last_used_at}],
                                   serving:{kind,label,reason}}
<plugin> account status  --json → same shape (cheap probe)
<plugin> account use <label|--clear>        (pin / unpinned)
<plugin> account remove <label>
<plugin> account strategy <fill_first|least_used>
<plugin> account add <label> [--home dir]   (TTY: native login into a fresh
                                           managed home; --home adopts an
                                           existing credential home)
```

Discovery: the panel runs `<cli_argv> account list --json`; a plugin
without the verb exits nonzero and is listed as "single login" — zero
manifest bump needed. That is how every provider gets the UI "for free":
implement one verb family over your own pool, the host panel lights up.

## UI decision: TUI panel (not a web dashboard, not the desktop app)

- gray is a terminal-first tool; `/connect`, `/plugins`, `/gateway`,
  `/agents` are all TUI modals. `/accounts` joins that family.
- `account add` needs a **real TTY** — the provider CLI's own interactive
  login (browser/device code + paste-back). A local web dashboard would
  have to bridge a PTY; the TUI just drops the alt screen and hands the
  terminal to `<cli> account add`, exactly like a shell-out editor.
- 9router needs a web UI because it *is* a headless proxy server; gray's
  dashboard surface is the TUI. A web/desktop UI can still come later on
  the same `account --json` contract (the panel's data layer is just
  `spawn_raw(argv, ["account","list","--json"])` — the agents_panel pattern).

### Screens

```
/accounts  (bare → modal; no TTY → plain-text listing, like /gateway)

┌ Accounts ──────────────────────────────────────────────────┐
│ claude (claude-subscription)            strategy fill_first│
│   ● work      r…@gmail.com · Rich Clinton   41 uses  2h ago│
│     personal  r…@corp.com    cooling 23m (usage limit …)   │
│   + add account                                            │
│ codex (codex)                            vault · fill_first│
│   ● default   chatgpt account            verified          │
│   + add account                                            │
│ cursor — single login via /connect                         │
└────────────────────────────────────────────────────────────┘
 ↑↓ select · enter/u pin · c clear pin · a add · d remove ·
 s strategy · r refresh · q close
```

Add flow: prompt for `label [home]` → no home ⇒ leave alt screen, spawn
`<cli> account add <label>` inheriting stdio (native `auth login` runs in
the real terminal), re-enter, relist. With a home ⇒ `account add <label>
--home <dir>` runs non-interactively (adopt).

## What landed (this run)

- `~/.gray/auth.json` v1→v2: `plugin:*` entries become account pools
  (`accounts` map + `selected` + `strategy`); `CredentialStore` migrates on
  first write with an `auth.json.v1.bak` backup; `read_plugin` serves the
  pool pick; `acquire()` tags the lease with the serving account label.
- `claude-sub`: `account` verbs accept `--json`.
- Host: `/accounts` TUI panel + text fallback; discovers pool-capable
  plugins via lock `cli_argv`; pin/clear/remove/add/strategy all route
  through the `account` CLI contract.
- Verified in a scratch GRAY_HOME (see final report).

## Left for later (deliberately out of scope)

- Per-**model** locks (`modelLock_<m>`) — 9router-level nuance; pools here
  cool per account only.
- codex-sub/vault add-account UI path (needs auth_start-with-label in the
  panel) — schema + store APIs are in place.
- Extract the three plugin pool forks into a shared crate — worth doing
  once this branch can bump gray-plugin's rev.
- Quota-aware selection (t3code windows) — plugin pools already record
  resets; a `quota` field could feed selection later.
- Orphaned `plugin:claude-sub:*` entry in live auth.json — harmless; a
  `gray accounts prune` verb could sweep refs that name missing lock
  entries.
