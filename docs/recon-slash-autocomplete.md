# Recon: slash-command autocomplete (pi, oh-my-pi, codex)

Date: 2026-02. Sources read: pi-mono `packages/coding-agent` + `packages/tui`, can1357/oh-my-pi, openai/codex `codex-rs/tui/src/bottom_pane/`.

## 1. pi (badlogic/pi-mono, TypeScript)

**Registry** — `packages/coding-agent/src/core/slash-commands.ts`:

```ts
export interface BuiltinSlashCommand {
	name: string;
	description: string;
	argumentHint?: string;
}
export const BUILTIN_SLASH_COMMANDS: ReadonlyArray<BuiltinSlashCommand> = [
	{ name: "model", description: "Select model (opens selector UI)", argumentHint: "<provider/model>" },
	// ... ~23 builtins; extensions/prompts/skills add SlashCommandInfo{name, description?, source}
];
```

**Autocomplete engine** — `packages/tui/src/autocomplete.ts` (shared TUI lib):

- a) **Trigger**: line starts with `/`, only at start of input (`beforePrefix.trim() === ""`), no second `/` in token — autocomplete.ts:308,393:
```ts
if (!options.force && textBeforeCursor.startsWith("/")) { ... }
const isSlashCommand = prefix.startsWith("/") && beforePrefix.trim() === ""
    && !prefix.slice(1).includes("/");
```
- b) **Row rendering**: popup drawn by the TUI editor above the input; rows = name + description.
- c) **Filtering**: fuzzy over command name:
```ts
const filtered = fuzzyFilter(commandItems, prefix, (item) => item.name).map(...)
```
- d) **Selection keys**: full-screen TUI (raw mode): arrows navigate, Tab/Enter accept, Escape closes (`custom-editor.ts`: "Escape/interrupt - only if autocomplete is NOT active").
- e) **Unknown command**: not verified in this pass (INCONCLUSIVE) — registry lookup happens at execution time in interactive-mode.

## 2. oh-my-pi (can1357 fork)

Important negative result (PROVEN): the Rust crates are a red herring for this question — `crates/pi-builtins/src/*.rs` are shell builtins for omp's embedded shell; the `slash` grep hits there were false positives (`pi-builtins/src/complete.rs` is bash-style `complete` for that shell, unrelated to slash commands). The slash UI lives in TypeScript, same architecture as pi.

**Registry** — `packages/coding-agent/src/slash-commands/builtin-registry.ts` (superset of pi):

```ts
export interface TuiBuiltinSlashCommand extends BuiltinSlashCommand {
	getArgumentCompletions?: (prefix: string) => AutocompleteItem[] | null | Promise<...>;
	getInlineHint?: (argumentText: string) => string | null;
	getAutocompleteDescription?: () => string | undefined;
}
const BUILTIN_SLASH_COMMAND_REGISTRY = [...modes, collab, session, lifecycle, marketplace, control];
// aliases registered in lookup Map alongside primary names
```

- Trigger/filter in `packages/tui/src/autocomplete.ts`: same `startsWith("/")` gate (:70), but adds scored matching (exact prefix → 900, substring → 80), a `skill:` namespace with staleness guards around Tab/Enter acceptance (:432+), and argument-level completion after the command word.
- Keys/rendering: same raw-mode TUI as pi.

## 3. codex (openai/codex, Rust ratatui)

Files: `codex-rs/tui/src/bottom_pane/{command_popup.rs, slash_commands.rs, selection_popup_common.rs}` (~1900 lines total).

**State + filtering** — command_popup.rs:

```rust
pub(crate) struct CommandPopup {
    command_filter: String,
    commands: Vec<CommandItem>,
    state: ScrollState,
}
let commands = commands_for_input(flags.into(), &service_tier_commands)
    .into_iter()
    .filter_map(|command| match command {
        SlashCommandItem::Builtin(cmd) => (!cmd.command().starts_with("debug") ...
```

- a) **Trigger**: composer detects `/` prefix and opens the popup with the current token as `command_filter` (composer-side wiring in chat_composer; not fully traced — CHECKED partially).
- b) **Row rendering**: `GenericDisplayRow` via `render_rows_with_col_width_mode`, capped at `MAX_POPUP_ROWS` (popup_consts.rs), auto-sized name column; alias commands hidden so each action appears once:
```rust
const ALIAS_COMMANDS: &[SlashCommand] = &[SlashCommand::Quit, SlashCommand::Btw];
```
- c) **Filtering**: prefix on `command_filter` against available commands (`commands_for_input` also applies feature flags).
- d) **Selection keys**: arrows + Enter through `ScrollState` (ratatui raw mode); exact keymap in selection_popup_common.rs not fully traced (CHECKED partially).
- e) **Unknown command**: not verified here (INCONCLUSIVE).

## Synthesis for gray (≤15 lines)

gray = Rust workspace, std::io line-based REPL, no raw mode. Commands = `{name, description}`.
All three CLIs autocomplete live-as-you-type because they own the terminal (raw mode / ratatui).
Gray cannot: stdin is line-buffered, nothing arrives until Enter. So:

- **"/mod" + Enter**: filter registry by prefix. Exactly one match → expand and run it. Multiple → print matches above the prompt, run nothing ("did you mean: /mode /moderate"). Zero or exact-match-only cases fall out naturally. This is the whole feature: ~15 lines, no second read.
- **Tab completion**: skip hand-rolling a two-read scheme (read matches → prompt "n>" → read again); it's clunky and reinvents readline. If Tab is ever wanted, add `rustyline` with a small `Completer` impl — it owns raw mode so live popup + Tab both come free. Until then YAGNI.
- **Rendering position**: print the match list as ordinary lines *above* the next prompt; no ANSI cursor math needed in line mode. Show `name  description` padded to the longest name.
- **Skip from pi/omp**: fuzzy scoring, skill namespaces, argument-hint sub-completion, inline hints, alias maps — all serve registries of 50+ commands gray doesn't have.
- **Skip from codex**: scroll state, column auto-width, flag-gated availability — popup machinery that exists only because of raw mode.
- Keep from all three: prefix-at-start-of-input trigger rule, description shown per row, unknown-prefix → show near-matches instead of executing.
