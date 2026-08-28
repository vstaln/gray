# Taste
- Prefers slash command palette to show ~8 commands at once instead of 2-3, with viewport sized to avoid clipping — favors information density in TUI menus. Confidence: 0.90
- Prefers concise, conversational command descriptions like "resume a previous conversation" over verbose technical syntax details like "(picker, --last, or <id>)". Confidence: 0.92
- Prefers thinking/status indicator at top of viewport above input box rather than below input near bottom edge — treats it as header not footer. Confidence: 0.88
- Prefers token usage summary footer (e.g., "• 3.1k tok · 56 think · 2.6k cached (84%)") rendered at the bottom after the assistant answer, not above/injected before streamed text — expects answer → tokens ordering. Confidence: 0.92
- Prefers Grok-style unified diff rendering for edit/write tool outputs (colored +/- lines with hunk headers and context) over terse "wrote ..." confirmations — expects visible before/after diffs like Grok. Confidence: 0.88
- Prefers Codex-exact status line styling: no bullet/dot prefix, bold white "Working"/"Thinking" label with dim gray timing suffix like "(12s • esc to interrupt)", Esc as primary interrupt key over ctrl-c, and shade-based (white/dim) styling over colored shimmer. Confidence: 0.92
