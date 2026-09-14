# SPEC-04 — Decision provenance log (dsh `--dump-config` lesson)

## Problem
During the gray2 session, `/thinking` showed `max` for
`muse-spark-1.3-contributor` in the TTY while headless probes of the same
binary showed the correct 6 levels. Root cause (found on round 3): the
models.dev background fetch landed seconds after launch, and kilo/openrouter's
qualified `meta/muse-spark-1.3-contributor` entry (WITH `max`) clobbered the
bare id via last-writer-wins suffix aliases. Nothing at runtime said which
cache entry supplied the levels — shell logs record commands, never *why* a
decision resolved the way it did. dsh answers this class of question with
`--dump-config` (inspectable composed config); mini-swe-agent with a full
trajectory file. Gray has neither for model-capability resolution.

Fixed by `9740148` (suffix aliases now gap-fill). This spec makes the next
bug of this shape a one-round diagnosis.

## Design
1. **Provenance return alongside the existing answer.** In
   `crates/gray/src/setup/context/providers.rs`, add a sibling to
   `supported_efforts(model_id)` (do NOT change its signature — callers in
   `effort.rs`, status paths, and tests depend on it):
   `pub fn supported_efforts_provenance(model_id: &str) -> EffortProvenance`
   where
   ```rust
   pub struct EffortProvenance {
       pub model_id: String,        // as queried
       pub levels: Vec<String>,     // resolved levels incl. "off"
       pub source: EffortSource,    // Exact{provider} | SuffixAlias{provider} | FamilyTable | FullCatalog | NonReasoning
       pub detail: String,          // e.g. "kilo qualified key won suffix alias" / "family: muse-spark"
   }
   ```
   `supported_efforts` keeps working by delegating (or both share a private
   resolver — prefer the private-resolver refactor if it's smaller than
   duplicating the lookup chain).
2. **Log it where the decision is consumed.** At the single choke point where
   the `/thinking` modal (and piped status) resolves levels —
   `supported_thinking_levels` callers in `effort.rs` + status path — emit one
   `log::debug!` line:
   `effort provenance: model=<id> levels=[…] source=<source> detail=<detail>`.
   Gate a user-visible variant behind `RUST_LOG=gray=debug` (no new flags, no
   UI changes; gray's logging already flows there — see `logging.rs`).
3. **Document the recipe.** One paragraph in the SPEC-03 fast-loop docs
   (or AGENTS.md troubleshooting): "levels look wrong → rerun with
   `RUST_LOG=gray=debug`, read the `effort provenance` line". The dsh
   equivalent of `--dump-config` for this subsystem.

## Files to touch
- `crates/gray/src/setup/context/providers.rs` — `EffortProvenance`,
  `EffortSource`, resolver, unit tests (exact beats suffix; suffix beats
  family; family beats catalog — pin the kilo/openrouter poisoning order
  explicitly as a regression test next to the `9740148` test).
- `crates/gray/src/setup/context/effort.rs` (verify exact filename on
  implementation — the `/thinking` modal path) — the `log::debug!` line.
- Piped `/thinking` status path (same file family as the `0067d7a` fix) —
  same line.
- `AGENTS.md` or fast-loop docs — the debug recipe paragraph.

## Acceptance
- `cargo test -p gray providers` green, including the poisoning-order test.
- Manual: with `RUST_LOG=gray=debug`, opening `/thinking` on a bare
  contributor id logs `source=FamilyTable` (or `Exact{…}` post-fetch), and
  the source that would previously have been `SuffixAlias{kilo}` is visible
  in the log instead of invisible in behavior.
- No user-visible output change at default log level; no signature changes
  to `supported_efforts` / `supported_thinking_levels`.
- `cargo fmt --check` clean.

## Non-goals
- No TUI/CLI surface (`--dump-config` clone is explicitly out).
- No change to resolution semantics — observability only.
- No provenance for other subsystems (provider routing, model pricing) —
  pattern may copy later, out of scope here.
