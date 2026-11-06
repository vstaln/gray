use std::path::{Path, PathBuf};

const PROJECT_RULES_NAMES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

pub(crate) const PROJECT_RULES_MAX_CHARS: usize = 32_768;

pub fn project_context_block(cwd: &Path) -> Option<String> {
    let gray_home = crate::setup::gray_home()
        .ok()
        .and_then(|p| p.canonicalize().ok());

    if !crate::project_trust::is_trusted(&gray_core::paths::gray_home_or_temp(), cwd) {
        return None;
    }
    let path = find_project_rules(cwd, gray_home.as_deref())?;
    let body = std::fs::read_to_string(&path).ok()?;
    let body = strip_rationale_comments(body.trim());

    if body.trim().is_empty() {
        return None;
    }
    let mut body = body;
    if body.chars().count() > PROJECT_RULES_MAX_CHARS {
        let cut = body
            .char_indices()
            .nth(PROJECT_RULES_MAX_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(body.len());
        body.truncate(cut);
        body.push_str(&format!(
            "\n…[truncated — read {} for the full file]",
            path.display()
        ));
    }

    Some(format!(
        "<project_context source=\"{}\">\nProject rules; they outrank general defaults. The file also has `# r1: ...` rationale lines, stripped here; keep them when editing it.\n\n{body}\n</project_context>",
        path.display()
    ))
}

fn find_project_rules(cwd: &Path, gray_home: Option<&Path>) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(d) = dir {
        let is_gray_home = gray_home
            .map(|h| d.canonicalize().ok().as_deref() == Some(h))
            .unwrap_or(false);
        if !is_gray_home {
            for name in PROJECT_RULES_NAMES {
                let candidate = d.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        dir = d.parent();
    }
    None
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
}
