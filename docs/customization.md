# Customization

Everything here is plain files under `~/.gray` or the project, no code. Plugins cover the rest (see [plugins.md](plugins.md)).

## User themes

`~/.gray/themes/<name>.json`, picked with `/theme <name>` from the
`themes` plugin
(saved as `"theme"` in `config.json`; `GRAY_THEME` overrides per run).
Gray ships one palette and no other built-ins; a theme overrides
only the roles it names (the `UiTheme` roles in `crates/gray/src/theme.rs`).
Colors: `#rrggbb`, `#rgb`, ANSI names, 0-255 indexes, `default` (the
terminal's own color), or names from a `vars` map. `/theme new <name>`
writes the current palette as a starting point; `/theme reload` re-reads
the active file. Lines already in the transcript keep the colors they
were drawn with.

## Prompt templates

`prompts/*.md` become `/name`. Pi's placeholder
syntax (`$1`, `$@`, `$ARGUMENTS`, `${1:-default}`, `${@:2}`, `${@:2:1}`);
a body without placeholders gets the arguments appended. Searched:
project `.gray/prompts`, `.pi/prompts`, `.claude/commands` (cwd up to
the git root), then `~/.gray/prompts`, `~/.pi/agent/prompts`,
`~/.claude/commands`. First name wins; built-ins always win; a template
shadows a plugin command of the same name.

## Keybindings

`~/.gray/keybindings.json` uses pi's file format and
ids (`tui.editor.cursorWordLeft`, `tui.input.submit`, `app.interrupt`,
`app.exit`, …), so a pi `keybindings.json` mostly works as is: a value
(string or list) replaces that action's defaults, `[]` unbinds, pi ids
gray doesn't implement are skipped quietly. Gray adds slash-command
bindings: `"/compact": "ctrl+shift+k"`. Ctrl+C always keeps clearing /
cancelling. Every editor and mid-turn key goes through one table
(`crates/gray/src/keymap.rs`); `/hotkeys` (from the `quality-of-life`
plugin) lists the live bindings, and `/reload` re-reads the file.

## `/reload`

Re-reads the theme, keybindings and prompt templates
without restarting.

## Status line

`"status_line"` in `config.json` lays out the footer
as two segment lists:

```json
"status_line": {
  "left":  ["context", "cache", "timer", "work", "⎇ {branch}"],
  "right": ["command", "model", "effort"],
  "separator": " · ",
  "command": "~/bin/gray-status",
  "interval_ms": 5000
}
```

Segments: `context`, `cache`, `timer`, `work`, `model`, `effort`,
`cwd`, `dir`, `branch` and `command`. `{name}` inside a string is a template; anything else is
literal text. Empty segments drop out with their separator. `command`
runs through `sh -c` every `interval_ms` (floor 500ms, 3s timeout) with
a JSON snapshot on stdin (`cwd`, `workspace.current_dir`, `model.id`,
`model.display_name`, `context`, `branch`, …, the Claude Code statusline
fields); its first stdout line is the `command` segment. `/reload` picks
up edits.

## System prompt

`~/.gray/AGENTS.md` is the editable
system prompt, every `AGENTS.md`/`CLAUDE.md` at or above cwd
is appended per turn.
