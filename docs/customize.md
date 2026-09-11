# Make gray yours

Every axis below works in the real binary today — see the
`examples/hello-skill` and `examples/hello-plugin` proofs
(`gray plugin check`, `--dump-manifest`, skill discovery).

## Skills — teach gray a workflow

Drop a directory with a `SKILL.md` (frontmatter `description:` required)
into `~/.gray/skills` (global) or `.gray/skills` (project, walks up to
git root; also `~/.config/opencode/skills`, `~/.agents/skills`,
`~/.claude/skills`). Run it with `/skills:<name>`.
Loader: `crates/gray/src/skills/`. Copy me: `examples/hello-skill/SKILL.md`.

## Plugins — add tools and slash commands

Ship an executable sidecar answering `plugin/manifest` over stdio NDJSON
(wire spec: `docs/plugins.md`, schema: `docs/schema/manifest.v1.json`),
then register it in `gray.yml`: `- sidecar: /path/to/plugin.sh`.
Check it with `gray plugin check <dir>`; inspect boot with
`gray --dump-manifest`. Copy me: `examples/hello-plugin/plugin.sh`.

## Providers — point gray at any model

Any OpenAI-compatible endpoint works: `gray --base-url <url>` (or
`GRAY_BASE_URL`), `gray --model <provider/model>` (or `GRAY_MODEL`),
key via `GRAY_API_KEY`. `/connect` walks the same picker interactively;
`openai`/`xai` also offer browser OAuth. Resolution order (each wins over
the next): CLI flags > env > `~/.gray/config.json`. Code:
`crates/gray/src/config.rs`, `crates/gray/src/setup/catalog.rs`.

## Slash commands — fixed set, plugins extend it

The 16 built-ins (`/model`, `/skills`, `/plugin`, …) are fixed in
`crates/gray/src/repl/commands.rs`. New `/commands` come only from a
plugin manifest's `commands: ["/x"]` (routed via `command/run`).

## System prompt and pickers — tune behavior without code

Edit `~/.gray/AGENTS.md` directly (or `/agentsmd show|reset`); project
`AGENTS.md`/`CLAUDE.md` files append as context. `/model`, `/thinking`,
`/context` persist to `~/.gray/config.json`.

## Honest gaps (not customizable today)

Themes: syntax colors are a hardcoded Tokyo Night (`crates/gray-markdown`);
pi-gallery theme files install as skills-only placeholders (P3). Ad-hoc
user slash commands (no plugin): not supported. `capabilities` in the
manifest are advisory, not enforced.
