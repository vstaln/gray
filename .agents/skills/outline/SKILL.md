---
name: outline
description: >
  Cheapest way to learn a large file's shape: prints signatures, type
  declarations and doc comments only — bodies elided, line numbers kept so a
  precise range can be read next. Measured on this repo at ~19% of the bytes
  (a 975-line, 38 kB file comes out 9 kB), so it is the difference between
  reading a module and skimming one. Use when a file is large and you only
  need to navigate it: finding a function, checking whether a type or command
  exists, seeing a module's public surface, or deciding which range to read
  before editing. Especially worth it right after a failed read that
  truncated, or before touching a file you have never opened. Line-based, not
  a parse tree — it can miss a multi-line signature and can match a keyword
  inside a string literal, so read the range for anything exact.
---

# outline

Print a file's skeleton, then read only the range you need.

```bash
bash .agents/skills/outline/bin/outline.sh <file> [more files...]
```

`${SKILL_DIR}/bin/outline.sh` resolves too (braces required — a bare
`$SKILL_DIR` is not substituted), so the path works from any cwd.

## What you get

Every signature, type declaration and doc comment, with the line number it
came from, and nothing else. Comment lines ride with the signature that
follows them; a block of TODOs in the middle of a function never prints.

```text
=== crates/gray/src/plugin_cli.rs (975 lines) ===
     1  //! Native plugin command registration, help metadata and bounded UI requests.
    13  struct Catalog {
    33  fn catalog(name: &str) -> anyhow::Result<&'static Catalog> {
    63  pub fn home() -> anyhow::Result<PathBuf> {
```

## Then read the range

The line numbers exist so the next call is cheap. Page to the exact block
rather than re-reading the file:

```bash
sed -n '33,60p' crates/gray/src/plugin_cli.rs
```

## When not to use it

- You already know the line range — just read it.
- The file is small (under ~150 lines): reading it whole is cheaper than the
  two calls.
- You need exact semantics — a multi-line signature, a macro body, a trait
  bound. It is line-based, not a parse tree, and it can match a keyword
  inside a string literal.

Multiple files in one call is the intended use for "what does this crate
expose": `outline.sh crates/gray/src/repl/*.rs`.
