//! Skills discovery.
//!
//! Discovery rules:
//! - if a directory contains `SKILL.md`, treat it as a skill root and do not recurse further
//! - otherwise, load direct `.md` children in the root
//! - recurse into subdirectories to find `SKILL.md`
//! - respect `.gitignore` / `.ignore` / `.fdignore` via prefix-aware `ignore`-crate semantics
//! - global: `~/.gray/skills`, `~/.config/opencode/skills`, `~/.config/opencode/*/skills`, `~/.agents/skills`, `~/.claude/skills`, `~/.pi/agent/skills`
//! - project: `.gray/skills`, `.opencode/skills`, `.agents/skills`, `.claude/skills`, `.pi/skills`, walking up to git root

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;
const IGNORE_FILE_NAMES: &[&str] = &[".gitignore", ".ignore", ".fdignore"];

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    pub base_dir: PathBuf,
    pub disable_model_invocation: bool,
    /// synthetic source label: "user" | "project" | "path"
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub kind: String, // "warning" | "collision"
    pub message: String,
    pub path: PathBuf,
    pub collision: Option<CollisionInfo>,
}

#[derive(Debug, Clone)]
pub struct CollisionInfo {
    pub resource_type: String,
    pub name: String,
    pub winner_path: PathBuf,
    pub loser_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct LoadSkillsOptions {
    pub cwd: PathBuf,
    pub agent_dir: Option<PathBuf>,
    pub skill_paths: Vec<PathBuf>,
    pub include_defaults: bool,
}

// ---------------------------------------------------------------------------
// Helpers: paths
// ---------------------------------------------------------------------------

fn to_posix_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn to_posix_str(s: &str) -> String {
    s.replace('\\', "/")
}

fn canonicalize_path(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn resolve_home() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME") {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = std::env::var("GRAY_HOME") {
        return Some(PathBuf::from(h));
    }
    None
}

fn gray_agent_dir() -> PathBuf {
    if let Ok(gray_home) = std::env::var("GRAY_HOME") {
        return PathBuf::from(gray_home);
    }
    if let Some(home) = resolve_home() {
        let gray = home.join(".gray");
        if gray.exists() {
            return gray;
        }
        let pi = home.join(".pi").join("agent");
        if pi.exists() {
            return pi;
        }
        return gray;
    }
    PathBuf::from(".gray")
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut cur = if start.is_file() {
        start.parent().map(PathBuf::from)
    } else {
        Some(start.to_path_buf())
    };
    while let Some(dir) = cur {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        cur = dir.parent().map(|p| p.to_path_buf());
    }
    None
}

// ---------------------------------------------------------------------------
// Ignore handling — delegates to `ignore` crate's GitignoreBuilder.
// Patterns are prefixed to `root_dir` in add_ignore_rules so a single
// builder rooted at root_dir suffices. Negation (!), dir-suffix (/),
// and globs (*, **, ?) are handled by the crate.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct IgnoreMatcher {
    builder: GitignoreBuilder,
    built: Option<Gitignore>,
}

impl Default for IgnoreMatcher {
    fn default() -> Self {
        Self {
            builder: GitignoreBuilder::new(""),
            built: None,
        }
    }
}

impl IgnoreMatcher {
    fn new(root: &Path) -> Self {
        Self {
            builder: GitignoreBuilder::new(root),
            built: None,
        }
    }

    fn add(&mut self, patterns: Vec<String>) {
        for pat in patterns {
            let _ = self.builder.add_line(None, &pat);
        }
        self.built = None;
    }

    fn ignores(&mut self, rel_posix: &str) -> bool {
        if rel_posix.is_empty() {
            return false;
        }
        let is_dir = rel_posix.ends_with('/');
        let path_str = if is_dir {
            &rel_posix[..rel_posix.len() - 1]
        } else {
            rel_posix
        };
        if self.built.is_none() {
            self.built = Some(self.builder.build().unwrap_or_else(|_| Gitignore::empty()));
        }
        let gi = self.built.as_ref().unwrap();
        if gi.is_empty() {
            return false;
        }
        matches!(
            gi.matched(Path::new(path_str), is_dir),
            ignore::Match::Ignore(_)
        )
    }
}

fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
        return None;
    }
    let mut pattern = line.to_string();
    let mut negated = false;
    if pattern.starts_with('!') {
        negated = true;
        pattern = pattern[1..].to_string();
    } else if pattern.starts_with("\\!") {
        pattern = pattern[1..].to_string();
    }
    if pattern.starts_with('/') {
        pattern = pattern[1..].to_string();
    }
    let prefixed = if prefix.is_empty() {
        pattern
    } else {
        format!("{prefix}{pattern}")
    };
    if negated {
        Some(format!("!{prefixed}"))
    } else {
        Some(prefixed)
    }
}

fn add_ignore_rules(matcher: &mut IgnoreMatcher, dir: &Path, root_dir: &Path) {
    let relative_dir = pathdiff_relative(dir, root_dir);
    let prefix = if relative_dir.as_os_str().is_empty() {
        String::new()
    } else {
        format!("{}/", to_posix_path(&relative_dir))
    };
    for filename in IGNORE_FILE_NAMES {
        let ignore_path = dir.join(filename);
        if !ignore_path.exists() {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&ignore_path) {
            let patterns: Vec<String> = content
                .split(['\n', '\r'])
                .filter_map(|line| prefix_ignore_pattern(line, &prefix))
                .collect();
            if !patterns.is_empty() {
                matcher.add(patterns);
            }
        }
    }
}

fn pathdiff_relative(path: &Path, base: &Path) -> PathBuf {
    // minimal relative; fallback to file name if not under base
    if let Ok(rel) = path.strip_prefix(base) {
        return rel.to_path_buf();
    }
    path.to_path_buf()
}

// ---------------------------------------------------------------------------
// Frontmatter
mod load;

pub(crate) use load::{load_skill_from_file, load_skills_from_dir_internal};

pub fn load_skills_from_dir(dir: &Path, source: &str) -> LoadSkillsResult {
    let root = dir.to_path_buf();
    let mut matcher = IgnoreMatcher::new(&root);
    load_skills_from_dir_internal(dir, source, true, &mut matcher, &root)
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut lines = vec![
        "\n\nThe following skills provide specialized instructions for specific tasks.".to_string(),
        "Use the skill tool to load a skill's instructions when the task matches its description."
            .to_string(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];
    for skill in visible {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&to_posix_str(&skill.file_path.to_string_lossy()))
        ));
        lines.push("  </skill>".to_string());
    }
    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

pub fn format_skill_invocation(skill: &Skill) -> String {
    format!("Skill `{}`: {}", skill.name, skill.description)
}

// ---------------------------------------------------------------------------
// Top-level loader
// Handles global + project defaults and explicit paths, with collision diagnostics.
// ---------------------------------------------------------------------------

pub fn load_skills(options: LoadSkillsOptions) -> LoadSkillsResult {
    let resolved_cwd = if options.cwd.as_os_str().is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        options.cwd.clone()
    };
    let resolved_agent_dir = options.agent_dir.clone().unwrap_or_else(gray_agent_dir);

    let mut skill_map: HashMap<String, Skill> = HashMap::new();
    let mut real_path_set: HashSet<PathBuf> = HashSet::new();
    let mut all_diagnostics: Vec<Diagnostic> = Vec::new();
    let mut collision_diagnostics: Vec<Diagnostic> = Vec::new();

    // Inline helper to avoid capturing `all_diagnostics` in a closure (borrowck).
    let do_add = |result: LoadSkillsResult,
                  skill_map: &mut HashMap<String, Skill>,
                  real_path_set: &mut HashSet<PathBuf>,
                  all_diagnostics: &mut Vec<Diagnostic>,
                  collision_diagnostics: &mut Vec<Diagnostic>| {
        all_diagnostics.extend(result.diagnostics);
        for skill in result.skills {
            let real = canonicalize_path(&skill.file_path);
            if real_path_set.contains(&real) {
                continue;
            }
            if let Some(existing) = skill_map.get(&skill.name) {
                collision_diagnostics.push(Diagnostic {
                    kind: "collision".to_string(),
                    message: format!("name \"{}\" collision", skill.name),
                    path: skill.file_path.clone(),
                    collision: Some(CollisionInfo {
                        resource_type: "skill".to_string(),
                        name: skill.name.clone(),
                        winner_path: existing.file_path.clone(),
                        loser_path: skill.file_path.clone(),
                    }),
                });
            } else {
                real_path_set.insert(real);
                skill_map.insert(skill.name.clone(), skill);
            }
        }
    };

    if options.include_defaults {
        // global
        let global_skills = resolved_agent_dir.join("skills");
        do_add(
            load_skills_from_dir(&global_skills, "user"),
            &mut skill_map,
            &mut real_path_set,
            &mut all_diagnostics,
            &mut collision_diagnostics,
        );
        if let Some(home) = resolve_home() {
            // OpenCode global skills & plugins (e.g. superpowers)
            let config_base = std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".config"));
            let opencode_dir = config_base.join("opencode");
            let opencode_skills = opencode_dir.join("skills");
            if opencode_skills.is_dir() && opencode_skills != global_skills {
                do_add(
                    load_skills_from_dir(&opencode_skills, "user"),
                    &mut skill_map,
                    &mut real_path_set,
                    &mut all_diagnostics,
                    &mut collision_diagnostics,
                );
            }
            if let Ok(entries) = fs::read_dir(&opencode_dir) {
                for entry in entries.flatten() {
                    let sub_skills = entry.path().join("skills");
                    if sub_skills.is_dir()
                        && sub_skills != opencode_skills
                        && sub_skills != global_skills
                    {
                        do_add(
                            load_skills_from_dir(&sub_skills, "user"),
                            &mut skill_map,
                            &mut real_path_set,
                            &mut all_diagnostics,
                            &mut collision_diagnostics,
                        );
                    }
                }
            }

            // Agents and Claude global skills
            let agents_skills = home.join(".agents").join("skills");
            if agents_skills.is_dir() && agents_skills != global_skills {
                do_add(
                    load_skills_from_dir(&agents_skills, "user"),
                    &mut skill_map,
                    &mut real_path_set,
                    &mut all_diagnostics,
                    &mut collision_diagnostics,
                );
            }
            let claude_skills = home.join(".claude").join("skills");
            if claude_skills.is_dir() && claude_skills != global_skills {
                do_add(
                    load_skills_from_dir(&claude_skills, "user"),
                    &mut skill_map,
                    &mut real_path_set,
                    &mut all_diagnostics,
                    &mut collision_diagnostics,
                );
            }

            let pi_skills = home.join(".pi").join("agent").join("skills");
            if pi_skills.is_dir() && pi_skills != global_skills {
                do_add(
                    load_skills_from_dir(&pi_skills, "user"),
                    &mut skill_map,
                    &mut real_path_set,
                    &mut all_diagnostics,
                    &mut collision_diagnostics,
                );
            }
        }
        // project: walk up to git root collecting skills
        let git_root = find_git_root(&resolved_cwd);
        let mut project_roots: Vec<PathBuf> = Vec::new();
        let mut cur = Some(resolved_cwd.clone());
        while let Some(dir) = cur {
            project_roots.push(dir.clone());
            if let Some(root) = &git_root
                && &dir == root
            {
                break;
            }
            cur = dir.parent().map(|p| p.to_path_buf());
            if cur.is_none() {
                break;
            }
            if let Some(root) = &git_root
                && cur.as_ref() == Some(root)
            {
                project_roots.push(root.clone());
                break;
            }
        }
        for ancestor in project_roots.iter().rev() {
            for cfg in [".gray", ".opencode", ".agents", ".claude", ".pi"] {
                let d = ancestor.join(cfg).join("skills");
                if d.is_dir() {
                    do_add(
                        load_skills_from_dir(&d, "project"),
                        &mut skill_map,
                        &mut real_path_set,
                        &mut all_diagnostics,
                        &mut collision_diagnostics,
                    );
                }
            }
        }
    }

    // explicit paths handling
    let mut user_skill_roots = vec![resolved_agent_dir.join("skills")];
    if let Some(home) = resolve_home() {
        let config_base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config"));
        let opencode_dir = config_base.join("opencode");
        user_skill_roots.push(opencode_dir.join("skills"));
        if let Ok(entries) = fs::read_dir(&opencode_dir) {
            for entry in entries.flatten() {
                let sub = entry.path().join("skills");
                if sub.is_dir() {
                    user_skill_roots.push(sub);
                }
            }
        }
        user_skill_roots.push(home.join(".agents").join("skills"));
        user_skill_roots.push(home.join(".claude").join("skills"));
        user_skill_roots.push(home.join(".pi").join("agent").join("skills"));
    }
    // For is_under_path checks
    let is_under = |target: &Path, root: &Path| -> bool {
        if target == root {
            return true;
        }
        target.strip_prefix(root).is_ok()
    };

    for raw in &options.skill_paths {
        let resolved = if raw.is_absolute() {
            raw.clone()
        } else {
            resolved_cwd.join(raw)
        };
        if !resolved.exists() {
            all_diagnostics.push(Diagnostic {
                kind: "warning".to_string(),
                message: "skill path does not exist".to_string(),
                path: resolved.clone(),
                collision: None,
            });
            continue;
        }
        let meta = match fs::metadata(&resolved) {
            Ok(m) => m,
            Err(e) => {
                all_diagnostics.push(Diagnostic {
                    kind: "warning".to_string(),
                    message: e.to_string(),
                    path: resolved.clone(),
                    collision: None,
                });
                continue;
            }
        };
        let source = if !options.include_defaults {
            if user_skill_roots
                .iter()
                .any(|root| is_under(&resolved, root))
            {
                "user"
            } else {
                "path"
            }
        } else {
            "path"
        };
        if meta.is_dir() {
            do_add(
                load_skills_from_dir(&resolved, source),
                &mut skill_map,
                &mut real_path_set,
                &mut all_diagnostics,
                &mut collision_diagnostics,
            );
        } else if meta.is_file() && resolved.extension().and_then(|e| e.to_str()) == Some("md") {
            let (skill, diags) = load_skill_from_file(&resolved, source);
            if let Some(s) = skill {
                do_add(
                    LoadSkillsResult {
                        skills: vec![s],
                        diagnostics: diags,
                    },
                    &mut skill_map,
                    &mut real_path_set,
                    &mut all_diagnostics,
                    &mut collision_diagnostics,
                );
            } else {
                all_diagnostics.extend(diags);
            }
        } else {
            all_diagnostics.push(Diagnostic {
                kind: "warning".to_string(),
                message: "skill path is not a markdown file".to_string(),
                path: resolved.clone(),
                collision: None,
            });
        }
    }

    let mut skills: Vec<Skill> = skill_map.into_values().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    all_diagnostics.extend(collision_diagnostics);
    LoadSkillsResult {
        skills,
        diagnostics: all_diagnostics,
    }
}

/// Convenience: discover skills for `cwd` with defaults (global + project).
pub fn discover_skills(cwd: &Path) -> LoadSkillsResult {
    load_skills(LoadSkillsOptions {
        cwd: cwd.to_path_buf(),
        agent_dir: None,
        skill_paths: vec![],
        include_defaults: true,
    })
}
