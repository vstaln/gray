use std::path::{Path, PathBuf};

const PROJECT_RULES_NAMES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

pub(crate) const PROJECT_RULES_MAX_CHARS: usize = 32_768;

pub fn project_rules_files(cwd: &Path) -> Vec<PathBuf> {
    let gray_home = crate::setup::gray_home()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    let mut files: Vec<PathBuf> = cwd
        .ancestors()
        .filter(|d| gray_home.is_none() || d.canonicalize().ok() != gray_home)
        .filter_map(|d| {
            PROJECT_RULES_NAMES
                .iter()
                .map(|name| d.join(name))
                .find(|candidate| candidate.is_file())
        })
        .collect();
    if let Some(global) = gray_core::paths::user_home()
        .map(|h| h.join(".claude").join("CLAUDE.md"))
        .filter(|p| p.is_file())
    {
        files.push(global);
    }
    files.reverse();
    files
}

pub fn project_rules_label(cwd: &Path) -> Option<String> {
    let home = gray_core::paths::user_home();
    let names: Vec<String> = project_rules_files(cwd)
        .iter()
        .map(|p| {
            home.as_deref()
                .and_then(|h| gray_core::paths::tilde(p, h))
                .unwrap_or_else(|| p.display().to_string())
        })
        .collect();
    (!names.is_empty()).then(|| names.join(" · "))
}

fn rules_block(path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    let mut body = strip_rationale_comments(body.trim());
    if body.trim().is_empty() {
        return None;
    }
    if let Some((cut, _)) = body.char_indices().nth(PROJECT_RULES_MAX_CHARS) {
        body.truncate(cut);
        body.push_str(&format!(
            "\n…[truncated — read {} for the full file]",
            path.display()
        ));
    }
    Some(format!(
        "<project_context source=\"{}\">\n{body}\n</project_context>",
        path.display()
    ))
}

pub fn project_context_block(cwd: &Path) -> Option<String> {
    let blocks: Vec<String> = project_rules_files(cwd)
        .iter()
        .filter_map(|p| rules_block(p))
        .collect();
    if blocks.is_empty() {
        return None;
    }
    Some(format!(
        "Project rules, outermost directory first; they outrank general defaults and a nearer file outranks a farther one. The files also have `# r1: ...` rationale lines, stripped here; keep them when editing.\n\n{}",
        blocks.join("\n\n")
    ))
}

fn is_rationale_comment(line: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix('#') else {
        return false;
    };
    let Some(rest) = rest.trim_start().strip_prefix('r') else {
        return false;
    };
    let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    digits > 0 && rest[digits..].trim_start().starts_with(':')
}

pub(crate) fn strip_rationale_comments(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut stripped = false;
    for line in body.lines() {
        if is_rationale_comment(line) {
            stripped = true;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    if stripped { out } else { body.to_string() }
}

pub struct ProjectContextPlugin;

#[async_trait::async_trait]
impl gray_plugin::Plugin for ProjectContextPlugin {
    fn manifest(&self) -> gray_plugin::Manifest {
        gray_plugin::Manifest {
            name: "project-context".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: vec![],
            ..Default::default()
        }
    }

    fn tools(&self) -> Vec<std::sync::Arc<dyn gray_core::agent::Tool>> {
        vec![]
    }

    async fn prompt_context(&self, cwd: &str) -> Option<String> {
        project_context_block(Path::new(cwd))
    }

    /// Project rules are the user's own instructions, not a plugin extra:
    /// lean mode keeps them.
    fn is_global(&self) -> bool {
        true
    }
}
