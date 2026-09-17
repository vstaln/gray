# Approved runtime reliability work

Work in detached snapshots, not the active checkouts. Baseline includes the
user's uncommitted changes, recorded separately; never publish them as our work.

1. Extend the existing print runner with opt-in NDJSON v1. Return stable session
   and invocation IDs, safe progress, final assistant text, usage, terminal errors.
   Keep human output and existing persistence/error cleanup intact. Prove with a
   loopback provider and real binary, including redacted prompts and resumption.
2. Switch Discord runner to that stream. Persist explicit session IDs, bounded
   stream parsing, sanitized diagnostics, configurable deadlines and cancellation.
   Keep old sessions resumable without scanning them for answers.
3. Durable SQLite inbox/outbox. Deduplicate incoming IDs, serialize conversations,
   retry delivery independently of generation; interrupted generation marked
   uncertain, never automatically rerun side-effecting work. Preserve per-chunk
   delivery progress; ambiguous sends must not promise exactly-once delivery.
4. Persistent budget ledger, model request limits, no unknown-pricing-as-free.
   Require explicit pricing/budget policy for paid unattended use. Document that
   client accounting cannot guarantee provider-side invoice caps.
5. Session maintenance, explicit capability selection, schedule CRUD online,
   compatibility migration from jobs.json, and real runner/background recovery tests.
6. Verify whole touched test files, build/check (cargo test forbidden under X),
   reread all writes, report remaining limits honestly. Do not deploy or replace
   active binaries/services until integration is approved.
