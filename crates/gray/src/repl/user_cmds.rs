//! Feedback + permissions slash-commands (split from `repl`).

use super::*;

#[derive(Debug, PartialEq)]
pub(crate) enum PermissionsAction {
    Show,
    Set(String),
}

pub(crate) fn parse_permissions_args(raw: &str) -> PermissionsAction {
    let mut toks = raw.split_whitespace().skip(1);
    match toks.next() {
        None => PermissionsAction::Show,
        Some(mode) => PermissionsAction::Set(mode.to_string()),
    }
}

pub(crate) fn handle_feedback(
    text: Option<String>,
    config: &Config,
    session_state: &Option<SessionState>,
    tui: Option<&crate::composer::SharedTui>,
) {
    let Some(raw) = text.filter(|t| !t.trim().is_empty()) else {
        say(tui, "usage: /feedback <what happened> — saves locally and opens a prefilled issue");
        return;
    };
    let title = crate::feedback::build_title(&raw);
    let version = env!("CARGO_PKG_VERSION");
    let os = format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH);
    let model = config.model.as_deref().unwrap_or("no model");
    let session = session_state.as_ref().map(|s| s.session_id.as_str()).unwrap_or("none");
    let body = crate::feedback::build_body(&raw, version, &os, model, session);
    let url = crate::feedback::issue_url(&title, &body);
    match crate::setup::gray_home().map(|h| h.join("feedback")) {
        Ok(dir) => match crate::feedback::save_feedback(&dir, &title, &body, &crate::feedback::timestamp()) {
            Ok(path) => {
                crate::feedback::open_in_browser(&url);
                say(tui, &format!("saved {}\nfile an issue: {url}", path.display()));
            }
            Err(e) => say(tui, &format!("could not save feedback ({e}) — file manually: {url}")),
        },
        Err(e) => say(tui, &format!("could not resolve gray home ({e}) — file manually: {url}")),
    }
}

pub(crate) fn handle_permissions(
    config: &mut Config,
    direct: Option<String>,
    gate: &gray_core::approvals::ApprovalGate,
    tui: Option<&crate::composer::SharedTui>,
) {
    use gray_core::approvals::{normalize_mode, permission_modes};
    fn persist(config: &Config) {
        if let Ok(path) = crate::setup::saved_config_path() {
            let mut saved = crate::setup::load_saved_config_at(&path);
            saved.permissions = config.permissions.clone();
            let _ = crate::setup::save_saved_config_at(&path, &saved);
        }
    }
    fn announce(mode: &str, tui: Option<&crate::composer::SharedTui>) {
        let (label, desc) = permission_modes()
            .into_iter()
            .find(|(id, _, _)| *id == mode)
            .map(|(_, l, d)| (l, d))
            .unwrap_or((mode, ""));
        if let Some(shared) = tui {
            let mut t = shared.lock().expect("tui lock");
            t.push_action("Permissions updated to", Some(label));
            t.push_dim(desc.to_string());
        } else {
            println!("✓ Permissions updated to {label}\n  {desc}");
        }
    }
    if let Some(raw) = direct {
        match normalize_mode(&raw) {
            Some(mode) => {
                config.permissions = Some(mode.to_string());
                gate.set_mode(mode);
                if let Some(shared) = tui {
                    shared.lock().expect("tui lock").set_permission_mode(mode.to_string());
                }
                persist(config);
                announce(mode, tui);
            }
            None => say(
                tui,
                &format!("unknown mode '{raw}' — use: read-only, auto, full"),
            ),
        }
        return;
    }
    let bg = tui.map(|s| s.lock().expect("tui lock").snapshot());
    let current = gate.mode();
    let picked = with_modal_sync(tui, || crate::setup::run_permissions_modal(&current, bg.as_ref()));
    match picked {
        Ok(Some(mode)) => {
            config.permissions = Some(mode.clone());
            gate.set_mode(&mode);
            if let Some(shared) = tui {
                shared.lock().expect("tui lock").set_permission_mode(mode.clone());
            }
            persist(config);
            announce(&mode, tui);
        }
        Ok(None) => {}
        Err(e) => say(tui, &format!("permissions error: {e}")),
    }
}

#[cfg(test)]
mod feedback_permissions_tests {
    use super::{parse_permissions_args, PermissionsAction};

    #[test]
    fn permissions_args_parse() {
        assert!(matches!(
            parse_permissions_args("/permissions"),
            PermissionsAction::Show
        ));
        match parse_permissions_args("/permissions full") {
            PermissionsAction::Set(m) => assert_eq!(m, "full"),
            _ => panic!("expected Set"),
        }
    }
}
