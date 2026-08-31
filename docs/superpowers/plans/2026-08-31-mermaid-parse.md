# Mermaid Parse Extract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract parse logic from `crates/gray-markdown/src/mermaid.rs` (3634 lines) into `mermaid/parse.rs` and simplify where layout breaks hide.

**Architecture:** Create `crates/gray-markdown/src/mermaid/` directory with `mod.rs` re-export. Pure cut-paste for Task 1; Task 2 deletes dead simplification. No new deps.

**Tech Stack:** Rust, ratatui (only for types, not parse), unicode-width

**Spec:** Keep `pub(crate) fn render` API stable; callers `crate::mermaid::render` unchanged.

## Global Constraints
- `cargo build -p gray-markdown` must pass after each task.
- No new dependencies.
- Preserve `pub(crate)` visibility via `pub use`.

---

### Task 1: Extract parse to mermaid/parse.rs

**Files:**
- Create: `crates/gray-markdown/src/mermaid/parse.rs`
- Create: `crates/gray-markdown/src/mermaid/mod.rs`
- Modify: `crates/gray-markdown/src/lib.rs` if needed for mod resolution

**Produces:** `mermaid::parse` exports `Graph`, `Node`, `Edge`, `Dir`, `Shape`, `Head`, `Group`, `parse_graph`, `parse_state`, `parse_class`, `parse_er`, `parse_sequence`, helpers `split_statements`, `clean_label`, `decode_html_entities`, `strip_html_tags` (pub(crate) where needed)

- [ ] **Step 1: git mv crates/gray-markdown/src/mermaid.rs crates/gray-markdown/src/mermaid/mod.rs ; mkdir -p crates/gray-markdown/src/mermaid**
- [ ] **Step 2: Create `mermaid/parse.rs` with lines ~97-1350 from old mermaid.rs** (Shape through parse_er helpers, plus Graph/Node defs). Keep imports `std::collections::HashMap`, `unicode_width` that parse needs. Remove ratatui imports from parse.
- [ ] **Step 3: In `mermaid/mod.rs` replace with `pub mod parse; pub use parse::{Graph, ...};` and keep render/layout/canvas inline for now.
- [ ] **Step 4: Verify** `cargo build -p gray-markdown 2>&1 | tail -n 10` PASS
- [ ] **Step 5: Commit** `git add crates/gray-markdown/src/mermaid/ && git commit -m "refactor: extract mermaid parse to mermaid/parse.rs"`

---

### Task 2: Simplify — delete over-flexible parse branches causing layout breaks

**Files:**
- Modify: `crates/gray-markdown/src/mermaid/parse.rs`

**Context:** Narrow-terminal breaks partly from parse accepting `MAX_NODES=128` + `MAX_GROUPS=24` then layout tries to fit them in 60cols → fallback box. Also `HTML_FORMAT_TAGS` 24 entries + `strip_markdown` double-passes are dead weight.

- [ ] **Step 1: Cap `MAX_NODES` 128→64 and `MAX_GROUPS` 24→12** (ponytail: lower ceiling, upgrade when real diagram needs it — comment `# ponytail: capped, raise if legit diagram hits it`)
- [ ] **Step 2: Delete `strip_markdown` backtick/strong pass if not covered by tests** — `clean_label` already does `decode_html_entities` once; keep single path. If tests fail, revert.
- [ ] **Step 3: Verify** `cargo test -p gray-markdown 2>&1 | tail -n 20` PASS ; `cargo build -p gray-markdown` PASS
- [ ] **Step 4: Commit** `git commit -m "refactor: simplify mermaid parse caps and dead markdown strip"`

---

## Self-Review
- No placeholders; Task 1 is cut-paste, Task 2 gated by tests.
- Re-exports keep external API stable.

