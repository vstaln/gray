# Reference Study Protocol

Applies to every clone under `reference/` — 74 existing ones and every new checkout.
Governs how reference code may inform gray, and how agents are dispatched to read it.

## The rule

**Design parity only.** Study architecture, design, and constants; write your own code.
Never copy verbatim code, identifiers, comments, doc or README prose, or UI strings —
from **any** version of the referenced project.

## Why

- **Language swap is not a defence.** Copyright protects expression — structure,
  sequence, organization, function decomposition — not syntax. A TypeScript to Rust
  port is a *translation*, and translations are derivative works. "we are using pure
  Rust, not Bun" does not move the line at all.
- **The test is substantial similarity, not similarity of surface tokens.** If your
  Rust file's decomposition and naming map one-to-one onto the upstream file, it is a
  port no matter how much the syntax changed. Rust idioms — ownership, enums, tokio,
  crates — make natural divergence available; take it.
- **A restrictive upstream license removes the license entirely.** A
  noncommercial-only source (PolyForm Noncommercial and similar) grants no right
  to port for a commercial product, so the question of how clean the port is
  never arises.
- **A permissive upstream license only *permits* the port.** It does not make a port
  better engineering. Design parity reaches the same understanding without the
  license risk at all, and without inheriting someone else's boundaries.

## What to extract

- Architecture: component boundaries, who talks to whom, where state lives
- Protocols: wire shapes, ordering guarantees, crash-safety sequences — described
  behaviourally, never transcribed
- Numbers: batch sizes, timeouts, retry counts, buffer limits, compaction thresholds
- Failure modes: what breaks, what the recovery is, what the operator sees
- Product decisions: what they chose *not* to do, and why

## What never crosses

Verbatim code of any kind. Copied identifiers, function or field names. Comments.
Doc and README prose. UI strings and error messages. Config keys and schema field
names. Any of the above fed to a model with "rewrite it in Rust" as the instruction.

## The 1:1 test

Before a reference-informed file lands: open it beside the upstream file. If every
function in yours has a counterpart there with the same name and the same neighbours,
it is a port. Restructure — split, merge, rename from your own vocabulary, or solve it
in a shape Rust makes natural — until the answer is no.

## Agent brief

Paste verbatim at the top of any study dispatch. It is written to be safe by
construction rather than by review, because review catches too late.

> You are studying a third-party open-source project to inform gray's design. You must
> NOT copy from it.
>
> **1. No verbatim text.** No code, no identifiers, no comments, no doc or README
> prose, no UI strings, no config or schema keys.
>
> **2. No translation.** Rewriting this project's TypeScript as Rust line by line is
> copying, not studying. Changing the language does not change the analysis.
>
> **3. Your deliverable is a DESIGN NOTE in your own words**, not a port. Write it
> FIRST, after at most 3 file reads, then refine — a committed partial note beats a
> dead agent.
>
> The note must contain:
> - What the design does, in your own vocabulary
> - Component boundaries and data flow
> - Protocols and ordering guarantees, described behaviourally
> - Concrete numbers: limits, timeouts, batch sizes, thresholds — state the number,
>   not the code that computes it
> - Failure modes and their recovery paths
> - Tradeoffs: what they chose, what they gave up, what they deferred
> - Open questions for gray
>
> Implement in Rust only if asked, and only from your own note. Never have a project
> file open while writing implementation code.
>
> **Self-check before finishing:** would a reader who knows the upstream recognize
> your file structure as a rewrite of theirs? If yes, restructure until the answer
> is no.
