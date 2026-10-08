//! System prompt construction. The editable file stays comment-stripped and
//! byte-stable; the runtime prompt appends the directory already known to Gray.
//! Skills are added separately through the per-turn prompt/context hook.

use std::path::Path;

/// Strip `<!-- ... -->` spans (multi-line allowed) and trailing whitespace.
/// Comments stay in the editable file; the model never sees them. An unclosed
/// comment swallows the rest of the file.
pub fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start + 4..].find("-->") {
            Some(end) => rest = &rest[start + 4 + end + 3..],
            None => return out.trim_end().to_string(),
        }
    }
    out.push_str(rest);
    out.trim_end().to_string()
}

/// Build the system prompt: the file text, verbatim, minus HTML comments.
/// `None`/empty → "".
pub fn build_system_prompt(custom_prompt: Option<String>) -> String {
    // A leading comment (like the default's) must not leave a blank first line.
    strip_comments(custom_prompt.as_deref().unwrap_or_default())
        .trim_start()
        .to_string()
}

/// Append runtime context without writing machine-specific paths into AGENTS.md:
/// the caller's cwd, then the static tool-batching facts the model cannot infer.
/// Use the caller's cwd (also passed to tools), not this process's ambient cwd:
/// resumed sessions and headless callers must describe their execution context.
/// JSON quoting keeps newlines, quotes and Windows backslashes unambiguous.
pub fn build_runtime_prompt(custom_prompt: Option<String>, cwd: &Path) -> String {
    let mut prompt = build_system_prompt(custom_prompt);
    if !prompt.is_empty() {
        prompt.push_str("\n\n");
    }
    let directory = serde_json::to_string(&cwd.to_string_lossy())
        .expect("serializing a directory string cannot fail");
    prompt.push_str(&format!(
        "Working directory: {directory}\n\
         This is the starting directory for shell commands and relative file paths. \
         You do not need to run `pwd` just to discover it. \
         Each shell call starts here; `cd` inside a command does not change later calls. \
         Commands run until they exit \u{2014} there is no default timeout, so long builds and \
         test suites are fine; page the output of a long run rather than skipping it."
    ));
    prompt.push_str("\n\n");
    prompt.push_str(TOOL_BATCHING_GUIDANCE);
    prompt
}

/// The model's own identity block, appended after the user's file at build
/// time (Hermes' volatile prompt section carries the same two lines). The
/// model cannot see its picker row or provider from inside the loop, and
/// without them it confabulates a name from provider branding — a stale
/// "SWE-2 High" when the user picked something else. `label` is the
/// picker-visible name (a composite row's "A + B" label, or the catalog's
/// display name); the raw row `id` rides beside it when the label adds
/// something. `provider` is the connection's display name; either line is
/// skipped when empty so a missing value never renders as a lie.
pub fn identity_block(model_label: &str, model_id: &str, provider: &str) -> String {
    let mut lines = Vec::new();
    if !model_id.is_empty() {
        lines.push(if model_label.is_empty() || model_label == model_id {
            format!("Model: {model_id}")
        } else {
            format!("Model: {model_label} ({model_id})")
        });
    }
    if !provider.is_empty() {
        lines.push(format!("Provider: {provider}"));
    }
    lines.join("\n")
}

/// Static harness facts the model cannot infer, and that decide how many rounds a
/// task costs. The concurrency machinery is otherwise invisible: the model issues
/// one call per round, each round re-bills the whole conversation, and the
/// parallel lane plus bash's background hand-off never engage. Kept in the binary so
/// it is never comment-stripped and never lands in the user's editable file, and
/// appended after the runtime directory so the user's verbatim text stays ahead
/// of it. Every claim is true whether or not the parallel lane is enabled:
/// one turn's calls are collected and returned together either way.
const TOOL_BATCHING_GUIDANCE: &str = "\
Tool batching: the tool calls you make in one turn are all run, and their results \
arrive together in the next turn. Independent calls — reading several files, running \
the build and the tests, probing two hypotheses — therefore cost one round between \
them instead of one round each, and every round re-reads the whole conversation. \
Issue independent calls together in one turn, and only chain calls across turns when \
the later one actually depends on the earlier one. Same-turn calls may also run \
concurrently, which changes latency, not cost. A bash command still running at its \
timeout is not killed: it keeps running as a background job and its completion notice \
wakes you in a later turn, so keep working or end your turn instead of polling.";

#[path = "system_prompt_tests.rs"]
#[cfg(test)]
mod tests;
