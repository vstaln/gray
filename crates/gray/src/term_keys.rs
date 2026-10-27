use std::io::stdout;

use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};

const MODIFY_OTHER_KEYS_ON: &str = "\x1b[>4;2m";
const MODIFY_OTHER_KEYS_OFF: &str = "\x1b[>4;0m";

pub fn push_sequence(flags: KeyboardEnhancementFlags) -> String {
    use crossterm::Command as _;

    let mut out = String::new();
    PushKeyboardEnhancementFlags(flags)
        .write_ansi(&mut out)
        .expect("write to String cannot fail");
    out
}

fn write_mode(seq: &str) {
    use std::io::Write as _;
    let mut out = stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
}

pub const DISABLE_ENV: &str = "GRAY_NO_KEYBOARD_ENHANCEMENT";

pub fn disabled() -> bool {
    matches!(
        std::env::var(DISABLE_ENV).ok().as_deref().map(str::trim),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

pub fn flags_for(
    term_program: Option<&str>,
    tmux_extended_keys_format: Option<&str>,
) -> KeyboardEnhancementFlags {
    let base = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS;
    let name = term_program.unwrap_or_default().to_ascii_lowercase();
    let leaks_release_events = name.contains("ghostty") || name.contains("iterm");
    let tmux_loses_shift_enter = tmux_extended_keys_format.is_some_and(|f| f == "xterm");
    if leaks_release_events || tmux_loses_shift_enter {
        base
    } else {
        base | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
    }
}

pub fn should_enable_modify_other_keys(
    in_tmux: bool,
    tmux_extended_keys_format: Option<&str>,
) -> bool {
    in_tmux && matches!(tmux_extended_keys_format, Some("csi-u"))
}

pub fn in_multiplexer() -> bool {
    std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some()
}

pub fn tmux_extended_keys_format() -> Option<String> {
    for args in [
        ["display-message", "-p", "#{extended-keys-format}"],
        ["show-options", "-gqv", "extended-keys-format"],
    ] {
        let output = std::process::Command::new("tmux")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            continue;
        }
        if let Some(value) = String::from_utf8(output.stdout)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return Some(value);
        }
    }
    None
}

#[derive(Debug)]
pub struct KeyboardEnhancementGuard {
    pushed: bool,
    modify_other_keys: bool,
}

impl KeyboardEnhancementGuard {
    pub fn push() -> Self {
        if disabled() {
            return Self {
                pushed: false,
                modify_other_keys: false,
            };
        }
        let in_tmux = in_multiplexer();
        let format = if in_tmux {
            tmux_extended_keys_format()
        } else {
            None
        };
        let flags = flags_for(
            std::env::var("TERM_PROGRAM").ok().as_deref(),
            format.as_deref(),
        );
        let modify_other_keys = should_enable_modify_other_keys(in_tmux, format.as_deref());
        let pushed = crossterm::execute!(stdout(), PushKeyboardEnhancementFlags(flags)).is_ok();
        if modify_other_keys {
            write_mode(MODIFY_OTHER_KEYS_ON);
        }
        Self {
            pushed,
            modify_other_keys,
        }
    }

    pub fn active(&self) -> bool {
        self.pushed
    }
}

impl Drop for KeyboardEnhancementGuard {
    fn drop(&mut self) {
        if self.modify_other_keys {
            write_mode(MODIFY_OTHER_KEYS_OFF);
        }
        if self.pushed {
            let _ = crossterm::execute!(stdout(), PopKeyboardEnhancementFlags);
        }
    }
}
