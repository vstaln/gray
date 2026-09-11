//! Feedback slash-command (split from `repl`).

use super::*;

pub(crate) fn handle_feedback(
    text: Option<String>,
    config: &Config,
    session_state: &Option<SessionState>,
    tui: Option<&crate::composer::SharedTui>,
) {
    let Some(raw) = text.filter(|t| !t.trim().is_empty()) else {
        say(
            tui,
            "usage: /feedback <what happened> — saves locally and opens a prefilled issue",
        );
        return;
    };
    let title = crate::feedback::build_title(&raw);
    let version = env!("CARGO_PKG_VERSION");
    let os = format!("{} / {}", std::env::consts::OS, std::env::consts::ARCH);
    let model = config.model.as_deref().unwrap_or("no model");
    let session = session_state
        .as_ref()
        .map(|s| s.session_id.as_str())
        .unwrap_or("none");
    let terminal = crate::feedback::terminal_label(
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    );
    let shell = crate::feedback::shell_label(std::env::var("SHELL").ok().as_deref());
    let body = crate::feedback::build_body(&raw, version, &os, model, session, &terminal, &shell);
    let url = crate::feedback::issue_url(&title, &body);
    match crate::setup::gray_home().map(|h| h.join("feedback")) {
        Ok(dir) => {
            match crate::feedback::save_feedback(&dir, &title, &body, &crate::feedback::timestamp())
            {
                Ok(path) => {
                    crate::feedback::open_in_browser(&url);
                    say(
                        tui,
                        &format!("saved {}\nfile an issue: {url}", path.display()),
                    );
                }
                Err(e) => say(
                    tui,
                    &format!("could not save feedback ({e}) — file manually: {url}"),
                ),
            }
        }
        Err(e) => say(
            tui,
            &format!("could not resolve gray home ({e}) — file manually: {url}"),
        ),
    }
}
