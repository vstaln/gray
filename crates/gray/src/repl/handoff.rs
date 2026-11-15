use super::*;
use crate::session_store::real_user_text;
use gray_core::message::Role;

const REQUEST_CHARS: usize = 400;
const COLD_HINT_TOKENS: usize = 40_000;
const REPLY_CHARS: usize = 1_200;
const TITLE_CHARS: usize = 60;

pub(crate) async fn start(
    config: &Config,
    cwd: &Path,
    rest: Option<String>,
    agent: &Option<Agent>,
    session_state: &mut Option<SessionState>,
    tui: Option<&crate::composer::SharedTui>,
    pending_command: &mut Option<ReplCommand>,
) {
    let Some(root) = default_root() else {
        say(tui, "nothing to continue");
        return;
    };
    let store = JsonlSessionStore::new(root);
    let (source, extra) = split_source(&store, rest).await;

    if let Some(id) = source {
        match build(&store, &id).await {
            Some(text) => *pending_command = Some(ReplCommand::New(Some(with_extra(text, extra)))),
            None => say(tui, "nothing to continue"),
        }
        return;
    }

    let has_messages = agent.as_ref().is_some_and(|a| !a.messages().is_empty());
    if has_messages {
        let cold = tui.is_some_and(|s| {
            let mut t = s.lock().expect("tui lock");
            t.cache_is_cold() && t.context_tokens() >= COLD_HINT_TOKENS
        });
        if cold {
            super::session::ensure_session_state(session_state, config, cwd).await;
            let current = session_state.as_ref().map(|s| s.session_id.clone());
            let text = match current {
                Some(id) => build(&store, &id).await,
                None => None,
            };
            if let Some(text) = text {
                *pending_command = Some(ReplCommand::New(Some(with_extra(text, extra))));
                return;
            }
            let text = extra.unwrap_or_else(|| CONTINUE_PROMPT.to_string());
            if !confirm_in_place(tui, &text) {
                return;
            }
            echo_prompt(tui, &text);
            *pending_command = Some(ReplCommand::Prompt(text));
            return;
        }
        let text = extra.unwrap_or_else(|| CONTINUE_PROMPT.to_string());
        echo_prompt(tui, &text);
        *pending_command = Some(ReplCommand::Prompt(text));
        return;
    }

    let current = session_state.as_ref().map(|s| s.session_id.clone());
    let source = match store.recall_validated(cwd).await {
        Some(id) => Some(id),
        None => crate::resume::recent_summaries(&store, false)
            .await
            .into_iter()
            .find(|s| Some(&s.id) != current.as_ref())
            .map(|s| s.id),
    };
    let Some(id) = source else {
        say(tui, "nothing to continue");
        return;
    };
    match build(&store, &id).await {
        Some(text) => *pending_command = Some(ReplCommand::New(Some(with_extra(text, extra)))),
        None => say(tui, "nothing to continue"),
    }
}

async fn split_source(
    store: &JsonlSessionStore,
    rest: Option<String>,
) -> (Option<SessionId>, Option<String>) {
    let Some(rest) = rest else {
        return (None, None);
    };
    let rest = rest.trim();
    if rest.is_empty() {
        return (None, None);
    }
    let (token, tail) = match rest.split_once(char::is_whitespace) {
        Some((t, r)) => (t, r.trim()),
        None => (rest, ""),
    };
    let tail = (!tail.is_empty()).then(|| tail.to_string());
    let exact = SessionId::new(token.to_lowercase());
    if store.load(&exact).await.is_ok() {
        return (Some(exact), tail);
    }
    if token.chars().count() >= 8
        && token.contains('-')
        && let Some(id) = crate::resume::resolve_prefix(store, token, true).await
    {
        return (Some(id), tail);
    }
    (None, Some(rest.to_string()))
}

pub(crate) async fn build(store: &JsonlSessionStore, id: &SessionId) -> Option<String> {
    let (_, entries) = store.load(id).await.ok()?;
    let start = match entries.iter().rposition(|e| e.compaction_boundary) {
        Some(i) if i + 1 < entries.len() => i + 1,
        _ => 0,
    };
    let active = &entries[start..];
    let first_user = active.iter().find_map(|e| real_user_text(&e.message));
    let last_user = active.iter().rev().find_map(|e| real_user_text(&e.message));
    let last_reply = active.iter().rev().find_map(|e| {
        if e.message.role != Role::Assistant {
            return None;
        }
        let text = e.message.text_content().trim().to_string();
        (!text.is_empty()).then_some((e.entry_id, text))
    });

    let title = crate::session_store::read_title(store.root_dir(), id.as_str())
        .or_else(|| first_user.as_deref().map(short_title));

    let mut out = match &title {
        Some(t) => format!("Continue from the \"{t}\" chat ({}).", id.as_str()),
        None => format!("Continue from chat {}.", id.as_str()),
    };
    let mut quoted_something = false;
    if let Some(text) = last_user {
        let (head, cut) = head_slice(&text, REQUEST_CHARS);
        out.push_str("\n\nLast request:\n");
        out.push_str(&quoted(&head, false, cut));
        quoted_something = true;
    }
    if let Some((entry_id, text)) = last_reply {
        let (tail, cut) = tail_slice(&text, REPLY_CHARS);
        out.push_str("\n\nWhere it left off (end of the last reply):\n");
        out.push_str(&quoted(&tail, cut, false));
        out.push_str(&format!(
            "\n\nEarlier turns: recall id=\"{}:{entry_id}\" (around=N for more).",
            id.as_str()
        ));
        quoted_something = true;
    }
    quoted_something.then_some(out)
}

fn short_title(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(TITLE_CHARS)
        .collect()
}

fn head_slice(text: &str, max: usize) -> (String, bool) {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    (head, chars.next().is_some())
}

fn tail_slice(text: &str, max: usize) -> (String, bool) {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return (text.to_string(), false);
    }
    let mut start = chars.len() - max;
    if let Some(off) = chars[start..].iter().position(|c| c.is_whitespace())
        && start + off + 1 < chars.len()
    {
        start += off + 1;
    }
    (chars[start..].iter().collect(), true)
}

fn quoted(body: &str, leading_ellipsis: bool, trailing_ellipsis: bool) -> String {
    let mut text = body.to_string();
    if leading_ellipsis {
        text.insert(0, '…');
    }
    if trailing_ellipsis {
        text.push('…');
    }
    text.lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn confirm_in_place(tui: Option<&crate::composer::SharedTui>, text: &str) -> bool {
    let Some(shared) = tui else {
        return true;
    };
    let mut t = shared.lock().expect("tui lock");
    if t.cold_confirm_armed.as_deref() == Some(text) {
        t.cold_confirm_armed = None;
        let _ = t.draw();
        return true;
    }
    t.cold_confirm_armed = Some(text.to_string());
    t.push_dim(
        "cache is cold and this chat has no earlier turns to summarise · /continue again to send it here"
            .to_string(),
    );
    let _ = t.draw();
    false
}

fn with_extra(text: String, extra: Option<String>) -> String {
    match extra {
        Some(extra) => format!("{text}\n\n{extra}"),
        None => text,
    }
}

fn echo_prompt(tui: Option<&crate::composer::SharedTui>, text: &str) {
    match tui {
        Some(shared) => {
            shared
                .lock()
                .expect("tui lock")
                .push_user_prompt(text, &[], !text.starts_with('/'))
        }
        None => say(None, &format!("{} {text}", crate::tui::arrow_glyph())),
    }
}
