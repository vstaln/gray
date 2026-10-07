//! `/theme`, `/hotkeys` and `/reload`: user themes, keybindings and the
//! file-based customizations they share a reload path with.

use std::path::Path;

use super::say;
use crate::theme::{self, UiTheme};

/// What a `/theme` invocation decided: text for the user, plus the palette
/// to apply (`Some((theme, name))`, `name` `None` for the built-in one).
#[derive(Debug)]
pub(crate) struct ThemeOutcome {
    pub(crate) message: String,
    pub(crate) apply: Option<(UiTheme, Option<String>)>,
}

impl ThemeOutcome {
    fn say(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            apply: None,
        }
    }
}

fn persist(config_path: &Path, name: Option<&str>) -> Result<(), String> {
    let _lock = crate::setup::lock_saved_config_at(config_path).map_err(|e| format!("{e:#}"))?;
    let mut saved = crate::setup::load_saved_config_at(config_path);
    saved.theme = name.map(str::to_string);
    crate::setup::save_saved_config_at(config_path, &saved).map_err(|e| format!("{e:#}"))
}

fn with_warnings(mut msg: String, warnings: &[String]) -> String {
    for w in warnings {
        msg.push_str(&format!("\n  warning: {w}"));
    }
    msg
}

/// `/theme` core against explicit paths (test seam). `active` is the name
/// of the theme in use now (`None` = built-in).
pub(crate) fn theme_command(
    arg: Option<&str>,
    active: Option<&str>,
    themes_dir: &Path,
    config_path: &Path,
) -> ThemeOutcome {
    let arg = arg.map(str::trim).unwrap_or("");
    let (verb, rest) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
    let rest = rest.trim();
    match verb {
        "" | "list" => {
            let mut out = String::from("themes:");
            let mark = |on: bool| if on { "●" } else { " " };
            out.push_str(&format!(
                "\n  {} {} (built-in)",
                mark(active.is_none()),
                theme::BUILTIN_THEME_NAME
            ));
            let names = theme::list_themes_in(themes_dir);
            for n in names.iter().filter(|n| !theme::is_builtin_name(n)) {
                out.push_str(&format!("\n  {} {n}", mark(active == Some(n.as_str()))));
            }
            out.push_str(&format!(
                "\n/theme <name> to switch · /theme new <name> to start one in {}",
                themes_dir.display()
            ));
            ThemeOutcome::say(out)
        }
        "new" => {
            if rest.is_empty() {
                return ThemeOutcome::say("usage: /theme new <name>");
            }
            if !theme::valid_theme_name(rest) || theme::is_builtin_name(rest) {
                return ThemeOutcome::say(format!("`{rest}` can't be a theme name"));
            }
            let path = themes_dir.join(format!("{rest}.json"));
            if path.exists() {
                return ThemeOutcome::say(format!("{} already exists", path.display()));
            }
            let write = std::fs::create_dir_all(themes_dir)
                .and_then(|()| std::fs::write(&path, theme::theme_to_json(&theme::theme())));
            match write {
                Ok(()) => ThemeOutcome::say(format!(
                    "wrote {} (a copy of the current palette)\nedit it, then /theme {rest}",
                    path.display()
                )),
                Err(e) => ThemeOutcome::say(format!("could not write {}: {e}", path.display())),
            }
        }
        "reload" => match active {
            None => ThemeOutcome::say("the built-in palette has no file to reload"),
            Some(name) => match theme::load_theme_in(themes_dir, name) {
                Ok((t, warnings)) => ThemeOutcome {
                    message: with_warnings(format!("✓ reloaded theme {name}"), &warnings),
                    apply: Some((t, Some(name.to_string()))),
                },
                Err(e) => ThemeOutcome::say(format!("theme not reloaded: {e}")),
            },
        },
        name if !rest.is_empty() => ThemeOutcome::say(format!(
            "usage: /theme [<name> | new <name> | reload] (got `{name} {rest}`)"
        )),
        name if theme::is_builtin_name(name) => {
            let note = persist(config_path, None).err();
            ThemeOutcome {
                message: match note {
                    None => "✓ theme gray (built-in)".to_string(),
                    Some(e) => format!("✓ theme gray (built-in) — not saved: {e}"),
                },
                apply: Some((theme::GRAY_UI_THEME, None)),
            }
        }
        name => match theme::load_theme_in(themes_dir, name) {
            Ok((t, warnings)) => {
                let mut msg = format!("✓ theme {name}");
                if let Err(e) = persist(config_path, Some(name)) {
                    msg.push_str(&format!(" — not saved: {e}"));
                }
                ThemeOutcome {
                    message: with_warnings(msg, &warnings),
                    apply: Some((t, Some(name.to_string()))),
                }
            }
            Err(e) => ThemeOutcome::say(format!("theme not applied: {e}")),
        },
    }
}

pub(crate) fn handle_theme(arg: Option<String>, tui: Option<&crate::composer::SharedTui>) {
    let (dir, config_path) = match (theme::themes_dir(), crate::setup::saved_config_path()) {
        (Ok(d), Ok(c)) => (d, c),
        (Err(e), _) | (_, Err(e)) => {
            say(tui, &format!("could not resolve gray home: {e}"));
            return;
        }
    };
    let active = theme::active_theme_name();
    let outcome = theme_command(arg.as_deref(), active.as_deref(), &dir, &config_path);
    if let Some((t, name)) = outcome.apply {
        theme::set_theme(t, name.as_deref());
    }
    say(tui, &outcome.message);
    if let Some(shared) = tui {
        let _ = shared.lock().expect("tui lock").draw();
    }
}

/// `/hotkeys` core against an explicit gray home (test seam). Returns the
/// text to show and, for `reload`, the keymap to activate.
pub(crate) fn keys_command(
    arg: Option<&str>,
    gray_home: &Path,
    active: &crate::keymap::Keymap,
) -> (String, Option<crate::keymap::Keymap>) {
    let arg = arg.map(str::trim).unwrap_or("");
    match arg {
        "" | "list" => (render_keymap(active, gray_home), None),
        "reload" => {
            let km = crate::keymap::load_in(gray_home);
            let msg = with_warnings(
                format!(
                    "✓ reloaded keybindings ({} override{})",
                    km.overridden.len(),
                    if km.overridden.len() == 1 { "" } else { "s" }
                ),
                &km.warnings,
            );
            (msg, Some(km))
        }
        other => (format!("usage: /hotkeys [reload] (got `{other}`)"), None),
    }
}

fn render_keymap(km: &crate::keymap::Keymap, gray_home: &Path) -> String {
    let path = crate::keymap::path_in(gray_home);
    let mut out = String::from("keybindings:");
    for &a in crate::keymap::ALL {
        let keys: Vec<String> = km.keys_for(a).iter().map(|c| c.display()).collect();
        let keys = if keys.is_empty() {
            "—".to_string()
        } else {
            keys.join(", ")
        };
        let mark = if km.overridden.iter().any(|o| o == a.id()) {
            "*"
        } else {
            " "
        };
        out.push_str(&format!(
            "\n {mark} {:<22} {:<30} {}",
            keys,
            a.id(),
            a.describe()
        ));
    }
    if !km.commands().is_empty() {
        out.push_str("\ncommands:");
        for (chord, cmd) in km.commands() {
            out.push_str(&format!("\n * {:<22} {cmd}", chord.display()));
        }
    }
    out.push_str(&format!(
        "\n\n* = set in {} (pi's ids and key syntax; `\"/cmd\": \"ctrl+x\"` binds a command; /hotkeys reload applies edits)",
        path.display()
    ));
    with_warnings(out, &km.warnings)
}

pub(crate) fn handle_keys(arg: Option<String>, tui: Option<&crate::composer::SharedTui>) {
    let home = match crate::setup::gray_home() {
        Ok(h) => h,
        Err(e) => {
            say(tui, &format!("could not resolve gray home: {e}"));
            return;
        }
    };
    let (msg, km) = keys_command(arg.as_deref(), &home, &crate::keymap::active());
    if let Some(km) = km {
        crate::keymap::set_active(km);
    }
    say(tui, &msg);
}

/// `/reload`: re-reads every file-based customization in place — the
/// active theme, `keybindings.json`, and prompt templates. Plugins keep
/// running (`/plugin` manages those).
pub(crate) fn handle_reload(tui: Option<&crate::composer::SharedTui>) {
    let mut lines = Vec::new();
    match theme::active_theme_name() {
        Some(_) => handle_theme_quiet(Some("reload"), &mut lines),
        None => lines.push("theme: built-in gray".to_string()),
    }
    match crate::setup::gray_home() {
        Ok(home) => {
            let (msg, km) = keys_command(Some("reload"), &home, &crate::keymap::active());
            if let Some(km) = km {
                crate::keymap::set_active(km);
            }
            lines.push(msg);
        }
        Err(e) => lines.push(format!("keybindings not reloaded: {e}")),
    }
    crate::prompt_templates::invalidate_cache();
    lines.push("✓ prompt templates re-read on next use".to_string());
    say(tui, &lines.join("\n"));
    if let Some(shared) = tui {
        let _ = shared.lock().expect("tui lock").draw();
    }
}

fn handle_theme_quiet(arg: Option<&str>, lines: &mut Vec<String>) {
    let (dir, config_path) = match (theme::themes_dir(), crate::setup::saved_config_path()) {
        (Ok(d), Ok(c)) => (d, c),
        (Err(e), _) | (_, Err(e)) => {
            lines.push(format!("theme not reloaded: {e}"));
            return;
        }
    };
    let active = theme::active_theme_name();
    let outcome = theme_command(arg, active.as_deref(), &dir, &config_path);
    if let Some((t, name)) = outcome.apply {
        theme::set_theme(t, name.as_deref());
    }
    lines.push(outcome.message);
}

#[path = "customize_tests.rs"]
#[cfg(test)]
mod tests;
