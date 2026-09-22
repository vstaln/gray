# Plan: memory provenance + bash composability

Status: DONE. Track 1 landed as `fc45d57`, Track 2 as `38e502e`, both on PR
#128 (released after v0.1.2 went out on main). Source: the `arxiv-mega/`
compilation (2026-09-22), action items 3, 2a and 2b.

Two things the plan got wrong, recorded so they are not repeated:

1. The original suffix was `; printf ...`, which ended the command with *the
   report's* exit status and turned every failure into a success. The
   benign-exit table lost `grep`'s "no matches" note. Fixed by capturing
   `$?` and re-raising it; regression test added.
2. The cwd cell was keyed by session only, so a caller handing over a
   different context cwd was silently overridden by a stale record. Fixed by
   storing the base cwd the entry was captured against.

## Track 1 — memory provenance

Why: arXiv 2607.14611 (Bad Memory) shows planted payloads in memory files attack
current and future sessions in shipping systems (Claude Code, Codex), and its
defence is being able to find and purge a poisoned entry. Without knowing which
session wrote an entry, the only cleanup is nuking the store. 2407.12784
(AgentPoison) and 2605.17830 (Remembering More, Risking More) make the same
argument from different directions.

Design constraint: the store is hand-editable Markdown (`- key: text`), so
provenance rides on the line as an HTML-comment trailer rather than changing the
format:

    - key: text <!-- gray:saved=2026-09-22;source=abc123 -->

- Invisible when the file is rendered as Markdown.
- Backward compatible: lines without a trailer parse unchanged.
- Hand-editing a line drops its trailer, which is honest — an edited entry's
  origin is unknown.
- Stripped before the snapshot reaches the model (same pattern as the
  `# r<n>:` AGENTS.md rationale strip), so it costs no executor tokens.

Changes in `crates/gray/src/memory.rs`:
1. `Provenance { saved, source }` + `Default`.
2. `parse()` returns entries plus a provenance map; splits the trailer off the
   end of the line only.
3. `render(entries, prov)` writes trailers (disk); `render_served(entries)`
   writes text only (what the model sees).
4. `change()` carries provenance through and stamps `saved`/`source` on insert.
5. `capture()` serves text only.
6. `list()` shows the date and source so a human can see them.
7. Thread the session id into the in-session save path; the CLI path stamps
   `source=cli`.

## Track 2 — bash composability (no new tool surface)

Why: every bash call is a fresh `sh -c` in a fixed cwd (same as mini-swe-agent,
confirmed in `shell/spawn.rs`), so the model re-establishes its directory
constantly. And the truncation note computes the omitted range but only says
"grep the log path above" — the bench retro measured 380 truncation re-reads per
run.

2a. Session-scoped working directory.
- A per-session cwd cell; each call spawns in the current session cwd.
- The command gains a trailing `; printf '%s' "$PWD" > <tmpfile> 2>/dev/null`.
  Writing to a file rather than stdout keeps the command's output and the
  durable log byte-identical.
- After exit, read the file and adopt the cwd if the directory still exists.
- A killed command never writes the file, so the previous cwd stands.
- The vision `cat <image>` path is exempt (it never spawns).

2b. Paging hint on truncation.
- The omission note already knows the omitted line and byte ranges. Emit the
  exact `sed -n 'A,Bp'` command to page the log, instead of telling the model to
  work out how.

## Order

Track 1, then 2a, then 2b — each with its own tests, full gate after each.
