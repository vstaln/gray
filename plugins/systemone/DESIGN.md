# systemone — design (v1: computer use)

Status: **draft for approval** · 2026-10-05 · tier: standard (substantial, reversible).

## 0. Contract

User decisions (2026-10-05):

| # | Decision | Choice |
|---|---|---|
| D1 | Where the machinery lives | systemone plugin + general host hooks, added only when a feature needs one |
| D2 | v1 feature | **computer use** |
| D3 | Default backend | hosted Jev (`api.typesafe.ai`, `jev-latest`); any `/v1/systemone` server via env |
| D4 | Rollout | shadow first: dry-run proposes, never executes until enabled |
| D5 | Scope | **browser + desktop** |
| D6 | Who types text and verifies "done" | gray's own model (hand-back) |
| D7 | Engine | port into the plugin; reuse OSS heavily, make it ours |
| D8 | Browser profile | the user's real Chrome |
| D9 | "Add an extension" | **Chrome (MV3) extension** (confirmed) — the only robust route into the real profile (see §3). A GraySpace panel comes later. |

Out of scope for v1: skill routing, tool-call risk gate on all tools, context trimming,
model routing (they need the deferred host hooks, §8). Wayland. Windows/macOS.

## 1. Facts this design rests on

- Protocol `1.1` sidecars get a **330 s** `tool/call` TTL (`crates/gray-plugin/src/sidecar.rs:660`,
  `ASK_TTL`), so a bounded multi-step run fits in one tool call.
- Plugins can block for a user answer via `host/ask` (`crates/gray/src/host.rs`, `HOST_ASK`),
  served concurrently with an in-flight `tool/call` (sidecar reader dispatches `host/*` to handler
  tasks). It is capability-gated: without a declared **and granted** `host.ask` the transport
  replies `{"error":"capability_not_granted: host.ask"}` (`sidecar.rs:365`). Other failures also
  arrive as `result.error`: no handler (`sidecar.rs:407`, e.g. under `gray plugin check`), ask
  service not installed outside the REPL (`crates/gray/src/ask.rs:168`), 300 s handler timeout.
- Host limits: wire frames > 256 KiB are dropped (`MAX_FRAME`, `sidecar.rs:287`); tool content is
  truncated at 50 KiB (`gray-core/src/tool_out.rs`, `MAX_BYTES`).
- Google Chrome ≥ 136 ignores `--remote-debugging-port` on the default user-data-dir
  ([Chrome blog](https://developer.chrome.com/blog/remote-debugging-port)); per
  `chrome/browser/devtools/remote_debugging_server.cc` (main) the check is compiled in only under
  `GOOGLE_CHROME_BRANDING`, so Chromium builds still honor the flag. Real-profile routes are a
  `chrome.debugger` extension or the `chrome://inspect` toggle.
- `chrome.debugger` works in a normal profile without flags, but: Chrome shows a "started
  debugging this browser" bar the user can dismiss (detaches the session), it conflicts with an
  open DevTools on the same tab, and it cannot attach to `chrome://` pages or the Web Store.
- Native messaging: Chrome spawns the host from
  `~/.config/google-chrome/NativeMessagingHosts/<name>.json`, `allowed_origins` pins extension IDs
  (no wildcards) ([docs](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging)).
- This machine: Void Linux, X11 (`DISPLAY=:0`), `at-spi2-core` running, `xdotool`, `scrot`,
  Playwright Chromium (`~/.cache/ms-playwright/chromium-1187`), Chrome 150 profiles in
  `~/.config/google-chrome`; no OCR engine; `TYPESAFE_API_KEY` unset.
- Jev is text-only; one request ≤ 64k tokens; Choice ≤ 255 options; Score 2–10 levels.

## 2. Architecture

```
gray agent (System 2: plans, types text, verifies)
   │ tool/call  computer_run / computer_type / computer_act / computer_observe
   ▼
gray-systemone sidecar (Rust)
   ├─ client      POST /v1/systemone (exists)
   ├─ decide      one request per step: op + speculative targets + goal/stuck nouls
   ├─ policy      code rules first; decisions only add friction; host/ask for risky steps
   ├─ trace       run folder, replayable
   └─ devices ── Device trait: observe() -> Observation, execute(Action) -> Effect
        ├─ chrome   (extension bridge)  ◄── unix socket ◄── native host ◄── MV3 extension in real Chrome
        ├─ cdp      (direct CDP: Chromium / dedicated profile / headless tests)
        └─ x11      (AT-SPI tree + OCR fallback, XTest input)
```

One loop, many devices. The loop never sees selectors, coordinates, or JS: model output is an
**index into the latest observation**; the device resolves it to a live node and re-validates
(freshness, visibility, occlusion) before acting. (Rule taken from jev-ultrafast.)

**Concurrency.** The sidecar moves to tokio in P1. The stdio wire loop never blocks on a run:
each `tool/call` is a task, replies are written through one serialized writer, and the native-host
socket listener, `host/ask` replies, `event/notify`, and `plugin/shutdown` stay live during a
300 s run. One run per device at a time; a second `computer_run` on a busy device returns
`error: device busy`. `plugin/shutdown` cancels runs and releases devices (detach debugger).

**Manifest.** `protocol: "1.1"` (330 s TTL) and `capabilities: ["host.ask"]`. Install asks the
operator to grant it (`gray plugin capabilities systemone`); if not granted, every risky step is
treated as denied.

### 2.1 Observation (device → loop)

```json
{"device":"chrome","url":"…","title":"…","app":null,
 "elements":[{"i":7,"role":"combobox","name":"Where to?","value":"","section":"Search",
              "href":null,"focused":false,"editable":true,"sensitive":null}],
 "text":"<visible text, ≤ 6000 chars>","fresh_token":"…"}
```

`sensitive`: `password` | `payment` | `otp` | null — computed by the device (input type,
`autocomplete="cc-*"`/`one-time-code`, AT-SPI role `password text`). ≤ 255 elements per target head,
ordered by visibility then document order; the rest are counted, not offered. Each element also
carries `in_form` and `submits` (`type=submit`, a button inside a `<form>`, or Enter in a form field)
for the policy (§6). The serialized observation is budgeted to ≤ 48 KiB by the device (trim text,
then low-visibility elements) so it never hits the host's 50 KiB content cap.

### 2.2 Step decision (loop → backend, one round trip)

Ported from jev-ultrafast / Jev for Chrome questions:

- `operation` Choice over the ops the observation supports: `CLICK`, `TYPE_TEXT`, `SELECT`,
  `PRESS_ENTER`, `SCROLL_UP`, `SCROLL_DOWN`, `WAIT`, `DONE`, `BLOCKED`.
- `<op>_target` Choice per op, over compatible elements only (speculative: only the chosen op's
  head is used).
- `goal_achieved` Noul and `stuck` Noul, asked independently (cross-check: a `DONE` with
  `goal_achieved < 0.5` is withheld once; same for `BLOCKED` vs `stuck`).
- State carries code-computed facts (typesafe-computer-use): goal, url/title/app, element table,
  recent actions with their visible effect ("navigated", "content changed", "no visible change"),
  visited URLs, focused field.

### 2.3 Loop and hand-back (D6)

`computer_run` loops observe → decide → policy → execute until one of these, then returns to the
gray agent with the outcome, the last observation, and a trace summary:

| Outcome | When |
|---|---|
| `need_text` | op = `TYPE_TEXT`: returns the target element + field context; the agent replies with `computer_type`. Sensitive fields return `refused` instead. |
| `done` | `DONE` and `goal_achieved ≥ 0.5`; the agent verifies from the observation. |
| `blocked` | `BLOCKED` confirmed by `stuck`, captcha/login wall, or a repeat-click loop (3× in 6 steps). |
| `uncertain` | operation confidence below threshold: top-3 ops/targets with probabilities. |
| `needs_confirm_denied` | a risky step was not approved: user declined, `host.ask` not granted, ask service absent (headless), or any ask `result.error`. Never retried within the run. |
| `limit` | step limit (default 20) or 300 s wall clock (inside the 330 s TTL). A risky step needing confirmation with < 45 s left also ends here, reporting the pending step, so an ask never outlives the TTL. |
| `unavailable` | device not ready: extension not installed/unpaired, Chrome closed, debugger detached (`onDetach`, dismissed infobar, DevTools opened), restricted page, AT-SPI bus missing. Carries a fix hint. |
| `error` | backend failure (HTTP/transport/invalid answer after one retry) or device protocol error. Partial trace kept. |

Text never comes from Jev or a second model: the gray agent writes it (no extra key).

### 2.4 Tools exposed to the gray agent

| Tool | Args | Effect |
|---|---|---|
| `computer_observe` | `{device}` | Observation only. |
| `computer_run` | `{device, goal, max_steps?}` | The loop (§2.3). In shadow mode: one decision, returned as a proposal, nothing executed. |
| `computer_type` | `{device, element, text, submit?}` | Types agent-written text into an observed element; refuses sensitive fields. |
| `computer_act` | `{device, op, element?}` | One deterministic action chosen by the agent (override path). |

`device` ∈ `chrome` | `cdp` | `desktop`. Existing `judge`, `semantic_find`, `/s1` stay.
`/s1 computer` shows device status, mode (shadow/act), and the last run's trace path.

## 3. Real Chrome: the extension (D8, D9)

`plugins/systemone/extension/` — **fork of [chy4pro/jev-for-chrome](https://github.com/chy4pro/jev-for-chrome)
(MIT, MV3)**, renamed and re-owned as "gray for Chrome".

Keep from upstream: content-script observer (element table, hit-testing, freshness), executor via
`chrome.debugger` (`Input.dispatch*`, trusted events, no implicit Enter), on-page badges and status
bar, Step/Stop, trace export.
Remove: its own Jev client, text-model client, and key storage in options — the brain lives in the
plugin, keys stay in gray's env.
Add: a `chrome.runtime.connectNative("ai.gray.systemone")` port; the extension becomes a device
that answers `observe` / `execute` and shows what gray is doing. A visible **Stop** always works.

Bridge:
- `gray-systemone --native-host` is the binary Chrome spawns. It relays length-prefixed native
  messages to the running sidecar over `$GRAY_HOME/run/systemone.sock` (dir 0700, socket 0600).
- First connect pairs with a one-time code shown in gray (`/s1 computer pair`); the extension stores
  the pairing token. No localhost HTTP/WebSocket port — no web page can reach it.
- The manifest carries a fixed `key` so the extension ID — and `allowed_origins` — is stable for
  unpacked installs.
- Install: `/s1 computer install-chrome` writes the native host manifest
  (`~/.config/google-chrome/NativeMessagingHosts/ai.gray.systemone.json`, and the Chromium path),
  then the user loads the unpacked extension once.

The extension only acts in the tab bound at run start, plus tabs/popups whose `openerTabId` is a
bound tab. `chrome.debugger.onDetach` ends the run as `unavailable`.

The native host reconnects to the socket with backoff (the sidecar respawns per gray session);
with no live sidecar it replies `unavailable: gray is not running` to the extension.

## 4. Direct CDP device (`cdp`)

For Chromium builds, a dedicated `~/.gray/chrome-profile`, and headless tests. Same observer and
executor JavaScript as the extension (one shared `observe.js`), injected via `Runtime.evaluate`;
input via `Input.dispatchMouseEvent`/`dispatchKeyEvent`. Ships first because it is testable
headless on this machine and de-risks the shared observer before the extension exists.

## 5. Desktop device (`desktop`, X11)

Perception (typesafe-computer-use step design, rebuilt for Linux):
1. AT-SPI tree of the active window via the `atspi` crate (Apache-2.0, pure Rust, zbus): role,
   name, value, states (focused, editable, password), extents. Primary source.
2. OCR fallback when the tree is empty or thin (Electron/canvas apps): screenshot of the active
   window (`x11rb`/`xcap`) → `ocrs` (Rust, rten models; license to verify) → word boxes become
   clickable `text` elements.
3. Code facts: app name, window title, focused element, dates/relative dates found in text.

Actions: `x11rb` XTest (move/click/scroll/key) — `xdotool` as a fallback shell-out. Keyboard input
goes to the focused window only after re-checking focus.

Kill switch: pointer in the top-left screen corner aborts the run (typesafe-computer-use), plus
gray's normal turn cancel.

## 6. Policy (applies to every device)

1. **Decisions only add friction.** Jev can propose; code decides whether it executes.
2. Never type into `sensitive` fields; never auto-fill credentials, payment, or OTP.
3. `host/ask` confirmation before any action that **submits or commits**, decided by action shape
   first, words second:
   - target has `submits` (submit input, button inside a form, `PRESS_ENTER` in a form field);
   - target is button-like with an empty accessible name inside a form (icon-only buttons);
   - target name/role matches pay/buy/order/checkout/delete/remove/send/publish/transfer/confirm;
   - file uploads, downloads, and any host on `SYSTEMONE_COMPUTER_DENY_HOSTS` (host-level, no path
     granularity).
   A Jev `irreversible` Noul may add a confirmation; it can never remove one.
4. Never attempt captchas, "verify you are human" pages, or login walls: the run ends `blocked`.
5. Shadow mode (D4) is the default: `SYSTEMONE_COMPUTER_ACT=1` enables execution. In shadow mode
   `computer_run` returns one proposed step with probabilities; `computer_act`/`computer_type`
   still require act mode.
6. Every run writes `$GRAY_HOME/systemone/runs/<ts>/` (observations, requests, answers,
   actions, outcome; sensitive values never recorded). `/s1 computer replay <run>` re-feeds the
   recorded observations and answers through `decide` + policy and diffs the chosen actions —
   offline, no device, no backend.
7. Hosted backend (D3): observations — page text, element names/labels, non-sensitive values —
   leave the machine. Values of sensitive fields are never included; their labels are.

## 7. OSS reuse map

| Source | License | What we take |
|---|---|---|
| [chy4pro/jev-for-chrome](https://github.com/chy4pro/jev-for-chrome) | MIT | Extension skeleton, observer, executor, badges, trace, DONE/BLOCKED cross-checks |
| [browser-use/jev-ultrafast](https://github.com/browser-use/jev-ultrafast) | MIT | Action space, question wording, speculative target heads, freshness/occlusion/wait rules |
| [awlevin/typesafe-computer-use](https://github.com/awlevin/typesafe-computer-use) | MIT | Desktop step design, code-computed facts, stop rules, hand-back contract, run folder/replay |
| [odilia-app/atspi](https://github.com/odilia-app/atspi) | Apache-2.0 | AT-SPI client |
| [robertknight/ocrs](https://github.com/robertknight/ocrs) | verify | OCR fallback |
| [ollaya-dev/ollaya](https://github.com/ollaya-dev/ollaya) | Apache-2.0 | Local backend (optional, via `SYSTEMONE_BASE_URL`) |

Forked files keep their copyright headers; `THIRD_PARTY_NOTICES.md` gets one section per source.

## 8. Deferred: general host hooks (D1)

Added only with the first feature that needs them: `turn/input` (prompt/context gets the user
message), per-turn context attached to the user message instead of the cached system prompt,
`tool/after` (rewrite a result), compaction keep/drop. None is required for computer use.

## 9. Phases and acceptance

| Phase | Deliverable | Acceptance (all with a mock `/v1/systemone` unless noted) |
|---|---|---|
| P0 ✅ | client, `judge`, `semantic_find`, `/s1` | built, smoke-tested |
| P1 | tokio sidecar + decide + policy + trace/replay + `cdp` device + 4 tools, shadow/act | Scripted mock answers; local fixture pages (form, dropdown, search + results, covered element, password field, icon-only submit) in headless Chromium. Shadow: one proposal, zero executed actions (asserted from the trace). Act: fixture tasks complete. Password field → `refused`. Submit/icon-only submit → ask path; with the ask stubbed to `result.error`, outcome is `needs_confirm_denied`. `computer_observe` answers while a `computer_run` is in flight. 300 s cap and the 45 s ask floor honored with a slow mock. Replay of a recorded run reproduces the same actions. |
| P2 | extension fork + native host + pairing | Unpacked load in Chromium with a fixed ID; pair; same fixture tasks via `device=chrome`; Stop aborts mid-run; debugger detach → `unavailable`; unpaired client rejected; sidecar restart → native host reconnects. |
| P3a | `desktop` device, AT-SPI only | AT-SPI observation of a GTK app and Firefox; click + type round trip in act mode; corner kill switch. |
| P3b | OCR fallback (only if P3a shows thin trees on apps we need) | Canvas/Electron window observed via OCR; click lands on the OCR'd word. |
| P4 | live Jev + real session | Needs `TYPESAFE_API_KEY`. Re-run fixtures + 3 real sites (Wikipedia search, HN comments, a flight search stopping before booking). Real-REPL checklist: grant `host.ask`, confirm one risky submit, decline one. |

Verification constraints: no `cargo test` in X (global rule) — unit tests are written and run in a
TTY/CI; in-session checks are `cargo check/build/clippy` + headless smoke runs. The real `host/ask`
path needs the interactive REPL and a human, so it is P4's checklist, not an automated P1 check.

## 10. Risks

- **Real-profile blast radius (D8).** The extension acts as the user on logged-in sites. Mitigated
  by §6 and the bound tab; residual risk is accepted by D8 and must be stated at install.
- **Accuracy.** Jev for Chrome reports 9–14 of 17 across rounds; DONE needs the agent's
  verification. Thresholds get tuned from shadow-mode traces, not cookbook defaults.
- **AT-SPI coverage.** Electron/Chrome expose trees only with accessibility enabled
  (`--force-renderer-accessibility`); OCR fallback covers the rest, slower.
- **Element cap.** Pages with > 255 interactive elements are truncated by visibility; hierarchical
  choice (jev-tree) is a later fix.
- **Async runtime.** CDP websockets, zbus, the socket, and concurrent tool calls need one; the
  sidecar moves to tokio in P1 (still one process).

### Failure modes

| Failure | Behavior |
|---|---|
| Extension not installed / unpaired | `unavailable` + `/s1 computer install-chrome` / `pair` hint |
| Chrome closed or tab closed mid-run | `unavailable`, trace kept |
| Debugger detached (infobar dismissed, DevTools opened) | `unavailable`, run cancelled, no retry |
| Restricted page (`chrome://`, Web Store) | `unavailable` before the first step |
| Socket stale after sidecar respawn | native host reconnects with backoff; extension shows "gray not running" meanwhile |
| Backend down / slow | one retry, then `error`; slow responses count against the 300 s cap |
| `host.ask` not granted / headless | risky steps → `needs_confirm_denied` |
| AT-SPI bus missing | desktop `unavailable` with the at-spi2-core hint |
