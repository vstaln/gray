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

/// A short inventory of the session's plugins so the model knows what the
/// user has installed instead of inferring it from the tool list alone.
/// `cli` holds names `gray <name>` forwarding can reach (lock-registered
/// plugins — dispatch lazily adopts entries that were never probed).
pub fn plugins_block(
    manifests: &[gray_plugin::Manifest],
    cli: &std::collections::BTreeSet<String>,
) -> String {
    if manifests.is_empty() {
        return String::new();
    }
    const MAX_TOOLS: usize = 8;
    let mut lines = Vec::with_capacity(manifests.len());
    for m in manifests {
        let mut parts: Vec<String> = Vec::new();
        let commands: Vec<&str> = m
            .commands
            .iter()
            .chain(m.subcommands.iter())
            .map(String::as_str)
            .collect();
        if !commands.is_empty() {
            parts.push(format!("commands {}", commands.join(" ")));
        }
        if !m.tools.is_empty() {
            let shown: Vec<&str> = m
                .tools
                .iter()
                .take(MAX_TOOLS)
                .map(|t| t.name.as_str())
                .collect();
            let extra = m.tools.len() - shown.len();
            parts.push(if extra == 0 {
                format!("tools {}", shown.join(", "))
            } else {
                format!("tools {}, +{extra} more", shown.join(", "))
            });
        }
        if cli.contains(&m.name) {
            let verbs = if m.completion.is_empty() {
                String::new()
            } else {
                format!(" — verbs: {}", m.completion.join(" "))
            };
            parts.push(format!("cli `gray {}`{verbs}", m.name));
        }
        let version = if m.version.is_empty() {
            String::new()
        } else {
            format!(" {}", m.version)
        };
        if parts.is_empty() {
            lines.push(format!("- {}{version}", m.name));
        } else {
            lines.push(format!("- {}{version} — {}", m.name, parts.join("; ")));
        }
    }
    format!(
        "Plugins live in this session:\n{}\nPlugin slash commands are typed by the user, not you — \
         drive a plugin through its listed tools, or from bash via its `gray` cli where marked \
         (`gray <name> --help` prints its usage).",
        lines.join("\n")
    )
}

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

#[path = "system_prompt_tests.rs"]
#[cfg(test)]
mod tests;
