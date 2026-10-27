use std::path::Path;

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

pub fn build_system_prompt(custom_prompt: Option<String>) -> String {
    strip_comments(custom_prompt.as_deref().unwrap_or_default())
        .trim_start()
        .to_string()
}

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

fn git_field(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

pub fn session_card(cwd: &Path) -> String {
    let mut card = format!("Working directory: {}", cwd.display());
    if let Some(root) = git_field(cwd, &["rev-parse", "--show-toplevel"]) {
        card.push_str(&format!("\nRepo: {root}"));
        if let Some(branch) = git_field(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]) {
            card.push_str(&format!("  Branch: {branch}"));
        }
    }
    card
}

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

pub fn plugins_block(
    manifests: &[gray_plugin::Manifest],
    cli: &std::collections::BTreeSet<String>,
) -> String {
    let names: Vec<&str> = manifests
        .iter()
        .map(|m| m.name.as_str())
        .filter(|n| cli.contains(*n))
        .collect();
    if names.is_empty() {
        return String::new();
    }
    format!(
        "Plugin CLIs (run from bash; `gray <name> --help` prints usage): {}",
        names.join(", ")
    )
}

const TOOL_BATCHING_GUIDANCE: &str = "\
Tool batching: calls made in one turn all run, possibly concurrently, and their results \
arrive together, so independent calls (several reads, build and tests) cost one round \
instead of one each; chain across turns only when a call needs an earlier result. A bash \
command still running at its timeout is not killed: it keeps running as a background job \
and its completion wakes you later, so keep working or end your turn instead of polling.";

pub fn harness_facts(cache_ttl: Option<std::time::Duration>) -> String {
    let mut out = TOOL_BATCHING_GUIDANCE.to_string();
    if let Some(ttl) = cache_ttl.filter(|t| *t <= std::time::Duration::from_secs(20 * 60)) {
        let mins = ttl.as_secs().div_ceil(60).max(1);
        out.push_str(&format!(
            " This provider's prompt cache expires about {mins} minute{s} after the last \
             upstream request; a tool call that blocks longer than that re-bills the \
             whole conversation, so prefer backgrounding commands expected to run past it.",
            s = if mins == 1 { "" } else { "s" }
        ));
    }
    out
}
