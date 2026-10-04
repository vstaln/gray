# Changelog

## [Unreleased]

### Added
- **`--bare` (or `GRAY_BARE=1`) runs gray with nothing but itself.** The built-in system prompt
  and the `bash` tool; `~/.gray/AGENTS.md` (neither read nor created), memory, skills, project
  AGENTS.md/CLAUDE.md, plugins (`gray.yml`, installed, pi), cache warming and the update check are
  all skipped. Model, provider and context settings still apply. For benchmarks and reproducible
  runs, where whatever happens to be installed on the machine must not leak into the result.
- **`--json` streams the answer as it is written.** With `GRAY_STREAM_TEXT=1`, `gray -p --json`
  emits the assistant's prose as `progress` rows with `phase: "text"`: one numbered `segment` per
  run of prose between tool calls, complete lines as an append-only `delta`, the unfinished line
  as a provisional `tail` (the word still being typed held back, so a secret is never shown half
  written), and a `done` row that closes the segment. Both go through the same redaction as the
  final answer. A chat surface (the Discord plugin) edits its reply in place instead of posting it
  at the end. Opt-in, so existing consumers keep the rows they get today.
- **`--json` tool rows name the tool properly.** `tool_started`, `tool_ran` and `tool_finished`
  rows carry a `label`: the plugin's manifest label when it declares one, else the wire name
  humanized the way the TUI headers already show it (`discord_send` reads `Discord Send`; a single
  token such as `bash` stays as it is). Surfaces show that instead of the raw id.

- **`--json` can carry plugin questions.** With `GRAY_JSON_ASK=1`, a `host/ask` from a questions
  plugin goes out as a `progress` row (`phase: "ask"`, `ask_id`, `questions`) and its answer comes
  back as one stdin line, `{"ask_id":N,"answers":{"<id>":["<label>"]}}`. Whatever drives gray (a
  chat bridge, a harness) shows the question in its own UI. gray itself still has no question
  tool: nothing asks unless a questions plugin is installed. EOF or no answer within the usual 300s
  resolves empty, as before.

### Fixed
- **Empty sessions stay out of the resume list.** A session that never sent a message (`(no message yet)`, usually the `just now` row at the top) no longer shows in the `/resume` picker, headless lists, or `--last`. Explicit `resume <id>` still loads one.
- **The `discord_send` preview reads like the channel does.** The transcript echoed raw markdown (`**bold**` with literal asterisks) while Discord renders it. The row now strips paired markers (`**`, `__`, `` ` ``, `~~`); unpaired `*`/`_` and spoiler bars stay untouched.

- **No more 403s that only a restart cleared.** `/connect` writes the picked provider's base URL
  and key into the live session before the model step saves anything, so dismissing it left that
  pick in memory beside the old model. Every later agent rebuild (`/model`, `/thinking`, `/new`, a
  resume) sent it, and the provider answered `403 Authentication failed` until a restart re-read
  `~/.gray/config.json`. A dismissed or failed `/connect` now restores the session exactly. And an
  auth failure re-reads the key saved for the same endpoint: when another window's `/connect` or
  `gray login` changed it, the session adopts it and says so, and Enter retries with it.
- **Cache warming runs with a thinking effort on.** Any effort other than `off` turned the
  long-tool cache warmer off, so a 5-minute tool at `xhigh` re-billed the whole prompt. As in
  pi's `isReplayable`, only a Claude thinking budget blocks the 1-token replay now (the budget is
  sized from the output cap and Anthropic keys its cache on it); reasoning efforts on every other
  model replay unchanged. A model with no dollar prices (a subscription or free tier) is warmed
  once its prompt reaches 20k tokens and the provider has reported cache activity, and so is a
  cheap model whose saving is under $0.05. On api.openai.com the replay's cap goes out as
  `max_completion_tokens`, which its reasoning models require, and a chat replay at an effort
  leaves out a thinking budget that cannot fit under the cap.
- **The input box follows the transcript, the way codex's does.** The band (status dock, input
  box, footer) was pinned to the screen's last rows, so a short session (a fresh start, a
  dismissed `/resume`) showed the banner at the top, the input box at the bottom and a
  screenful of dead rows between them. The band is now codex's inline viewport
  (`composer::terminal`, ported from `custom_terminal.rs`, `insert_history.rs` and
  `tui/scrollback.rs`): it sits directly under the last transcript row and only reaches the
  bottom once the transcript fills the screen. A height change keeps its top (the slash popup
  opens downward and the banner never moves). History goes in through DEC scroll regions:
  while there is room the band is shifted down with reverse index, then line feeds at the
  bottom of a region ending just above it push the oldest rows into scrollback, so an insert
  never erases or repaints the band. Windows Terminal, whose partial regions drop rows, and a
  one-row history region take codex's whole-screen path instead.
- **No doubled `❯` after a modal.** Closing `/resume` (or any alternate-screen picker) rebuilt
  the terminal from a cursor probe. The probe landed on the old prompt row, the band overflowed
  the screen and scrolled it, and the old input box stayed painted above the new one. The band
  is now cleared and repainted where it already is, as codex restores its saved viewport, and
  a resize reflow starts it on row 0 instead of probing.
- **No stray blank rows above the dock when the band shrinks with nothing to print.** The band
  rides the screen bottom, so any shrink (a multi-line follow-up queued mid-turn collapsing the
  input box, the slash popup closing) slid it down and opened the rows it gave up between the
  transcript and `⬡ Thinking…`/`⬡ Working…`. `settle_band` only filled them when a row was
  inserted, so a paused thinking run or a slow first token showed three or four blank rows
  where one belongs. The band now keeps its top on every shrink: the rows it gave up sit
  below its footer and the next inserts spend them.
- **Live thinking rows never open with a space.** A thinking buffer that exactly filled a row
  was flushed whole, so the next chunk's leading space started the following row
  (` changed and whether…`). Rows now break after the boundary space, and a continuation's
  leading spaces are trimmed when painted (the stored text is unchanged, so reflow is too).
- **One margin rule for the whole transcript.** Blank rows came from seven places at once
  (51 hand-placed `ensure_gap` calls, the markdown renderer's paragraph blanks, the thinking
  stream's `\n\n`, card padding, the dock's seam, rows the band vacated, the turn footer), and
  wherever two met the margin doubled: two blank rows above `⬡ Thinking…` after a paragraph
  break, two under a tool card at 91-94 columns. Now a block boundary only *owes* a gap, and the
  gap is paid as the prefix of the next block, never left as a trailing blank. Blank rows at a
  block's edges become boundaries; blank rows inside one (a code block's) are content and stay.
  The transcript therefore always ends in content and the band's seam row is the one gap to
  the dock or the input box, idle or mid-turn. Live streaming, resize reflow, session replay
  and the setup screen's backdrop all lay blocks out through the same funnel
  (`transcript::margins::admit`), and debug builds assert no write ever stacks two blank rows.
  The band also settles its height before every insert (`draw::settle_band`), so rows it gives
  up are filled rather than stranded; the shrink hint and the seam-overwrite slack are gone.
- **A turn-ending error reads as an error.** A rate limit, auth failure or server error that
  outlived its retries was streamed as plain prose, in the same colour as the answer. Its
  headline (`✗ Rate limited (retryable): …`) is now in the theme's error colour and the hint
  under it is muted, as codex renders errors. The retries before it are unchanged:
  `⬡ Reconnecting…` in the status dock and one dim `└` row per distinct cause.
- **An interrupt says so again.** Since the "(interrupted — press Enter to continue)" line was
  dropped, an interrupted turn left nothing in the transcript, and the only hint was the
  "Please continue…" ghost in an empty composer, which typing hides. An interrupt now leaves a
  dim `■ Conversation interrupted` line in history, as codex does; the Enter hint stays in the
  idle ghost, the one place where it is always true. The ghost's gate was also armed with
  `try_lock` and silently skipped whenever a painter held the TUI; it now always arms.
- **No doubled gap under the turn footer.** `end_turn` cleared the status dock but committed the
  `Worked for` / `Thought for` footer before the band shrank, so the footer scrolled in above
  the still-docked band and the shrink left a stray blank row between its trailing gap and the
  input box (most visible after an interrupt). The band now drops the dock first, and the
  footer and gap land in the rows it gave up.
- **Margins follow codex's layout.** A card's painted padding row was counted as the gap
  between blocks, so prose and thinking sat flush against every tool and prompt card. The
  padding belongs to the card again, and one unpainted row separates every card from its
  neighbours, the way codex keeps a user cell's padding apart from the row `history_cell`
  inserts between cells. Prose, thinking, the `●` of a tool card and the `❯` of a prompt card
  all start in one column (a 1-column gutter), and wrapped rows keep one column clear at the
  right edge. Live streaming, resize reflow and session replay share one wrap budget
  (`prose_width`), so the three agree.

- **A card's margin is visible again.** The gap that separates a card from the paragraph
  around it was an unpainted blank row, and an unpainted row against the composer surface
  is invisible: the margin existed in the transcript and not on screen. Card margins are
  painted again - one row above, one below, edge to edge with real cells in the card
  background - and `transcript_row_is_blank` now judges a row by its glyphs rather than its
  background, so a painted margin counts as the blank row it is instead of asking for a
  second one. Exactly one margin per card side, and a card after a card still shows one.

- **A commit never strands a blank row above the band.** `Tui::atomic` batches its inserts
  and draws (which resizes the band) once at the end, so `insert_before` had already scrolled
  the screen for the band's OLD, taller height by the time it shrank. The rows it vacated
  stayed blank between the transcript and the band: a thinking commit that ended in a blank
  row stacked on the dock seam and left a second blank above it, and a tool result committed
  its card against a still-taller band and stranded the difference above the dock. An insert
  now overwrites the band rows that are about to disappear (`top_slack`, refreshed each frame
  from the dock seam; `shrink_hint`, declared by `remove_live_tool`), instead of pushing them
  down. Both are advisory — a wrong guess is corrected by the next `draw`.

- **"Press Enter to continue" stops outliving the turn it was about.** The interrupted
  line was streamed into the transcript, which made it permanent history: it kept
  sitting under the next turn's status pill, long after the resume it offered was moot.
  It is a property of an idle composer, so it now lives only in the idle ghost, which
  paints while the box is genuinely empty and no turn is running. The error path's
  permanent copy of the same line went with it.

### Changed
- **The ChatGPT/Codex subscription plugin moved out of the gray repo** into
  [`vstaln/gray-codex-sub`](https://github.com/vstaln/gray-codex-sub), where it ships as the
  standalone `gray-codex-sub` sidecar. Install it, then `/connect` and sign in again —
  credentials stored under the old `codex-auth` plugin name are not reused.
- **CI waits on less.** `windows-runtime` gated every run at 9.5 minutes on a PR and 14.5 on
  main. It no longer builds a release binary: the installer tests run against the debug
  `gray.exe` the test build already made, and the downloadable preview ZIP is built by its own
  `windows-preview` job beside it. Tests run under `cargo nextest` (`.config/nextest.toml`,
  profile `ci`): every test in its own process, all binaries at once, each failure reported by
  name, so the sleep-bound shell lifecycle suites overlap instead of queueing binary by binary.
  That made the targeted Windows and macOS test steps and `cargo check --all-targets` pure
  repetition, so they are gone. The tool-call latency bench is `#[ignore]`d and runs in
  `perf-floor`. Docs-only changes skip the Rust jobs, a newer push cancels a PR's run in flight,
  only main writes the Rust cache, CI builds without dev debuginfo, and ripgrep installs without
  a package index refresh unless it needs one.
- **Plugins have one registry and one installer.** `commands.json` is gone: a `cli_argv` field on
  each `plugins/lock.json` entry carries the `gray <name> …` forwarding vector, and a legacy
  `commands.json` is folded in on first use and renamed `commands.json.migrated`. `gray install
  plugin` is removed — `gray plugin install <name|url|path>` is the one install path (index name,
  https tarball, or local executable; `GRAY_PLUGIN_PATH` still overrides). The foreign plugin
  arms (`npm:`, `git:`, `claude:`, pi-gallery `clawhub:`) are gone from `plugin install` — skill
  specs (`clawhub:…`, `github:…`, `url:…`) route to the skill installer as before and land in
  `~/.gray/skills`.
- **Discord setup moved out of core.** The app-setup flow, the pinned-catalog build, and the
  Discord-specific transcript rendering left `gray`; the Discord plugin owns `setup`, `doctor`,
  `register`, `run`, and service install. Install it from
  [gray-discord-plugin](https://github.com/vstaln/gray-discord-plugin)
  (`cargo install --git https://github.com/vstaln/gray-discord-plugin --locked`, so `gray-discord`
  is on PATH), then `gray plugin install discord` and `gray discord setup` replace the old
  `/gateway` wizard. `/gateway` now just lists installed apps and their declared subcommands.
- **The structured input protocol is `gray.input`.** The old `gray.discord.input` identifier
  still validates, so plugins that already emit it keep working.

## [0.1.10] - 2026-10-01

### Added
- **A native Anthropic provider.** A key on `api.anthropic.com` now speaks the Messages API
  instead of Anthropic's OpenAI-compatible endpoint, which has no prompt caching: every request
  re-billed the whole prompt. Requests carry `cache_control` breakpoints on the system prompt, the
  last tool and the last message; thinking blocks go back with their signatures; an overloaded 529
  or a rate limit retries before the stream opens.
- **Prompt-cache warming during long tool runs** (pi parity). On the native Anthropic provider,
  a tool that runs past the 5-minute cache lifetime no longer costs a full prompt re-write on the
  next request: at 90% of the lifetime the round's request is replayed with a one-token output
  cap, while the expected saving is at least $0.05, for up to an hour. Refresh usage is billed with
  the turn and never enters the conversation. Thinking budgets cannot be replayed safely, so it
  applies with thinking off; `GRAY_NO_CACHE_WARM=1` turns it off.
- Command output is compressed by what the command was, and truncation is no
  longer a dead end. `cargo`, `npm`/`pnpm`/`yarn`, `pip`/`uv` and `pytest`
  output loses its progress chatter (a 300-crate `cargo build` is 30 KiB of
  `Compiling …` lines and one `error[E0308]`), a generic rule collapses runs
  of identical lines for everything else, and the result says so before the
  body. The raw log stays on disk exactly where it was, so the squeeze costs
  the model nothing it cannot grep back. `GRAY_NO_SQUEEZE=1` turns it off.
- `gray spill` reads a tool result back. A result over the inline budget now
  keeps its preview *and* stores the full original under a content hash, with
  a `[spilled …]` footer naming the handle — so the middle that truncation used
  to amputate is recoverable: `gray spill grep <handle> <pattern>`,
  `gray spill head|tail <handle>`. It is a subcommand rather than a tool on
  purpose: the model has a shell, and a tool entry is schema every turn pays
  for. Handles are checked character by character, so one can never name a
  path outside the store; an evicted or unknown handle is an error that says
  what to do, never an empty result. `GRAY_NO_SPILL=1` turns the store off.
- Compression is metered, so the saving is a number instead of a claim.
  `gray spill stats` reports what was produced against what reached the model,
  broken down by rule and sorted by what each rule actually saved; `/usage`
  grows the same counterfactual as one line.
- A performance floor in CI (`scripts/perf-floor.sh`). Unique dependency
  count, crates with a C/C++ build step, workspace members and stripped binary
  size are down-only ceilings committed in `scripts/perf-baseline.json`. The
  numbers the README sells had no gate; raising one is a reviewable commit.
  Startup wall time is measured and printed but never enforced, because a time
  budget on a shared runner fails on load rather than on regressions.
- **`gray doctor` answers "is my setup OK?" in one command.** One pass/fail
  line each for the gray home (exists, writable), the provider credential
  (never its value), the selected model, the context window *and where that
  number came from*, the shell (Git Bash on Windows), the exec prefix, the
  gateway, and the installed plugins. `gray doctor --online` also does a
  `GET /models` against the configured provider — no tokens, but it leaves the
  machine, so it is opt-in. Exits non-zero when a check fails, so a setup
  script or CI can gate on it. Diagnose only: it never writes a config, starts
  the gateway, or installs anything.
- **Typing during a turn steers it.** Text typed while a turn is running used
  to wait for that turn to finish; it now joins the turn at the boundary before
  the model's next request, so a correction lands while the work is still in
  flight ("actually, skip the last step"). Steering only appends -- the turn
  keeps everything it has already done, and cancelling is still Ctrl-C/Esc. An
  input with attachments keeps them and still runs as its own turn afterwards,
  and whatever is left in the queue is sent once, after the turn, as before.
- **`/undo` and `/retry` step back inside a session.** `/undo` drops the last
  exchange — the last thing you said and everything the model said after it —
  from both the live context and the saved session; `/retry` is the same
  rewind with your message sent again. The rewind rewinds the *conversation*
  only: files the model wrote are untouched, and the pre-undo transcript is
  kept in `~/.gray/sessions/archive/`, so it is recoverable by hand. The cut
  always lands on a user turn, so no tool call is ever separated from its
  result.

- **`exec_prefix` runs the model's shell commands somewhere else.** One saved
  setting (`~/.gray/config.json`, or `GRAY_EXEC_PREFIX`) names a program that
  ends in a shell reading its script from stdin, so `docker exec -i dev sh -s`
  and `ssh box sh -s` cover both a local container and a remote box:
  ```json
  { "exec_prefix": "docker exec -i dev sh -s" }
  ```
  The command crosses as **text**, not as an argv the far side re-splits, so
  `$VAR`, globs, pipes, heredocs and quoting arrive exactly as written — the
  `ssh box sh -c 'ls'` trap cannot happen. Gray's non-interactive environment
  (`GIT_TERMINAL_PROMPT=0`, `GRAY_SESSION_ID`, `GRAY_CWD_REPORT`) is exported
  across the boundary too, because neither `ssh` nor `docker exec` forwards the
  client's environment. `current_dir` still applies to the local client, so a
  remote command starts in that account's login directory.

### Fixed
- **Windows installs work from one line.** `irm https://gray.alignment.id/install.ps1 | iex`
  failed three ways in 0.1.9: piped to `iex` there is no `$PSScriptRoot`, so `install.ps1`
  refused to run without a sibling `install-native.ps1`; that file was never published to the
  site anyway; and the native installer fetched a per-archive `.sha256` file the release never
  publishes, so every download-mode install died on a 404. `install.ps1` now fetches
  `install-native.ps1` from the same site over HTTPS and runs it as a script block (no
  execution-policy change), the release publishes it beside `install.ps1`, and the archive is
  verified against the channel's `SHA256SUMS-<channel>` file -- exactly one well-formed line for
  that zip, or nothing is trusted. The offline `-ArchivePath`/`-Sha256` route is unchanged.
- **The Windows installer defaults to stable**, like `install.sh`. `-Channel beta` still
  selects the per-commit build.
- **`-Wsl` no longer closes your PowerShell window.** Its two `exit` calls ran inside the
  user's session under `iex`; they now throw or return.
- The installers are ASCII-only: Windows PowerShell 5.1 decodes an uncharset `text/plain`
  response as Latin-1.
- **A provider error reads as a sentence, once.** A 429 printed its raw JSON envelope
  (`{"error": {"message": …, "type": …, "param": …}}`) on every retry row and again in the final
  error. HTTP errors now show the provider's own `error.message` (the full body still drives the
  classification), and a retry burst repeating the same cause is one row, not one per attempt.
- **Tool cards drop the log path.** Every bash card showed `log ~/.gray/shell/<session>/bash-<hash>.log`;
  the path stays in what the model reads (it pages the log back from it) but is gone from the
  card. A signal exit no longer says it twice: `exit 143 (SIGTERM) (terminated)`.
- **Claude caches again behind OpenAI-compatible routers.** The `cache_control` breakpoints were
  removed on 2026-09-23 on the theory that `prompt_cache_key` covers caching; Claude ignores that
  field and caches only at breakpoints, so Claude through OpenRouter re-billed the full prompt on
  every request. The breakpoints are back for Claude models.
- **Old reasoning no longer reappears under the answer.** When the band shrank at turn end, a
  refill reprinted remembered scrollback rows into the gap it left, and once the transcript had
  scrolled past the screen it picked the wrong ones: the first round's thinking showed up again
  below the final answer, above `Thought for`. The refill is gone; a shrink leaves blank rows that
  the next output fills.
- **The REPL composer rides the last rows of the screen from the first frame.** A fresh (or
  cleared) session used to park the input box and the footer right under the welcome banner, with a
  dead band of cleared rows down to the bottom of the screen: the pin only latched once the
  transcript had overflowed the viewport, and an explicit branch unpinned it again on every growth.
  The band is now positioned at `screen height - band height` unconditionally, so a shrink (status
  dock, live cards clearing at a tool result, end of turn), a growth and a resize all keep the
  footer's row the screen's last row and repaint what they vacate. Band budgeting makes the text
  area and the footer un-trimmable and sheds the most transient band first, so a busy screen loses
  the status dock before the transcript.
- **The input box keeps its top margin row.** 0.1.9 dropped the blank row the box owns above its
  `❯` row and leaned on the transcript's own trailing gap, so the prompt sat flush against whatever
  was above it. The pad row is back (and the caret follows the prompt row, not the pad), with
  `MIN_VIEWPORT_H` back at 4.
- **"Please continue…" no longer shows while the model is streaming.** The bare-Enter resume flag
  is armed when the REPL loop blocks on input, i.e. before the turn a bare Enter would continue is
  submitted, so mid-turn it was stale and the box kept painting the ghost over a live turn. It is
  dropped when a turn starts, and the hint is gated on the composer being idle.
- **A cron job added from the REPL comes back into that chat.** "Message me in a minute" fired,
  wrote its output under `cron/output/` and showed nothing in the conversation; the model polled
  with `sleep` to find out. A job added from inside a session (the bash tool's `GRAY_SESSION_ID`)
  now records that session as its origin, and whichever ticker fires it drops the result into
  `cron/inbox/<session>`. The REPL showing the session paints a `⏰ cron` card and, once idle,
  starts a turn with the result as a `[Cron delivery: <name>]` message. A background bash job that
  finishes while the REPL is idle starts a turn the same way, and queued `host/say` lines paint
  without waiting for the next keypress.
- **A parallel tool batch stops sitting in "Preparing tool" while it runs.** `tool_call_end` (the
  "args complete, executing" signal) fired for each member only after the slowest one finished; it
  now fires for every member before the batch starts.
- **A full-width numbered or diff row in a tool card stays on one row.** The card re-wrapped body
  rows two columns narrower than `tool_fmt` had already wrapped them, orphaning each full row's last
  word onto a continuation row.
- **A newline could smuggle a secret past the redactor.** The tokenizer splits on `' '` only, so
  `'\n'` glued neighbouring lines into one token: a secret below the first line of a shell chunk was
  written to the durable log and sent to the provider in the clear, and a secret that *did* fire armed
  the next glued token and deleted every line after it. Redaction is per line now, for every sink at
  once (bash output, `edit`, `write`, `grep`).
- **A secret split across a pipe read no longer leaks its tail.** The shell pump forwarded whatever
  each `read()` returned, so a token straddling an 8 KiB boundary was redacted in pieces. Readers
  now hold an unterminated line until its terminator arrives (4 KiB cap, flushed at EOF), per pipe,
  with the liveness stamp still taken on the raw read.
- **Concurrent credential writes can no longer erase each other.** The `auth.json` flock covered only
  the final write, so two gray processes updating the store (REPL plus cron fire, or two terminals)
  overwrote each other and the loser's credential was gone — a lost OAuth refresh meant a re-login.
  `/key` and provider removal took no lock at all on the same file; both now take it.
- **The plugin setup config is written through the repo's one private atomic writer.** The copy in
  `write_config` created the file at umask mode and chmod'ed it afterwards, and its fixed `config.tmp`
  name could be written through by a planted file or symlink.
- **The pinned self-update keeps no scratch directory behind.** `gray-installer-<pid>` under the
  shared temp dir (pre-creatable by another local user, removed only on success) is a private
  `TempDir` now, dropped on every exit including a checksum mismatch.

- **One blank row between blocks, and the card that owns none of it.** A tool card and a
  prompt card each shipped their own leading and trailing margin row *and* asked the
  transcript for a separating gap, so a card sitting between two paragraphs was fenced by
  two blank rows on each side (and a card after a card by four). Codex's rule is one blank
  row between blocks, contributed by the transcript alone: a block carries no outer
  margin of its own. The card formatters now emit their content only, and `ensure_gap` is
  the single owner of the separation, so a paragraph, a card and the next paragraph are
  always exactly one row apart.

### Changed
- CI and the release builds pass `--locked` on every platform, so a dependency edit without a lock
  update cannot ship from `main`.
- A bash result carries 12 KiB instead of 48 KiB (6 KiB head ++ 6 KiB tail),
  and what rides the wire is squeezed: color escapes are gone and a run of
  three or more identical lines collapses to one line plus a count. Every
  later request of the turn re-sends the whole history, so one big dump used
  to be re-billed for the rest of the session. Nothing is lost — the full log
  is on disk and the `Read more` hint names the exact byte window, now with
  4 KiB pages that fit the inline budget in one piece.
- Old tool output ages out of the context. A `ToolResult` of 8 KiB or more
  rides a request as a one-line citation stub once it is ten rounds old
  (arXiv:2607.25066 — masking costs about half an LLM summary and keeps the
  output addressable), and the mask moves in batches so the provider prefix
  cache stays warm between them. The stub names the tool-call id and this
  session's transcript file; the transcript itself keeps every byte. After an
  idle gap past the prompt-cache TTL the whole mask is applied at once instead
  — the next request re-bills the prefix either way, so the shorter prompt is
  free and is what gets cached from there on.
- `cat`/`sed -n`/`head`/`tail` of a file dedup like the `read` tool. The
  default profile is bash-only, so the repeated read that costs the tokens was
  the one with no dedup: an exact repeat of one of those reads, on a file whose
  mtime and size are unchanged, answers with a citation stub naming the
  command and the file — once, then the next repeat reads again. The ledger is
  the one `read`/`write`/`edit` already share, so the same file read through
  either surface dedups against the other, and `/new` and compaction keep
  their existing lifecycle. `GRAY_READ_DEDUP=0` still turns it off.
- The memory prompt and snapshot shrink. The keep/delete rubric moved out of
  the per-turn policy into `gray memory audit`, which already printed it, and
  the injected snapshot of each scope is capped at 4 KiB: newest entries
  first, with a line saying how many older ones were left out. ~700 policy
  tokens became ~500, and a memory that grew without bound can no longer grow
  the system prompt with it.

## [0.1.9] - 2026-09-30

### Fixed
- **The REPL composer stays on the last rows of the screen.** Once the transcript overflows the
  viewport, a latched `bottom_anchored` keeps the input box and the footer pinned to the screen's
  last rows instead of parking them above cleared rows; a shrink (status dock, live cards clearing at
  a tool result, end of turn) slides the viewport down and repaints what it vacated with the surface
  colour. Band budgeting makes the text area and the footer un-trimmable and sheds the most transient
  band first, so a busy screen loses the status dock before the transcript.

## [0.1.8] - 2026-09-30

### Fixed

- Heredocs survive the cwd-report suffix. `bash` appended
  `; __gray_rc=$?; printf ... ` to the command text, so a command whose last
  line was a heredoc terminator read `EOF; __gray_rc=$?` and the terminator
  never matched: the whole suffix landed inside the heredoc body — a shell file
  written with a garbage trailer, or a `SyntaxError` for an interpreter
  heredoc, silently (`rc=0`). Each piece of the suffix now sits on its own
  line, which also stops a trailing `#` comment from eating it and makes
  `cmd &` legal. 33 of 47 DeepSWE runs in the 2026-09-29 retro reported this;
  it cost each a wasted turn at best.

- The `windows-runtime` CI gate stops hanging on the search-index bench.
  Every `ci` run since the search-as-command merge (#145) died at
  `index_vs_spawn_tax` — "running for over 60 seconds", then silence until
  the job's 60-minute budget was spent. The hang had no reachable timeout:
  the bench's deadline assert lived inside its own poll loop, but the call it
  polled (`SearchPool::warm_picker`) never returned — it took the pool's
  `resident` mutex with a blocking `lock()`, and index construction held that
  same mutex across everything fff does, which on Windows stalled inside
  unbounded dependency waits (LMDB writer lock, git status, watcher init).
  The probe is now non-blocking (`try_lock` with a ~50ms budget, then "not
  warm" → fd/rg answers), construction runs with no pool lock held and is
  deduped by root, and every bench phase carries a hard timeout that fails
  naming the phase. A wedged build now costs one search its fallback lane —
  and costs CI a two-minute failure with a name on it — instead of a 60-minute
  silent hang. `windows-focused` takes a `test-path` input, so a native
  single-test iteration no longer means editing the workflow.

### Added
- `gray cron add "<schedule>" "<text>" --reminder` stores the text and delivers
  it verbatim at fire time. A reminder runs no agent turn, no pre-script, no
  skills and no tools, and its name (when `--name` is omitted) is a slug of the
  exact text — typos included, never rewritten. `--reminder` with `--script` or
  `--skills` is rejected. `cron_delivery` JSON lines now carry `kind`, `status`,
  `elapsed_ms` and `final_text`; `job_id` and `path` remain routing/log fields.
- A cron delivery is the final assistant message, not a transcript. The
  `[tool:…]` / `[result:…]` stream, the `Cronjob Response:` frame, the
  `(job_id:)` line, the dashes, the stop/manage footer and the output-file path
  are gone from the chat text, and the origin-session mirror no longer carries a
  tool log into the conversation. The full transcript still lands in
  `cron/output/<id>/<ts>.md` at mode 0600.
- A fire that failed before producing output now reaches the chat as a red
  `failed` delivery instead of silence.
- Cron transcripts are redacted before they are written to disk or shown:
  exact values from `<home>/auth.json` plus `gray_core::redaction`'s token
  shapes. Paths stay verbatim in a secret-free transcript. A length cap that cut
  an `<untrusted-output>` block open now closes it, so the transcript always
  carries balanced tags.

## [0.1.7] - 2026-09-29

### Added

- The composer keeps its place, and its footer stops flickering. A latched
  viewport floor (`latched_viewport_floor`) grows the frame by exactly what
  the pre-computed dock estimate short-cuts mid-stream, and the footer gauge
  paints on the frame's last row (`footer_paint_row`) instead of being pushed
  past the bottom and skipped for that frame — `Ω 223.8k/300k` and the pad band
  under it now ride out a mid-turn text wrap. Shrinking the viewport keeps its
  top edge fixed, so no blank scrollback is painted behind the box.

- Bare Enter continues. While the last turn is resumable, the empty input box
  shows a dim `Please continue…` ghost hint and submitting empty text continues
  the conversation; a box with text (or an attachment) still submits it.

- Streaming rows hold orphan punctuation. A lone `.`/`,`/`?` arriving as its own
  delta is held until the next chunk says whether it belongs to it, then
  released glued to what follows — no more a frozen lone-`.` transcript row at
  an interrupt.

### Fixed

- `~/.gray/provider_models.json` no longer accepts loopback base URLs. A local
  server's port changes on every start (and a unit test's is random) and
  nothing evicted old keys, so every one was a permanent dead entry — 43 had
  piled up. Remote providers still persist, so `/model` still paints its cached
  list instantly.

- The provider model-list fetch no longer rides the ambient Tokio runtime: a
  background refresh that outlives REPL shutdown used to kill its thread with
  `Tokio 1.x context ... being shutdown`. It runs in its own short-lived
  current-thread runtime.

- Binary detection in the read guard no longer magic-sniffs the first bytes —
  the NUL check decides — and a regular-file-to-FIFO swap between the guard and
  the open is now caught by an `O_NONBLOCK` open instead of hanging the turn.

### Changed

- Self-update hashing uses the `sha2` crate that is already a dependency
  instead of shelling out to `sha256sum`/`shasum` and writing the tarball to a
  temp file.

- gray-markdown drops the incremental open-block cache (its measured cost was
  maintenance, not reads) and the LaTeX-to-unicode stack — raw TeX shows as
  typed. Unicode repair uses a minimal Latin accent table instead of the
  `unicode-normalization` crate, so the dep count holds.

- The shell lane is one type carrying the live child, its process-group guard,
  the output pump and the liveness clock, instead of four parallel ones.

### Chore

- The internal working journal (superpowers plans, study notes, session
  records) is untracked from the public repo; reference and product docs stay
  tracked.

## [0.1.6] - 2026-09-28

### Added

- `/update` and `/restart` in the REPL, the two halves of landing a
  self-update. `/update` checks the channel, asks before installing, and says
  what to do next; `/restart` puts the running gray on the binary that is on
  disk — the gateway daemon first (it keeps running the build it started
  with), then this session re-exec'd into the new one, resuming the
  conversation with `resume --last`. Neither codex nor hermes has either: codex
  shows an update popup at startup and hands you a `brew upgrade` line, and
  hermes-rs's `restart` tears down LSP clients. A CLI that installs its own
  updates owes you a way to land on them.

  The gateway half distinguishes who owns the process. Installed under a
  supervisor, the supervisor restarts it. Running with nobody supervising it
  — a hand-launched `gray gateway run` — it is stopped *and* started again on
  the new build, because stopping without the relaunch would take a working
  gateway down and call it a restart.

- Foreign plugin packages work with zero per-plugin code: `gray plugin
  install <git-url>` now also takes a package's `commands/*.md` and
  `.opencode/command/*.md` prompt files, any installed package's `AGENTS.md`
  is served into every turn, and its command files become slash commands
  that run as prompts (ponytail's ruleset + six commands included). An
  optional package `gray.json` declares a state file so `/x mode`-style
  switches persist. Package code is still never executed: hooks, MCP
  servers, and lifecycle scripts stay out of scope.

### Changed

- The gray mark animates. `assets/logo-animated.svg` grows out of its own
  centre: the gem blooms first, then each of the 25 facet lines draws
  outward from the vertex nearest the centre, staggered by distance so the
  core lands before the outline. The source path — one path, nonzero fill
  rule, facet lines as windings that cancel — is reused verbatim as the mask
  the lines grow inside, so a line can only ever paint pixels the finished
  logo already has as line: the last frame is the logo, verified pixel for
  pixel. Pure CSS with no JS, it runs inside `<img>`, and it degrades to the
  finished mark when animation is unsupported or `prefers-reduced-motion` is
  set. `scripts/animate_logo.py` regenerates it (`--lines` for the
  transparent variant on dark surfaces, `--frames N dir` for previews);
  `assets/logo-grow.mp4` is the 1.9s preview.
- The startup banner is the gray ASCII logo again. The graychan art is
  `/hehe` only, and `/hehe` is a toggle: press it again to drop the art and
  get the logo back.

- The `/model` picker no longer waits on the network every open: the last
  fetched provider list is cached on disk and paints instantly, the live
  fetch refreshes it in the background, and a `─ recent ─` divider separates
  your recent models from the full list.

- `bash` takes `wait_ms` on `action:output` and `action:status`, so a single
  call can await a job instead of polling it once per turn. Capped at 600s,
  and every wait comes back with a liveness verdict: the log either grew
  while we waited (still producing) or stayed silent (possibly stuck — the
  agent's cue to inspect or cancel it).

### Fixed

- A command that goes silent no longer blocks its tool call forever. The
  blocking `bash` lane waited on `child.wait()` with no bound and reported
  nothing while it waited, so an external process that wedged at shutdown —
  a Playwright `browser.close()` race, reproduced in
  `docs/shell-hang-postmortem-2026-09-28.md` — held one call for 9m44s until
  the user cancelled, and the evidence that would have explained it (the
  job's log, finished at spawn time) was invisible until the call returned.
  A command that sets no `timeout` and emits no new output for 600s is now
  handed to a background job and the call returns immediately: `still
  running · job <id> · silent: no new output for 600s · log <path>`, not
  killed, with the agent's cue to inspect, await or cancel it. A chatty
  command is never touched — every output chunk resets the bound — and the
  reverted 30s/120s defaults stay reverted: this reports, it does not kill,
  because a long build and a wedged process have to stay distinguishable.
- The band between the last transcript row and the input box stops showing
  a stripe of the terminal's default background. Plugin-widget rows, the
  queued-follow-up preview and the ask modal were rendered as bare
  `Paragraph`s — transparent — so on a terminal whose default background
  differs from the theme they painted as a foreign block through the middle
  of the composer whenever a turn streamed (worst with a widget installed:
  its rows appear only while it has something to say). Those rows now carry
  the composer's own background like the live tool cards and the input box.
- The REPL composer holds its place. The inline viewport only re-anchored
  when its rows overran the bottom of the screen, so every viewport resize
  walked the input box and footer with it: a tool call grew them (status
  dock + live tool card) and the tool result shrank them again, leaving the
  footer parked above dead terminal rows while the turn kept streaming.
  Once the transcript has filled the screen the composer's fixed rows now
  stay on the screen's last rows, the dock/card rows above them scroll
  instead, and a shrink repaints the rows it vacated with the composer's
  own background. A transcript shorter than the screen still hugs the
  conversation, as before.
- The REPL footer's right segment stops walking across the bar mid-stream.
  Whether the reasoning-effort badge paints depends on a reasoning flag that
  background discovery keeps writing after startup (the provider's `/models`
  at `repl/mod.rs`, then models.dev), so on models whose two answers disagree
  — StepFun's own gateway reports `step-5-preview` as non-reasoning while
  models.dev marks it reasoning — the badge appeared and vanished a frame or
  two into a turn, shifting `Step 5 Preview · xhigh` eight columns. The
  visibility is now snapshotted once at the beginning of each turn, so the
  footer holds its shape while anything streams; the provider's converged
  answer still lands between turns.
- A 503 burst outlasting the provider's own 5-attempt budget no longer kills
  the turn the user is waiting on: the agent loop retries the whole request
  (up to 2 more times, short ramp, nothing streamed yet — a visible delta
  still ends the turn instead of replaying text you already read).
- `401` responses whose body says the model isn't supported (OpenRouter's
  `ModelError` shape) classify as a bad request, not an auth failure — no
  more "check API key" advice for a model problem.
- Empty tool-call padding deltas (gateways that append `{index}`-only
  entries to the final chunk) no longer materialize ghost fragments, so the
  `dropping tool call index N with empty name` warning spam is gone.
- `gray view` inside a compound shell command no longer reads as success with
  nothing attached: the bash tool appends a note that the image was NOT
  attached and how to re-run it bare, and the CLI fallback line says the
  terminal has no image protocol instead of a bare `viewed …`
  (`docs/bug-gray-view-compound-command.md`).
## [0.1.5] - 2026-09-26

### Changed

- The search index is a command, not a lane inside the `find` and `grep`
  tools. 0.1.4 left those two tools answering differently depending on the
  directory, the pattern and a decade of glob semantics — and a missed rule
  there is a wrong answer, not a slow one. `find` and `grep` are `fd`/`rg`
  again, unconditionally, and `gray find PATTERN [PATH]` / `gray grep PATTERN
  [PATH]` own the index: the same answer, served warm, with the fallback lane
  being the very tool the command stands in for. Both are claimed by bash like
  `gray view`, so a model reaches them without `gray` on its PATH.

  The policy is cost-first, which the tool placement had hidden: the index
  lives in process memory, so a fresh process holding none paid a full scan —
  1.4s against `fd`'s 20ms on a 20k-file repo, a "speedup" 70x slower than the
  shell it replaced. Now the first search in a process goes to `fd`/`rg` and
  starts the index in the background; every search after that is index-served.
  Cold `gray find` on that repo: 21ms. What the index is actually worth is
  grep on repeat searches (2.9x the tool lane); `find` is a wash.

- `cat` is no longer a second way to see a picture. It returned a
  full-resolution vision block while `gray view` capped at the shared 2000px,
  so the model had two paths with two answers and picked by habit. `gray view`
  is now the only one: `cat <media>` says so in one line instead of streaming
  binary into the context. Same text, same tool, one fewer thing to learn.

### Fixed

- A search cancelled before it starts now answers `cancelled by user`
  instead of racing the spawned `fd`/`rg` child's first line, which could
  return a finished result for a call the caller had already given up on.

## [0.1.4] - 2026-09-26

### Added

- `find` and `grep` answer from a resident file index instead of spawning
  `fd`/`rg` per call. A watcher-backed `fff-search` index is built lazily per
  search root, ranks hits by frecency, and serves the next search out of warm
  memory: measured on this repo, `find` 9.7ms against `fd`'s 17.8ms and `grep`
  17ms against `rg`'s 95ms, with identical result sets. The tool surface does
  not change — anything the index cannot answer exactly (a non-git root,
  `ignoreCase`, a negated or depth-anchored glob, an invalid pattern, a file
  target) falls straight through to the old lanes, and a cancel still stops at
  once instead of waiting out a cold scan. Design and decline rules:
  `docs/fff-search-index.md`.

- `gray view PATH...` shows an image file as an image (downscaled to the 2000px
  cap, the one shared with the `read` tool and pasted attachments). The `view`
  *tool* that 9ae15d3b deleted comes back as a command: bash's one vision path
  now claims `gray view <path>...` before the shell runs, alongside
  `cat <path>` (full resolution) — so an agent checking a rendered chart,
  screenshot or diagram gets an image instead of pixel soup, with no new tool
  in its toolset. Multi-path by design; a leading `~` is expanded, since
  nothing else would do it before the shell runs.

- `gray view movie.mp4` shows a contact sheet of sampled frames, so an agent
  can *see* a recording without shipping the whole file into the context;
  `--frames N` sets how many. Gemini and Gemma models get the native video
  instead of the sheet, and an oversized file says so rather than silently
  downscaling past the cap.

- `/gateway` is the connections picker: every installed app, its own status,
  and Enter on a needs-setup row runs *that app's* setup — the channel picker
  covers DMs, setup is token-first, the configs an app writes land in the
  user's home rather than gray's, and `gray gateway setup <app>` does the same
  headlessly for scripts.

- `gray --json progress` narrates a turn: `tool_started` / `tool_ran` /
  `tool_finished` rows carrying disclosed, bounded, redacted output plus the
  internal call id, and `thinking` phase rows. A front end can render live
  tool activity and a separate persistent tool card without parsing prose.

- Memory carries a turn: a one-sentence profile summary is injected with it,
  the prompt asks the model to show a memory's KEY before leaning on it, and
  the daily ingest is mechanical and bounded — append-only edits, daily caps,
  and no verbatim duplicates under a new key.

- Provider plugins join the plugin protocol at 1.2: host-owned credential
  refs, sidecar RPCs, cache/runtime roles, `/connect` plugin login, and an
  OpenAI dynamic provider profile. Codex/ChatGPT OAuth ships as a first-party
  optional plugin (`plugins/codex-auth`), so `/connect` discovers auth
  providers only when one is actually installed and enabled.

### Fixed

- An interrupted sidecar or provider stream no longer turns a usable partial
  answer into an error. A provider stream that ends without its completion
  marker is finished with a capped, nonfatal interruption notice rather than a
  `CoreError`, because the visible delta is already committed to history and
  replaying it would duplicate text the user read. On Windows a blocked pipe
  write is bounded by a worker task instead of stalling the runtime thread, so
  cleanup can actually terminate the child; child termination after a write
  timeout is bounded, and an already-exited child id no longer wedges cleanup.

- The default prompt points the agent at `gray view`, not `cat`, for images —
  `cat` still returns bytes, but the agent is told which one to reach for.

- Prompt caching drops two pieces of pi-parity over-engineering
  (`cache_control` masking and `x-session-id`), and `/resume` previews the
  latest message of a session rather than its opener.

- A 23-finding source audit landed with its sweep: drive-named archive entries
  refused on every platform, symlink-cycle and atomic-pid-claim guards,
  serialized config read-modify-write, a log-rotation guard that acquires
  before it opens, and the remaining secret-redaction gaps in tool output.
  Dispositions: `docs/audit-fixes-2026-09-22.md`.

## [0.1.3] - 2026-09-22

### Added

- Onboarding points at one place: a run with no model configured, and `/model`
  with nothing set, both say `run /connect to set up your provider & key` (with
  `/model provider/id` and `/help` as the alternates) instead of naming
  `/provider`, which `/help` never listed.

- Official plugins now ship in this monorepo under `plugins/` rather than a
  separate `grayplugins` repo, so `gray plugin install <name>` can never point
  at a repo that does not exist. Each directory is one sidecar — a single
  `plugin.sh` speaking protocol v1 NDJSON over stdio, with `echo/` as the
  reference implementation — and `plugins/index.json` is the catalog
  `gray plugin install` reads. Push tag `plugins-v<version>` (which must equal
  every manifest's `version`) to run `plugins-release.yml`: it syntax-checks
  each sidecar, tarballs the directories, and publishes a `plugins-v<version>`
  release with `SHA256SUMS-plugins`. `discord/`, `background/` and
  `permissions/` are scaffolds until the real bridge/runner/gate logic is
  ported in — each file's TODO says where.

### Fixed

- Compaction pinned the stable anchor by *value*: the retained tail was
  filtered with `retain(|m| m != &anchor)`, which deleted **every** message
  equal to the original intent — including a later turn that legitimately
  repeated the prompt — and left the request ending on an assistant message
  instead of a user turn. The walk now starts past `candidate[0]`, the
  anchor's own position, so only that one copy is excluded. As a side effect
  the anchor's tokens are no longer charged to both the pinned segment and
  the tail (arXiv:2512.22087).

- The context-overflow path compacted the *unscrubbed* history: a salvaged
  partial that the CCRM scrub (arXiv:2605.08563) had flagged rode the
  compaction trigger in full, so the failed trajectory was baked into the
  summary and reached every later request — exactly what the scrub exists to
  prevent. The scrub is now one view (`Agent::scrubbed_messages`) shared by
  the outbound request and the compaction input, so both carry the one-line
  marker. The persisted transcript still keeps the full text the user saw.

## [0.1.2] - 2026-09-22

### Added

- `/gateway` (alias `/gw`) opens a connections panel: one toggleable row per
  installed app (the merged plugin registry, so a transport appears the day it
  is installed), a rule, then one-line pointers at daemon, cron and memory —
  `gray gateway status`, `/cron` and `/memory` already print everything about
  those, so the panel names the command instead of restating its output. An app
  row carries what it still needs (`needs setup` when its default config file
  is absent — existence only, never the file, which holds the token) and the
  commands its own manifest declares; nothing is invented on its behalf.
  `space` flips an app's enabled flag through the same registry path
  `/plugin` uses, `/gateway on|off` still flips the persisted gateway master
  switch, and piped stdin prints the rows as text. `/gateway` and `/gw` are
  real commands again (they previously answered "the TUI gateway is gone")

- `gray login`, `gray whoami`, `gray logout` (and `/login`, `/whoami`,
  `/logout` in the REPL): enroll this machine with gray.alignment.id. The
  site's account page mints a one-time 5-minute code from a Supabase session;
  `gray login` exchanges it for a long-lived `gray_...` registry token stored
  in `~/.gray/registry-token.json` (mode 0600, same atomic writer as
  `auth.json`). Bare `gray login` prints the walkthrough and prompts for the
  code; `gray login <code>` is the non-interactive form the site's copy
  command emits. `gray logout` revokes the token server-side before dropping
  it, and logging in again revokes the token it replaces so a re-login never
  leaves a working credential the machine has forgotten. All three run before
  provider configuration, so a fresh machine can enroll before it can run a
  turn. `GRAY_REGISTRY_URL` points at a local registry
  (`pnpm backend:dev`); cleartext http is accepted only on loopback, since
  every call carries the token. Nothing in gray is gated on an account — the token
  only names the caller on registry calls — and the onboarding banner now says
  so instead of implying a login exists

- `/cron` and `/memory` are interactive on a TTY, riding the same picker loop
  as `/plugin` and `/skills`: `/cron` lists every job (name, id, schedule, next
  run, last status) plus the ticker's liveness row, and `space` pauses/resumes
  in place through `CronStore::set_paused`; adding and removing stay on the
  `gray cron` CLI. `/memory` lists every curated entry (key, scope, first
  line) read-only — forgetting stays `gray memory remove <key>`, because a
  picker must not make deletion a keystroke. Headless output is unchanged for
  both. The shared manager loop gained the axes these panels needed:
  per-row `read_only` (separators and pointers carry no switch), a
  `supports_remove` flag (listing panels leave removal to their command), and
  an `errors_tab` flag (package-install errors are noise on a cron listing)

### Changed

- Memory entries carry their latent reasoning. The policy now asks every saved
  entry to record the failure or correction that prompted it (quoted), whether
  it has recurred since, what was already tried and falsified, and the verbatim
  text of any entry it replaces — the keep/delete rule from arXiv 2608.11095,
  whose finding is that an instruction nobody can justify is an instruction
  nobody can safely delete, which is why prompt files only ever grow. A why
  without its outcome is worse than none. The rule itself: if an entry's failure
  has not recurred since the entry was added it is probably preventing that
  failure, so keep it; delete only when the failure kept recurring anyway or the
  entry duplicates another's target, and carry the removed entry's falsified
  attempts into its replacement
- `gray memory audit` reports which entries lack a why, which duplicate
  another's target, and which record a falsified outcome, with the rule above
  printed beside them. It deletes nothing — the paper's own warning is that
  automating the deletion emptied one prompt in eight and lost satisfaction on
  exactly those — so the decision stays with a human
- Repeated net growth with no removal (three consecutive saves) now prints a
  one-line warning pointing at the audit: unbounded growth is the disease, and
  it is visible in the entry count long before it is visible in behavior
- `AGENTS.md` / `CLAUDE.md` may carry `# r<n>: ...` rationale comments for a
  rule. They stay in the file for whoever edits it and are stripped before the
  rules reach the model, so rationale never costs the executor tokens — and
  the served block says so, so an editor preserves them. Files without such
  comments render byte-identically to before
- Windows installs natively by default. `dist/install.ps1` no longer routes a
  bare invocation into WSL: with no arguments it installs `gray.exe` into
  `%LOCALAPPDATA%\Programs\gray\bin`, verifies the archive checksum, extracts
  only `gray.exe` / `LICENSE` / `THIRD_PARTY_NOTICES.md` from the ZIP, and
  updates the user PATH. `-Wsl` remains an explicit compatibility route that
  pipes `install.sh` into a distro, and a native failure never falls back to
  it. The "experimental" and "acceptance pending" wording is gone from the
  installers and docs, and the README platform table lists Windows as native
  x86_64. Still documented as unsupported, unchanged by this: gateway and cron
  execution, self-update, Unix-shebang plugins, and ACL hardening of
  credential files

## [0.1.1] - 2026-09-21

- Windows builds ship with the release: `gray-<channel>-x86_64-windows.zip`
  alongside the four tarballs, checksummed into the same `SHA256SUMS` file.
  `install-native.ps1` installs it without elevation, probes the binary with a
  bounded 10s timeout, and replaces a running `gray.exe` through a staged
  copy-plus-backup rather than an in-place write. It is exercised in CI on
  windows-2025 under both PowerShell 7 and Windows PowerShell 5.1. The public
  `install.ps1` still defaults to installing inside WSL; native is opt-in via
  `-Native`
### Added

- Prompt-cache warmth timer + cache-miss warning in the composer. The
  footer carries a `◷ 4m` countdown next to the cache-hit percentage —
  how long the last request's prompt cache stays warm before an idle gap
  re-bills the whole prompt (Anthropic's `cache_control` and OpenAI's
  automatic prefix caching both expire after ~5 idle minutes), fading in
  the last minute and hidden entirely when the provider never reports
  cache activity. When a request re-bills tokens the previous one should
  have served from cache — an idle gap past the TTL, a model switch, or a
  provider-side eviction — the transcript gets a warning row
  (`⚠ Cache miss after 6m idle: 100k tokens re-billed (~$0.35)`) once the
  miss crosses 20k tokens or $0.10 over a full hit, so routine breakpoint
  noise stays silent. Detection is a port of pi's
  `packages/coding-agent/src/core/cache-stats.ts` (reference checkout
  under `reference/pi-mono`) onto gray's per-round `StepUsage` reports;
  compaction and `/new` reset the baseline, since a fresh summary is new
  content rather than re-billed content
- Remove a provider from the connect modal: `shift+enter` on a highlighted
  provider (the modal footer advertises it only for a row that holds a stored
  credential) opens a confirmation naming the provider and its endpoint, and
  `enter` deletes the credential — `auth.json` entry plus the active
  provider's second copy in `config.json`, so a removal cannot resurrect on
  the next start. `/provider` reloads the agent and reports the removal
  instead of announcing a connection
- Gateway daemon (`gray gateway ...`): the always-on host that fires cron with
  no REPL open. `run` is the foreground daemon (60s cron ticker with the same
  `HeadlessRunner`/`SaveLocalDeliver` as `tick`/`serve`, plus a control socket
  at `$GRAY_HOME/gateway.sock` answering `identify`/`status` — one JSON line
  in, one out, hermes wire shape); `install` writes a user service (runit
  `run`+`log/run` on Void, systemd `--user` unit elsewhere, `--print` previews,
  `--no-start` defers), `start`/`stop`/`restart` drive it, `uninstall` removes
  it, `status` reports daemon + service + ticker health (exit 1 when down).
  Process shape follows the hermes gateway: O_EXCL pid claim with
  `/proc` start-time against PID reuse, `gateway.state.json` recording why the
  last run stopped, socket-first liveness with pid-file fallback, 0600 socket,
  supervisor detected from the real init (never inferred outside-in), and a
  bounded 65s drain of in-flight fires on SIGTERM/SIGINT. `install`/`start`
  wait up to 10s for `runsvdir` to pick up a fresh service dir (it rescans
  every ~5s) instead of racing it, and `stop` tells runit-stopped vs
  tick-draining apart (was a `⚠ still running` either way).
- Cron in-chat firing: background REPL tick, `/cron` dashboard, delivery seam.
- Cron workstream B: `gray cron tick|serve|pause|resume|run`, job skills +
  pre-run scripts, local-file delivery (`cron/output/<id>/<ts>.md`).
- Cron ticker liveness: every tick pass writes a heartbeat
  (`$GRAY_HOME/cron/.last_tick`), and `gray cron list`, `gray cron add`, and the
  `/cron` dashboard report it — a store nobody is ticking now says so instead of
  printing a `next=` that will never arrive.
- Plugin system: `gray-plugin` crate with Plugin trait, builtin tools as profile-ordered plugins, `gray.yml` profile loader + sidecar entries, sidecar hook protocol over stdio with timeout/crash degradation
- Gateway daemon: `gray-gateway` crate (Telegram/Discord/Slack), real Discord adapter with slash commands, OAuth2 invite URL, full `/gateway` REPL suite, delegation durability
- Cron jobs: schedule/store/CLI + REPL, local wall-clock daily schedules, AI self-scheduling via `schedule_task`
- Dynamic context window via models.dev + disk cache (LiteLLM table, proportional reserve/keep), `/context` visual modal with suffix completion and thousand-separator parsing
- Usage/cost tracking: session cost from LiteLLM rates, footer + `/usage`, persisted session totals
- Skills: `skill` tool loading SKILL.md bodies, `/skills` command, discovery across opencode plugins/agents/claude skills
- Anthropic prompt caching always-on with cache hit-rate display; reasoning summaries on Responses API
- `request_user_input` tool (question overlay) so the agent can ask the user mid-turn
- Codex-style session resume (`--last`/`--all`, picker, transcript replay)
- `/effort` thinking-level selector and `/thinking` toggle
- Auto-compact on threshold/overflow; exploration-stall guard
- Noir space marketing site (landing/docs/pricing, favicon/OG/robots/sitemap) deployed to gray.alignment.id
- Multi-platform releases (darwin x86_64/aarch64, linux aarch64) with release channels (`RELEASING.md`)
- Startup update check + `gray update` subcommand
- Bulk hermes→Rust 1:1 port slices across core/cli/tui/tools/plugins/gateway/provider/sandbox/state/cron/acp
- Effort picker filtering: online models.dev capability filter with offline static ARM fallback
- Wire is_error flag, DeepSeek provider path, resume-replay, retention policy, and chat-shard routing
- Windows cfg gates for platform-specific code paths
- Resume messages for restored sessions
- Shift+Enter newline handling in composer
- Footer/badge/panel TUI chrome updates
- Attach guards for file attachment paths
- Glob tool for file pattern matching
- Session-ID threading across turns
- prompt_cache_key passthrough for chat requests
- `gray sessions prune --older-than-days N` for session-store GC; `persist_redacted: true` gateway option to scrub secrets from persisted gateway transcripts
- Verified installs: SHA256SUMS published per release, checked by install.sh (S1)
- `GRAY_NO_UPDATE_CHECK=1` and 24h update-check cache (L4)
- Gateway autostart defaults off; corrupt gateway.yaml warns instead of silently resetting (S2, S3)
- Safety / Subcommands / Platform / gateway docs in README (D2, S4)

- The connect modal's footer and the install manager's per-tab footers share
  extracted same-file helpers instead of repeating the render scaffolding
  three times each (-188 net lines across the two files). Behavior is
  unchanged, including the connect modal's conditional `shift+enter` hint,
  which only appears for a row that actually holds a stored credential
### Changed

- `gray plugin install discord` compiles the plugin from its pinned commit
  (`cargo build --release --locked`) instead of pip-installing a Python
  package, so installing a first-party plugin needs no interpreter. The
  catalog is Rust-only; user-written plugins stay language-agnostic
  (`GRAY_PLUGIN_PATH`, `plugin.sh`). A source pin must be a full commit ID,
  and the built binary has to answer `plugin/manifest` with the expected
  name before it is registered
- Clipboard/image paste is core again: `arboard` + `image` are always compiled in, no `--features clipboard` needed (kept as a no-op alias)
- Removed the native messaging gateway: deleted `crates/gray-gateway` (adapters, daemon, pairing, delivery, systemd), the `plugins/gateway` sidecar, `gray gateway ...`/`gray send`, and the `telegram`/`discord`/`slack`/`all-platforms` features. Chat returns as a plugin; `gray cron --deliver` targets are stored opaquely until a delivery backend exists. Dropped the `--all-features` CI checks.

- Multi-line input is no longer clipped by the inline viewport. The viewport
  cap was pinned near 14 rows regardless of terminal height, so a pasted
  paragraph that wrapped to 12 content rows lost its last row and anything
  longer was cut hard. The cap is the terminal height now (minus the shell
  prompt row); the idle 14-row transcript is unchanged
- The default tool surface stays bash-only, and `bash` now carries image
  vision: `cat <image>` returns the file as a vision block at full
  resolution (decode, EXIF orientation, re-encode at native size — no
  downscale, no halving) instead of the binary garbage a shell would
  stream. The claim is deliberately narrow — exactly `cat` plus one bare
  path — so flags, pipes, redirects, globs, and multi-file cats run
  normally, and missing or mislabeled files fall through to the shell's
  own error. A separate `view` tool was tried and removed the same day,
  on one principle: a capability the existing surface can carry does not
  need a new tool. `tools-minimal` is pinned bash-only by a test
- Project rules arrive without being asked for: the nearest `AGENTS.md` /
  `CLAUDE.md` above the working directory is served every turn as a
  self-describing `<project_context>` block (nearest ancestor wins, the
  gray-home file is skipped since it is the stored system prompt, 32K-char
  cap), so repo rules stay in the permanent prompt instead of arriving as
  prunable tool observations that can fall out of context mid-session
- An explicit `/skills <name>` invocation now pastes a binding directive
  ahead of the skill body — the instructions are binding for the current
  task, drop any conflicting plan — because a bare body pasted mid-task
  reads as background material and the model resumed its previous plan.
  The per-turn `<available_skills>` block gained the matching mid-task
  clause: stop and read a matching skill before continuing
- The default system prompt is 31 lines, down from 39: the three prose
  sections folded into workflow steps and two guideline bullets (both
  verify-contract sentences preserved verbatim), and the project-rules /
  skills narration deleted — both blocks explain themselves

### Fixed

- Tool headers no longer panic the REPL on multi-byte commands. The
  one-line command/arg preview byte-sliced at a fixed offset 80, so a
  command whose first line carried an emoji (or any wide char) straddling
  that offset — a pasted PR review's 🟠 merge-risk marker did exactly this,
  killing a live session mid-tool-call — crashed with `byte index 80 is
  not a char boundary`. The cap is now counted in display cells and cut on
  a char boundary (the repo's `text_width` helpers), so ASCII commands cut
  at exactly 80 as before and wide text fills the same 80 cells instead of
  a quarter of the way in. The same fixed-offset pattern was audited
  across the crates; the only other byte-slice site is ASCII-guarded
- Bash has no default timeout any more: commands run until they exit, and an
  explicit `timeout` is opt-in (clamped 1–3600 s) with the agent-level
  last-resort stop raised to 3660 s. The tool description used to promise
  120 s while the code killed at 30 s.
- A failing command piped into a pure text filter no longer reads as success.
  `sh -c` reports only the last stage's status, so `pytest -q | head -40`
  returned `exit 0`; the exit report now names the masked stage and covers
  `head`/`tail`/`cat`/`less`/`awk`/`sed`/`tr`/`cut`/`column`.
- Missing commands are explained instead of just failing: the result carries
  "`rg` is not installed here · use an equivalent you already have …".
- Truncated output names the omitted byte window and the offset of the next
  page, and pages are 16 KiB instead of 4 KiB.
- The shell inline budget is 48 KiB (was 12 KiB), the turn event cap is 500k
  (was 100k — long runs died on "turn event limit exceeded"), the loop guard
  nudges at 3 identical tool+args and only aborts at 6, and a dropped
  provider stream keeps complete tool args with a warning instead of killing
  the turn.
- Sampling is reachable: `GRAY_TEMPERATURE`/`GRAY_TOP_P` (env or saved
  config) are sent with every request and omitted when unset. Memory size
  caps are gone and `gray memory list/show/set/edit/remove/clear` exists.
- tps is now measured over streaming time only. Every rate (working pill,
  end-of-turn `Thought for … · N tps`, headless footer) divided by the
  whole-turn duration, so a turn that spent 40s in tool calls and 4s
  generating reported ~13 tps instead of ~125 — tool waits and inter-round
  gaps now never enter the denominator (`TurnStreamClock`). The `· 40s`
  duration next to it stays whole-turn on purpose
- Interrupted turns now save a complete transcript. The REPL gave a cancelled
  run 5s to clean up and then dropped it; a stall nothing can interrupt (a
  plugin `tool/before`/`pre_tool` hook, an in-flight compaction, a tool that
  ignores cancellation) skipped the loop's own cancel cleanup entirely, so the
  session stored an assistant `tool_use` with no `tool_result` — strict
  providers then 400 on resume and the session is bricked for good — and the
  partial text the user had already watched stream by was lost. The turn is now
  repaired on the way out (`Agent::repair_dropped_cancel`): unanswered calls
  get a synthetic `cancelled by user` result and the streamed text is
  salvaged, exactly once
- `plugin list` / `/plugin list` now show `install plugin` commands (e.g. discord): the list merges `commands.json` CLI entries with `lock.json` sidecars (CLI rows tagged `[command]`), and `enable`/`disable`/`remove` route to whichever registry owns the name (was `not installed`); `update <command>` warns and no-ops like other non-index sources. `gray plugins` (CLI) is pinned as the `gray plugin` alias by test
- Cancelling a turn no longer discards the in-flight tool's own report: both
  cancel paths (single dispatch, parallel join) abandoned the future on the
  same token the tool watches, so partial output and the process-group kill
  never ran and the turn answered with a bare synthetic `cancelled by user`.
  A cancelled tool now gets a bounded 3s window (inside the turn's own
  cooperative window) to report, then the turn ends with that output in
  history.
- Cron silence: `next=` rows look identical whether or not a driver (`serve`, a
  `tick` host, a REPL) is running, so jobs could sit due forever unnoticed.
  Overdue jobs are now named in `list`/`/cron`, and `add` warns at creation when
  no ticker has run inside the liveness horizon.
- Skills: folded (`description: >`) and literal (`|`) frontmatter now parse (were the bare marker) + `gray plugin install` accepts bare `https://github.com/<owner>/<repo>` URLs as git sources — `https://github.com/DietrichGebert/ponytail` installs all six skills, same as `npm:@dietrichgebert/ponytail`
- Project context: `AGENTS.md` / `CLAUDE.md` (cwd up to git root) now auto-attach as `<project_context>` hook context every turn — no more manual `cat`, and `/context` bills the exact block instead of showing `0 tokens`. `~/.gray/AGENTS.md` excluded (never double-billed); 16k chars per-file cap
- Modal backdrop: textarea copy pinned dim (box bg + text through the color map) with a universal regression test — no full-brightness composer surface may survive in any modal backdrop
- Dogfood (headless/pipe): bare `/thinking` and `/model` print status instead of a raw `No such device` error, bare `/resume` and `gray resume` (no TTY) list sessions as text, `/compact` with no model prints one line, exit-hint has no raw ANSI when piped
- Dogfood (plugin check): reference echo sidecar returns valid JSON on `{}` args — `tool/call` + concurrency checks pass
- `gray2` binary target (`cargo build` yields `gray` + `gray2`) + one regression test, zero warnings
- Dogfood (modal backdrop): input/footer text behind modals dimmed through the color map, not just SGR faint (was full-bright on terminals ignoring faint)
- Piped `/thinking` status lists the current model's filtered levels (same provider-driven filter as the modal), not the full catalog
-- `/thinking` levels: qualified models.dev entries (kilo/openrouter `meta/muse-spark-1.3-contributor` WITH `max`) no longer leak `max` into the bare contributor id via the suffix alias (gap-fill; exact provider keys stay authoritative) — the live picker showed `max` once the background models.dev fetch landed
-- Modal backdrop: transcript user cards now dim to near-black like the input box (the preserved full-gray card glowed through behind modals)
- Auto-compact UI (Codex parity): threshold/overflow/manual compaction raises a dedicated `Compacting context` status with its own clock before the summarization call; follow-up status writes can't obscure it, only the matching completion posts `Context compacted · {elapsed}`, and the input box stays mounted (viewport/transcript/textarea untouched)
- Bash-only tools with skills context: `tools-minimal` is `bash` + `shell_output` + `shell_kill` + `sleep` (no `skill` tool). The always-on context-only `skills` plugin appends the fresh `<available_skills>` list each turn and the model reads matches with bash (`cat <location>`); `/skills <name>` still pastes the skill visibly into chat before running it
- Working pill: clock anchors to the turn start (tool `Preparing tool:`/`Working` re-stamps no longer restart it at 0.0s) and the spinner carries no token estimate — exact counts stay in the footer gauge (context), the `Thought for · N tok` line (billed turn output) and `/usage` (session). Removes the chars/4 live estimator that read 2.5M on a 14s turn
- Skills: `/skills [name] [args]` (alias `/skill`) replaces the `/skills:<name>` colon form; bare `/skills` lists all discovered skills (global + project), not just `~/.gray/skills` installs, and no longer prints the text list on top of the TTY manager
- Synthesize tool outputs for orphaned function calls (unbricks sessions after mid-turn cancel)
- Classify upstream 5xx as ServerError; connection-safe errors with 10s timeout
- Detach bash tool with setsid to prevent password-prompt hangs; char-safe log preview (emoji byte-slice panic)
- SQLite session store WAL pragma; persist/restore token usage across resumes
- Legacy daily UTC crons auto-migrated to local wall-clock
- Continued TUI polish (card-box transcript/composer, alternate-screen modals, codex-style diffs, resize/reflow fixes)
- Ponytail cleanups: dropped mermaid renderer, vendored hermes port, rusqlite bundled feature, assorted dead code
- Modal dim fix for overlay backdrop rendering
- 429 backoff floor for rate-limit retries
- Billing-429 treated as terminal (no retry on insufficient credit/quota)
- 413 overflow handled as context overflow path
- Retry-After parsing for ms/date formats with cap
- REPL crash hardening against malformed input/panics
- Guard bypass batch 1+2 fixes
- Config strictness: reject unknown fields
- Popup restore on resize/refocus
- Unknown-tool fail-closed handling
- Clipboard async copy path
- History recall (Up/Down) while a turn is running, matching idle prompt behavior
- `Thought for` line and live status counter are per-turn (count from zero, final at turn end); session context stays in the footer gauge
- Log caps to bound disk/memory growth
- Transcript bound for long sessions
- Executor watchdog for hung tool runs
- Models-cache atomicity via atomic writes
- Markdown rendering fixes
- Cron: stale fire claims (>300s) from a crashed ticker are reclaimed instead of wedging the job forever
- Gateway: terminal adapter failures (revoked token, adapter not compiled) are parked until restart instead of re-entering the reconnect ladder forever
- Release: tarballs build with `--features all-platforms` (real adapters, not stubs); CI tests the all-platforms gateway code and asserts release artifact architecture
- Update: `GRAY_AUTO_UPDATE=1` auto-updates on the stable channel only; update trust model documented in SECURITY.md
- README: 0.x stability line, user-side rollback note, and raw-session persistence disclosure
- Manifests: gray-cron declared once in workspace.dependencies (GRY-003)
- README: default vs feature-gated source-build table; no blanket "ships in the binary" claim (GRY-004)
- Deps: gray-acp is an optional default-off `acp` feature (GRY-001); release builds use `--features all-platforms,acp`
- Deps: workspace Tokio declares explicit features instead of `full` (GRY-002)
- README demo GIF: dropped the 3.5s dead lead-in, stable 10fps, diff palette (2.3MB → 1.4MB, 12.8s → 9.5s)
- Stable update channel: `latest-stable.txt` now published; beta builds embed the beta channel (D1)
- Single-writer publish job: all four platform tarballs land atomically (D4, R2)
- Installer defaults to `~/.local/bin` (`--system` / `GRAY_INSTALL_DIR` for system-wide) (L3)
- Swapped unmaintained `serde_yaml` for `serde_yaml_ng`; `cargo audit` in CI (C2)
- Gateway `--help` names all three platforms (Telegram/Discord/Slack)
- TUI: bottom status line spans the full width; diff overlay extends fully to the right edge

### Known issues

- macOS binaries are not notarized (curl-install unaffected) (D3)
- Destructive-command guard is best-effort, not a sandbox — see README Safety (S4)

- `install.ps1` still installs through WSL by default; a native install
  needs `-Native`. Self-update refuses on native Windows rather than calling
  the WSL installer — close Gray and rerun `install-native.ps1`

### Added

- `GRAY_NO_JOBS=1` drops the managed-job surface: no `action`, `job_id`,
  `background`, `yield_ms` or `wait_ms` in the bash schema, those actions are
  refused with a message that says why, and the 600s-silence stall notice
  stops advertising an await it cannot perform. `timeout` stays — it is the
  anti-hang knob, not a jobs feature.

### Changed

- The default system prompt is 1,645 chars, down from 3,564 (~891 to ~411
  tokens on every turn). Cut: everything a capable model already does
  unprompted (`cat` is text, `rg`/`grep` exist, read the project's AGENTS.md)
  and everything the bash tool's own schema already states every request (the
  job API, "output is text"). Kept: every gray-specific fact (`gray view`,
  `gray find`/`gray grep`) and every discipline clause the benchmark retro
  measured (`your own passing check defines nothing`, `every public entry
  point`, `an error path nothing can reach is unimplemented`, one-shot probes,
  checklist-not-happy-path).

- The per-turn `<available_skills>` block: preamble cut from six sentences
  (~1,000 chars) to one (~400), and the list capped at 12 instead of 40. The
  descriptions and locations are untouched — those are the feature. A 40-skill
  install drops from ~24 KB to ~7 KB per turn.

## [0.1.7] - 2026-09-29
