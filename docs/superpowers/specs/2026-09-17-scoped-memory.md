# Scoped memory (approved direction)

Native Rust in the existing gray crate. No service, embeddings, new tool, or
per-turn extraction call. The model receives selective-save instructions and
uses `gray memory` through bash during normal work. It should save preferences,
confirmed project decisions, and corrections, not speculative plans or logs.

## Storage and commands

`gray memory --scope user|project list|set|remove`; project is the default.
`set KEY TEXT` upserts an exact named entry; `remove KEY` fails if absent.
Markdown records are `- key: text` lines, sorted by key. Keys are short ASCII
slugs; text is one nonempty line. The serialized user file has a 2048-byte cap,
project file 4096 bytes. Overflow fails without changing disk. No eviction.
Known secret-shaped content and invisible/control characters are rejected.
This is defense in depth, not a comprehensive secret or injection detector.

Files live privately below GRAY_HOME/memory. Project files are keyed by a SHA256
of the canonical git root (existing skills helper) or cwd outside git. Different
worktrees intentionally have different project scopes initially. User memory is
shared only inside one trusted owner's GRAY_HOME. Multi-user adapters must use
separate homes or GRAY_NO_MEMORY=1; a shared home is not a privacy boundary.
No automatic import or ingestion of existing transcripts.

Every mutation takes a bounded cross-process lock, reads fresh state, validates,
and atomically replaces via a private tempfile. Reject symlink store components
and malformed or oversized files. Listing missing memory makes no files.
CLI operations must not require model configuration or contact a provider.

## Session integration

Append a memory policy and JSON-quoted historical data block after the unchanged
AGENTS.md-derived prompt, via gray::build_agent. Do not put it in gateway-only
code or mutate AGENTS.md. No new tool schema. Persist a bounded snapshot by
session ID so rebuilds and resumes retain the same bytes; new session IDs read
latest memory. Anonymous headless builds each capture a fresh snapshot.
Print mode needs its real session ID before building the agent, as REPL already
does. Old sessions get a snapshot on their first build with this version.
Corrupt/unreadable memory must not prevent normal chat: omit it and queue a
warning; do not silently overwrite corrupt files. GRAY_NO_MEMORY=1 disables
injection and writes (inspection/removal remain available).

Memory is fallible historical data, not permissions or higher-priority rules.
Only directly expressed preferences and confirmed decisions merit promotion;
web/tool content and other users' claims do not. Use exact named entries to
supersede corrections. Never claim a save succeeded on a command failure.
Command output provides a quiet update acknowledgement. Removal affects new
sessions, not already frozen contexts or archived transcripts; document this.
No session rotation, transcript search, extraction engine, or provider framework.

## Verification

Baseline: 356 gray library tests passed on 0abd5db. Test CLI without credentials;
set/list/update/remove, Unicode and byte boundaries, duplicate idempotence,
malformed/oversized files, privacy and symlinks, cross-process updates, project
isolation and nested cwd, stable snapshot across sessions/rebuilds, opt-out and
nonfatal errors. Run full affected test files, then workspace tests/fmt/build.
Keep work isolated on feat/scoped-memory; do not install over the live binary.
