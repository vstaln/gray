# Resident search index (fff-search) behind `find`/`grep`

Gray's `find` and `grep` tools now try a resident, watcher-backed file index
before spawning `fd`/`rg`. The tool names, JSON schemas, and output format are
unchanged; the index is an acceleration layer, not a replacement.

## Why

`fd`/`rg` are one-shot CLIs: every call forks a process, re-stats the tree,
re-parses `.gitignore`, then exits. An agent session may run hundreds of
searches against the same repo. `fff-search` keeps the file index and a
content cache resident in-process, so repeat queries hit warm memory and gain
frecency ranking (recently/frequently accessed files rank higher) for free.

## Design

```
find:  indexed_glob  → declines → fd subprocess → ignore-walk builtin
grep:  indexed_grep  → declines → rg (vimgrep / --json) → grep_builtin
```

- `crates/gray-tools/src/search_index.rs` owns everything fff-related:
  `SearchPool` keeps one `SharedFilePicker` per canonical search root,
  created lazily on first use (an eager index would pay a full scan for a
  cwd that never gets searched). The picker owns a background filesystem
  watcher, so files written mid-session are found without a rescan.
- Frecency databases live under `<gray_home>/fff-frecency/<hash-of-dir>`,
  so ranking survives restarts. `global_pool()` serves tools constructed
  without an explicit pool; tests inject their own via `with_pool`.
- Every `indexed_*` entry point returns `Option`: `None` means "index
  unavailable — fall back". The tool contract never depends on the index.
- Index calls are synchronous and may wait for the first scan; tools run
  them inside `tokio::task::spawn_blocking`, raced against `ctx.cancel` the
  same way the fd/rg lanes are, with a 10s scan budget before deferring to
  the fallback lanes.
- The pool keeps at most `MAX_RESIDENT_ROOTS` indexes warm (oldest root
  evicted, which drops its watcher) and caps each content cache at
  `CACHE_FILES`/`CACHE_BYTES` rather than fff's auto-sized 512 MB.

## Where the index declines (deliberate)

- **Non-git directories.** fff's walker skips hidden files outside git
  worktrees and only honors `.gitignore` inside them — both diverge from the
  existing `fd --hidden` / `require_git(false)` contract. The index only
  serves directories under a git worktree; everything else falls through.
- **`ignoreCase: true`.** fff 0.10.x only offers `smart_case`, which cannot
  express "always insensitive". A `(?i)` regex shim was rejected because it
  pushes literal searches onto fff's slower regex engine; `rg` serves these
  natively instead. Revisit on fff 0.11, which adds `case_mode` —
  `insensitive` + `PlainText` keeps the fast literal path.
- **Negated globs** (`glob="!*.rs"`). rg reads `!` as exclusion; fff needs
  `Constraint::Not(Glob(..))` and a bare `!` glob would compile as a literal
  filename. Declined rather than translated — the contract stays rg-verified.
- **Invalid regex patterns.** fff falls back to literal matching internally
  and reports `regex_fallback_error`; gray's contract is the error text, so
  the rg lane produces it.
- **Empty constrained searches.** When a glob-scoped query finds nothing,
  fff retries the raw pattern with the glob dropped (`literal_fallback`).
  rg returns zero hits there, so `literal_fallback` is declined too.
- **Depth-anchored globs.** fff compiles globs with globset's default
  `literal_separator = false`, so `*` also eats `/`: `a/*.rs` matches every
  file below `a/` (69 index hits where `fd` reports 23 in this repo), and
  `*_tools*` matches through a directory component. fff paginates *before* a
  caller could filter, so a pattern it cannot answer exactly is declined
  rather than answered approximately. `find` therefore serves only
  slash-free patterns — `fd --full-path` owns the rest — and `grep` declines
  a `glob` argument containing a `/` (a slash-free glob needs no filter:
  globset matches it against the basename, exactly like `rg -g`).
- **Invalid globs** (`find "*.{rs"`). fff matches nothing; `fd` reports
  `unclosed alternate group`. The index declines so the error text is fd's.
- **File targets** (grep `dir` pointing at a file), init failures, and
  scan-timeouts all return `None`.

`find` also appends matching directories (`sub/`) the way `fd` does — fff's
directory search is fuzzy-text only and ignores glob constraints, so the lane
matches `picker.get_dirs()` against the same globset itself. That matcher,
like the file-side filter, is built with `literal_separator(true)` so a
slash-free pattern is a basename glob, which is what `fd` compares.

## Behavior changes

- Result order is frecency-ranked, not sorted. Same set, better ordering for
  an agent; tests assert membership, not position.
- Memory: fff holds the index + content cache resident (~360 B/file), capped
  at 4 resident roots x (4096 files / 64 MB). Measured on this repo: a full
  index costs ~11 MB of RSS.
- First search in a directory pays the initial scan (bounded at 10s, and
  interruptible by `ctx.cancel`); subsequent searches are in-process.
- `find "a/b/*.rs"`-style patterns and `grep` with a `glob` containing `/`
  now always run the `fd`/`rg` lanes — see the decline rules.

## Dependency

`fff-search = "=0.10.6"` in the workspace root `Cargo.toml`; member crates
reference it with `{ workspace = true }`.

**Why the `=` pin.** Normally a version like `"0.10"` lets Cargo silently
upgrade to any newer `0.10.x`. The `=` locks us to exactly `0.10.6` — nothing
changes under us until someone bumps it on purpose. We chose 0.10.6 over the
newer 0.11.0 because 0.11.0 had been released less than a week earlier.
Brand-new releases are risky: they have not been vetted by real usage yet,
and broken or malicious releases are usually caught and pulled within the
first few days. Once 0.11 has aged — and once we want its `case_mode` API
for `ignoreCase` — we can bump the pin deliberately.

**Why `zlob` must stay off.** Cargo crates offer optional "features" —
extra code you turn on in the dependency declaration. `zlob` is one of
fff's optional features, and it requires the zig compiler installed on the
build machine. Turning it on would break `cargo build` for CI and for any
contributor without zig, in exchange for functionality we do not use.

## Testing

`crates/gray-tools/src/search_index_tests.rs` — 22 tests covering pool
caching, gitignore/dotfile parity, glob semantics, grep hits/context/limits,
the `ignoreCase` decline, watcher pickup of new files, and tool-level proofs
that `lane_hits` only increments when the index actually serves a call.

```bash
cargo test -p gray-tools search_index
```

## Reviewer notes

- The fallback lanes (`fd`, `rg`, `grep_builtin`) are untouched; deleting
  `search_index.rs` and reverting the two `execute` blocks restores the old
  behavior exactly.
- `SearchPool` is not yet lifecycle-tracked like `FileLedger` (no `Weak` in
  builder.rs); pools are per-tool `Arc`s today, bounded by
  `MAX_RESIDENT_ROOTS` rather than by session teardown. If session-directory
  churn ever leaks watchers, add a `CURRENT_INDEX` mirror of
  `CURRENT_LEDGER`.
