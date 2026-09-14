# SPEC-05 — Install any skill-bearing GitHub repo by pasted URL + parse folded skill frontmatter

## Problem
Ponytail ([repo](https://github.com/DietrichGebert/ponytail)) documents ~20
supported harnesses and gray is not one of them. Two independent gaps, both
general (ponytail is only the witness case):

1. **Pasted repo URLs don't install.** `gray plugin install <spec>`
   (`crates/gray-pkg/src/ops.rs::parse_spec`) routes an `https://` URL to
   the git arm only with a `.git` path suffix. A bare repo page URL —
   `https://github.com/DietrichGebert/ponytail`, i.e. exactly what a user
   copies from the browser bar — falls into the tarball arm, which tries to
   unpack the GitHub HTML page as `.tar.gz` and fails. Working forms today
   (`git:https://…`, `….git`, `npm:@dietrichgebert/ponytail`) are all
   non-obvious spellings of the same intent.
2. **Folded skill descriptions parse as garbage.** Gray's frontmatter parser
   (`crates/gray/src/skills/load.rs::parse_yaml_like`) reads `description:`
   as one line. Every skill using the standard multi-line YAML style
   (`description: >` + indented continuation lines — all six ponytail
   skills, and the common style generally) loads with the literal
   description `">"`, silently breaking the `<available_skills>` discovery
   text. Install-time validation can't catch it: `">"` is non-empty.

## Design
1. **Bare `github.com/<owner>/<repo>` URLs are git sources.**
   `parse_spec`: an `http(s)` URL routes to `parse_git_spec` when
   `https_has_git_suffix` (unchanged) OR the new `https_is_bare_github_repo`
   matches. Matcher rules (`crates/gray-pkg/src/ops.rs`):
   - host (case-insensitive) is exactly `github.com`;
   - after R16 `@ref`-stripping and `?`/`#`-stripping, the path is exactly
     two segments (`<owner>/<repo>`), trailing slash tolerated, optional
     `.git` suffix tolerated, both segments non-empty.
   - Deeper paths (release tarballs, `/tree/…`, `/blob/…`) and non-github
     hosts stay tarball `Url`. No other routing changes.
   - Downstream (`install_git` → `extract_pi_skills` → lock write) is
     untouched: shallow clone with the honest unverified warning, only `.md`
     skill files taken under `~/.gray/plugins/pi/<pkg>/` (package code is
     never executed), `plugins/pi/<pkg>/` is already a discovery root.
2. **Block-scalar frontmatter.** `parse_yaml_like`: a value that is exactly
   `>`/`>-`/`>+` (folded, joined with spaces) or `|`/`|-`/`+` (literal,
   joined with newlines) gathers the following indented lines; blank lines
   skipped, stops at the first non-indented line. Chomping nuances ignored
   (descriptions are trimmed downstream). Quote-stripping and all other key
   handling unchanged.
3. **Docs.** One install paragraph in `docs/customize.md` (Skills section,
   ponytail as the example, npm form preferred / bare-URL form unverified),
   one `CHANGELOG.md` bullet under `[Unreleased]` → `Fixed`.

## Files to touch
- `crates/gray-pkg/src/ops.rs` — `parse_spec` condition +
  `https_is_bare_github_repo` + regression cases in
  `spec_parses_git_forms` (bare URL, trailing slash, `.git`, host case,
  release/page deeper paths stay `Url`, non-github stays `Url`).
- `crates/gray/src/skills/load.rs` — block-scalar gathering in
  `parse_yaml_like` + `folded_description_joins_continuation_lines` and
  `literal_and_chomped_markers_parse` unit tests.
- `docs/customize.md`, `CHANGELOG.md` — as above.

## Acceptance
- `cargo test -p gray-pkg --lib` green (incl. the new `parse_spec` cases).
- `cargo test -p gray --lib skills::` green (incl. the 2 new parser tests).
- `cargo fmt --check` clean.
- Live (scratch `GRAY_HOME`, real network): `gray plugin install
  npm:@dietrichgebert/ponytail` → 6 skills taken, verified;
  `gray plugin install https://github.com/DietrichGebert/ponytail` →
  same 6 skills under `plugins/pi/ponytail/`, unverified warning shown;
  all six load through the real loader with full (>100-char, non-`">"`
  ) descriptions.
- Out of scope for this spec (documented, not implemented): `/skills
  <name> <level>` arg validation for plugin-installed skills (upstream
  declares levels via hooks; gray validates `args:`), top-level
  `/ponytail*` commands (built-in set is fixed; plugins extend), always-on
  lifecycle injection (no gray equivalent; `AGENTS.md` fallback covers it).

## Non-goals
- No change to the `skills_ops` (`~/.gray/skills/`, single-bundle) path —
  multi-skill repos install via the plugin path; a mid-flight attempt to
  teach `skills_ops` multi-bundle installs was reverted (broke the build,
  API churn with no caller).
- No execution of package code (hooks, extensions, scripts) at install or
  runtime — gray takes `.md` skill files only.
- No Gray Index entry for ponytail; no per-repo special-casing anywhere.
