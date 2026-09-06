# P2 Pi Gallery — design (AMENDED)

Status: amended in-session 2026-09-06 (design consolidated in this session;
other agent closed — this session's locked decisions are source of truth).
Depends on P1 Tasks 4 (`LockEntry.enabled`, `set_enabled`,
boot skip) and 5 (`plugins/official.json` with `scope`, gateway sidecar).
Do not implement before P1 lands.

Resolved 2026-09-06 (this session; user's original vote stands — mirror pi):
state lives on `LockEntry` (`enabled` + `scope`); user scope in
`~/.gray/plugins/lock.json`; project scope in NEW `.gray/plugins.json`
mirroring pi semantics (project entry wins unless `autoload:false`);
`gray.yml` unchanged (builtins + explicit sidecars only). Project installs
gated on project trust. Copy rule (from P1 plan): pi source labeled exactly
`Pi Gallery (preview)`; gray source labeled `Gray Index`.

## 1. Goal

Make pi's package universe (`pi.dev/packages`) installable into gray with
`plugin search` / `plugin install`, results labeled per the copy rule,
without adding any new user-global state file (one project-local file
mirrors pi — see header).

## 2. Key scoping fact (load-bearing)

Pi extensions are **TypeScript modules for pi's runtime**
(`pi.registerCommand`, `ctx.ui.notify`, …). Gray sidecars are
**executables speaking NDJSON** (`plugin/manifest`, `command/run`, …).
A pi `extension`/`theme` cannot execute under gray *in P2* — the runtime
bridge is approved as P3 (node-bridge sidecar, separate plan), and nothing
here closes that door.

What gray *can* consume from a pi package in P2: **skills** (Agent Skills,
`SKILL.md` — runtime-agnostic markdown) and **prompt templates**.
Gray's skills discovery already scans `.pi/skills` project dirs, so
installed pi skills light up with no discovery change.

P2 = skills (+ prompt templates if layout allows) from pi packages.
Extensions/themes are listed in search results only when a package also
ships skills, and the installer says so honestly.

## 3. Sources model

- **Gray Index** (default, `GRAY_PLUGIN_INDEX` override, ETag cache —
  `gray-pkg/src/index.rs` as today): sidecars + gray skills.
- **Pi Gallery (preview)**: second source for `search` only at first;
  `install` accepts pi specs (below). Name collisions resolve to Gray
  Index; pi hits always carry the `(preview)` label.

## 4. Install specs

Extend `NameOrUrl` (`gray-pkg/src/ops.rs:40`) with pi's two forms:

- `npm:<pkg>[@<version>]` — resolve via npm registry metadata
  (`https://registry.npmjs.org/<pkg>`), download the tarball URL from
  `dist.tarball`, verify against `dist.shasum`/`integrity`.
- `git:<url>[@<ref>]` — shallow clone (`--depth 1`, ref or default
  branch). Phase (b); npm-first because tarball hashes give us
  verification `git` cannot.

Install = extract `<pkg>/skills/*` (+ `templates/*` if present) into the
user skills dir (global) or `.gray/skills` (project, after trust —
`docs/plugins.md` trust note already covers this), record a lock entry
with `ecosystem: "pi-gallery"` (display label `Pi Gallery (preview)`
applied at render time, never stored in the `source` URL field),
`enabled` default true (Task 4 serde default), hash = npm integrity
(or commit sha for git).

`gray-pkg` already has HTTPS download + tar.gz unpack (`fetch.rs`);
npm metadata is one more JSON GET on the same client. No new deps.

## 5. Search

`/plugin search <q>` fans out to Gray Index (local ETag cache) + pi
source, merges with source labels. Gray Index miss message contract
(`not in index: …`) is unchanged; pi-side failures degrade to
`Pi Gallery (preview): unreachable` — never a hard error, search is
advisory.

## 6. Trust

Mirror pi: global installs run no code (skills are prompt-time text —
lowest risk class); project-local installs require the existing trust
flow. Lock records the verified hash; `update` re-verifies. Extensions
inside an installed pi package are never executed in P2 — the installer
prints which skills were taken and which extension/theme content was
skipped.

## 7. Open questions (resolve before P2 plan)

1. **Search transport**: does pi.dev expose a JSON search API, or do we
   query the npm registry (`/-/v1/search`) and filter for pi manifests?
   Needs one probe (no build): fetch both endpoints, compare recall for
   3 known packages.
2. **Skill layout variance**: do all pi skills follow `<name>/SKILL.md`?
   Sample 5 packages' tarballs; if layouts vary, ship a 3-pattern probe
   (`skills/*/SKILL.md`, `*/SKILL.md`, manifest-declared paths).
3. **Templates**: include prompt templates in P2, or skills-only first?
   Recommendation: skills-only; templates ride along only if they live
   under the same extracted tree at zero extra code.
