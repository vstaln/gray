use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

type SkillsCache = Option<(String, u64, bool, BTreeSet<String>, Option<String>)>;

#[derive(Default)]
pub struct SkillsPlugin {
    cache: std::sync::Mutex<SkillsCache>,
}

#[async_trait::async_trait]
impl gray_plugin::Plugin for SkillsPlugin {
    fn manifest(&self) -> gray_plugin::Manifest {
        gray_plugin::Manifest {
            name: "skills".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tools: vec![],
            ..Default::default()
        }
    }

    fn tools(&self) -> Vec<std::sync::Arc<dyn gray_core::agent::Tool>> {
        vec![]
    }

    async fn prompt_context(&self, cwd: &str) -> Option<String> {
        let fingerprint = crate::skills::discovery_fingerprint(Path::new(cwd));

        let auto = crate::setup::skills_auto_enabled();
        let disabled = crate::setup::disabled_skill_names();
        if let Ok(guard) = self.cache.lock()
            && let Some((cached_cwd, cached_fp, cached_auto, cached_disabled, cached_block)) =
                guard.as_ref()
            && *cached_cwd == cwd
            && *cached_fp == fingerprint
            && *cached_auto == auto
            && *cached_disabled == disabled
        {
            return cached_block.clone();
        }
        let found = crate::skills::discover_skills(Path::new(cwd));
        let block = crate::skills::format_skills_for_prompt(&found.skills, auto, &disabled);
        let out = if block.trim().is_empty() {
            None
        } else {
            Some(block)
        };
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some((cwd.to_string(), fingerprint, auto, disabled, out.clone()));
        }
        out
    }
}

const PROJECT_RULES_NAMES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

pub(crate) const PROJECT_RULES_MAX_CHARS: usize = 32_768;

pub fn project_context_block(cwd: &Path) -> Option<String> {
    let gray_home = crate::setup::gray_home()
        .ok()
        .and_then(|p| p.canonicalize().ok());

    if !crate::project_trust::is_trusted(&crate::skills::gray_agent_dir(), cwd) {
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

pub fn resolve_skill_name(cwd: &Path, name: &str) -> Option<PathBuf> {
    crate::skills::discover_skills(cwd)
        .skills
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.file_path)
}

pub fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    let after = &trimmed[3..];
    let mut off = 0usize;
    for line in after.lines() {
        if line.trim() == "---" {
            return after[off + line.len()..].trim_start_matches('\n');
        }
        off += line.len() + 1;
    }
    content
}

pub fn apply_substitutions(body: &str, args: Option<&str>, skill_dir: &str) -> String {
    body.replace("$ARGUMENTS", args.unwrap_or(""))
        .replace("${SKILL_DIR}", skill_dir)
}
