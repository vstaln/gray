use std::sync::OnceLock;

static NERD_FONT: OnceLock<bool> = OnceLock::new();

fn env_override() -> Option<bool> {
    match std::env::var("GRAY_NERD_FONT")
        .unwrap_or_default()
        .trim()
        .to_lowercase()
        .as_str()
    {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

pub fn has_nerd_font() -> bool {
    *NERD_FONT.get_or_init(|| env_override().unwrap_or(false))
}

pub fn icon(name: &str) -> &'static str {
    if has_nerd_font() {
        match name {
            "arrow" => "❯",
            _ => "?",
        }
    } else {
        match name {
            "arrow" => ">",
            _ => "?",
        }
    }
}
