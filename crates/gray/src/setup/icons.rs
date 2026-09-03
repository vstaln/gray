//! Nerd Font icons with pure-ASCII fallback (ported from `reference/empryo/src/core/icons.ts`).
use std::sync::OnceLock;

static NERD_FONT: OnceLock<bool> = OnceLock::new();

fn env_override() -> Option<bool> {
    match std::env::var("GRAY_NERD_FONT").unwrap_or_default().trim().to_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn auto_detect() -> bool {
    let term = std::env::var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    if ["kitty", "wezterm", "alacritty", "hyper", "iterm", "ghostty"].iter().any(|t| term.contains(t)) {
        return true;
    }
    if std::env::var("TERMINAL_EMULATOR").unwrap_or_default().to_lowercase().contains("jetbrains") {
        return true;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    [format!("{home}/.fonts"), format!("{home}/.local/share/fonts"), "/usr/share/fonts".into()]
        .iter()
        .any(|d| std::path::Path::new(d).join("SymbolsNerdFont-Regular.ttf").exists())
}

/// Cached Nerd Font availability (env override wins, else auto-detect).
pub fn has_nerd_font() -> bool {
    *NERD_FONT.get_or_init(|| env_override().unwrap_or_else(auto_detect))
}

/// Override the cache (tests/startup); no-op if already initialized.
pub fn set_nerd_font(v: bool) {
    let _ = NERD_FONT.set(v);
}

/// Initialize cache from env/auto-detect (call once at startup).
pub fn init_nerd_font() {
    set_nerd_font(env_override().unwrap_or_else(auto_detect));
}

/// Icon by name: Nerd PUA glyphs when available, else pure ASCII.
pub fn icon(name: &str) -> &'static str {
    if has_nerd_font() {
        match name {
            "system" => "",     // \u{f013} cog
            "project" => "",    // \u{f07b} folder
            "tools" => "",      // \u{f0ad} wrench
            "skills" => "󰢪",    // \u{f08aa}
            "messages" => "",   // \u{f075} comment
            "context" => "",    // \u{f1c0} database
            "dot" => "●",
            "free" => "░",
            "reserve" => "✕",
            "arrow" => "›",
            "free_space" => "○",
            _ => "?",
        }
    } else {
        match name {
            "system" => "#",
            "project" => "P",
            "tools" => "T",
            "skills" => "*",
            "messages" => ">",
            "context" => "@",
            "dot" => ".",
            "free" => ".",
            "reserve" => "x",
            "arrow" => ">",
            "free_space" => "o",
            _ => "?",
        }
    }
}
