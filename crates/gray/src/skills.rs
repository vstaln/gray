//! Skills discovery — port of `packages/coding-agent/src/core/skills.ts` (510 LOC).
//!
//! Discovery rules (literal port):
//! - if a directory contains `SKILL.md`, treat it as a skill root and do not recurse further
//! - otherwise, load direct `.md` children in the root
//! - recurse into subdirectories to find `SKILL.md`
//! - respect `.gitignore` / `.ignore` / `.fdignore` via prefix-aware `ignore`-crate semantics
//! - global: `~/.pi/agent/skills` or `~/.gray/skills` (grok/codex style fallback)
//! - project: `cwd/.pi/skills` or `cwd/.gray/skills`, walking up to git root

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
        // fallback to pi agent dir for compat
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
struct IgnoreMatcher {
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
        matches!(gi.matched(Path::new(path_str), is_dir), ignore::Match::Ignore(_))
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
                .split(|c| c == '\n' || c == '\r')
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
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    disable_model_invocation: bool,
}

fn parse_frontmatter(content: &str) -> Result<(SkillFrontmatter, String), String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        // no frontmatter → empty, body is whole file (pi behavior: parse will return empty frontmatter)
        return Ok((SkillFrontmatter::default(), content.to_string()));
    }
    // find closing ---
    let after_open = &trimmed[3..];
    // skip first newline after opening
    let after_open = after_open.strip_prefix("\r\n").or_else(|| after_open.strip_prefix('\n')).unwrap_or(after_open);
    if let Some(end) = find_closing_delim(after_open) {
        let fm_str = &after_open[..end];
        let body = &after_open[end..];
        let body = body.strip_prefix("---").unwrap_or(body);
        let body = body.strip_prefix("\r\n").or_else(|| body.strip_prefix('\n')).unwrap_or(body).to_string();
        let fm = parse_yaml_like(fm_str);
        Ok((fm, body))
    } else {
        Err("unclosed frontmatter".to_string())
    }
}

fn find_closing_delim(s: &str) -> Option<usize> {
    for (idx, line) in s.lines().enumerate() {
        if line.trim() == "---" {
            // compute byte offset
            let mut off = 0usize;
            for (i, l) in s.lines().enumerate() {
                if i == idx {
                    return Some(off);
                }
                off += l.len() + 1; // +1 for \n (close enough)
            }
        }
    }
    None
}

fn parse_yaml_like(s: &str) -> SkillFrontmatter {
    let mut fm = SkillFrontmatter::default();
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim().trim_matches('"').trim_matches('\'');
            let mut val = line[colon + 1..].trim().to_string();
            // strip quotes
            if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
                || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
            {
                val = val[1..val.len() - 1].to_string();
            }
            match key {
                "name" => fm.name = Some(val),
                "description" => fm.description = Some(val),
                "disable-model-invocation" | "disable_model_invocation" => {
                    fm.disable_model_invocation = val == "true" || val == "True" || val == "TRUE"
                }
                _ => {}
            }
        }
    }
    fm
}

// ---------------------------------------------------------------------------
// Validation (literal port)
// ---------------------------------------------------------------------------

fn validate_name(name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if name.len() > MAX_NAME_LENGTH {
        errors.push(format!("name exceeds {MAX_NAME_LENGTH} characters ({})", name.len()));
    }
    let valid = name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        errors.push("name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)".to_string());
    }
    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }
    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }
    errors
}

fn validate_description(desc: &Option<String>) -> Vec<String> {
    let mut errors = Vec::new();
    match desc {
        None => errors.push("description is required".to_string()),
        Some(d) if d.trim().is_empty() => errors.push("description is required".to_string()),
        Some(d) if d.len() > MAX_DESCRIPTION_LENGTH => {
            errors.push(format!("description exceeds {MAX_DESCRIPTION_LENGTH} characters ({})", d.len()))
        }
        _ => {}
    }
    errors
}

// ---------------------------------------------------------------------------
// Core loaders
// ---------------------------------------------------------------------------

fn is_skill_md_file(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md")
}

fn load_skill_from_file(file_path: &Path, source: &str) -> (Option<Skill>, Vec<Diagnostic>) {
    let mut diagnostics = Vec::new();
    let is_declared_skill = is_skill_md_file(file_path);

    let raw = match fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(e) => {
            diagnostics.push(Diagnostic {
                kind: "warning".to_string(),
                message: e.to_string(),
                path: file_path.to_path_buf(),
                collision: None,
            });
            return (None, diagnostics);
        }
    };

    let frontmatter = match parse_frontmatter(&raw) {
        Ok((fm, _)) => fm,
        Err(e) => {
            if is_declared_skill {
                diagnostics.push(Diagnostic {
                    kind: "warning".to_string(),
                    message: e,
                    path: file_path.to_path_buf(),
                    collision: None,
                });
            }
            return (None, diagnostics);
        }
    };

    let has_description = frontmatter
        .description
        .as_ref()
        .map(|d| !d.trim().is_empty())
        .unwrap_or(false);

    if !is_declared_skill && !has_description {
        return (None, diagnostics);
    }

    let skill_dir = file_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let parent_dir_name = skill_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    for err in validate_description(&frontmatter.description) {
        diagnostics.push(Diagnostic {
            kind: "warning".to_string(),
            message: err,
            path: file_path.to_path_buf(),
            collision: None,
        });
    }

    let frontmatter_name = frontmatter.name.clone();
    let name = frontmatter_name.unwrap_or(parent_dir_name);

    for err in validate_name(&name) {
        diagnostics.push(Diagnostic {
            kind: "warning".to_string(),
            message: err,
            path: file_path.to_path_buf(),
            collision: None,
        });
    }

    if !has_description {
        return (None, diagnostics);
    }

    let description = frontmatter.description.unwrap_or_default();

    let skill = Skill {
        name,
        description,
        file_path: file_path.to_path_buf(),
        base_dir: skill_dir,
        disable_model_invocation: frontmatter.disable_model_invocation,
        source: source.to_string(),
    };
    (Some(skill), diagnostics)
}

// internal walker — literal port of loadSkillsFromDirInternal
fn load_skills_from_dir_internal(
    dir: &Path,
    source: &str,
    include_root_files: bool,
    matcher: &mut IgnoreMatcher,
    root_dir: &Path,
) -> LoadSkillsResult {
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();

    if !dir.exists() {
        return LoadSkillsResult { skills, diagnostics };
    }

    add_ignore_rules(matcher, dir, root_dir);

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return LoadSkillsResult { skills, diagnostics },
    };

    // Collect entries to allow two-phase scan (SKILL.md first, then others)
    let mut entry_list: Vec<(PathBuf, fs::FileType, bool)> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        let ft = match e.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let is_symlink = ft.is_symlink();
        entry_list.push((p, ft, is_symlink));
    }

    // Phase 1: if any SKILL.md exists, treat dir as skill root
    for (full_path, ft, is_symlink) in &entry_list {
        let name = full_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name != "SKILL.md" {
            continue;
        }
        let is_file = if *is_symlink {
            fs::metadata(full_path).map(|m| m.is_file()).unwrap_or(false)
        } else {
            ft.is_file()
        };
        let rel = pathdiff_relative(full_path, root_dir);
        let rel_posix = to_posix_path(&rel);
        if !is_file || matcher.ignores(&rel_posix) {
            continue;
        }
        let (skill, mut diags) = load_skill_from_file(full_path, source);
        if let Some(s) = skill {
            skills.push(s);
        }
        diagnostics.append(&mut diags);
        return LoadSkillsResult { skills, diagnostics };
    }

    // Phase 2: scan children
    for (full_path, ft, is_symlink) in entry_list {
        let file_name = full_path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if file_name.starts_with('.') {
            continue;
        }
        if file_name == "node_modules" {
            continue;
        }

        let (is_dir, is_file) = if is_symlink {
            match fs::metadata(&full_path) {
                Ok(m) => (m.is_dir(), m.is_file()),
                Err(_) => continue,
            }
        } else {
            (ft.is_dir(), ft.is_file())
        };

        let rel = pathdiff_relative(&full_path, root_dir);
        let rel_posix = to_posix_path(&rel);
        let ignore_path = if is_dir {
            format!("{rel_posix}/")
        } else {
            rel_posix.clone()
        };
        if matcher.ignores(&ignore_path) {
            continue;
        }

        if is_dir {
            let mut sub = load_skills_from_dir_internal(&full_path, source, false, matcher, root_dir);
            skills.append(&mut sub.skills);
            diagnostics.append(&mut sub.diagnostics);
            continue;
        }

        if !is_file || !include_root_files || !file_name.ends_with(".md") {
            continue;
        }

        let (skill, mut diags) = load_skill_from_file(&full_path, source);
        if let Some(s) = skill {
            skills.push(s);
        }
        diagnostics.append(&mut diags);
    }

    LoadSkillsResult { skills, diagnostics }
}

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
    let visible: Vec<&Skill> = skills.iter().filter(|s| !s.disable_model_invocation).collect();
    if visible.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    lines.push("\n\nThe following skills provide specialized instructions for specific tasks.".to_string());
    lines.push("Use the skill tool to load a skill's instructions when the task matches its description.".to_string());
    lines.push("When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string());
    lines.push(String::new());
    lines.push("<available_skills>".to_string());
    for skill in visible {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!("    <description>{}</description>", escape_xml(&skill.description)));
        lines.push(format!("    <location>{}</location>", escape_xml(&to_posix_str(&skill.file_path.to_string_lossy()))));
        lines.push("  </skill>".to_string());
    }
    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

pub fn format_skill_invocation(skill: &Skill) -> String {
    format!("Skill `{}`: {}", skill.name, skill.description)
}

// ---------------------------------------------------------------------------
// Top-level loader — port of loadSkills()
// Handles global + project defaults and explicit paths, with collision diagnostics.
// ---------------------------------------------------------------------------

pub fn load_skills(options: LoadSkillsOptions) -> LoadSkillsResult {
    let resolved_cwd = if options.cwd.as_os_str().is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        options.cwd.clone()
    };
    let resolved_agent_dir = options
        .agent_dir
        .clone()
        .unwrap_or_else(gray_agent_dir);

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
        do_add(load_skills_from_dir(&global_skills, "user"), &mut skill_map, &mut real_path_set, &mut all_diagnostics, &mut collision_diagnostics);
        // also check ~/.pi/agent/skills for compat
        if let Some(home) = resolve_home() {
            let pi_skills = home.join(".pi").join("agent").join("skills");
            if pi_skills != global_skills {
                do_add(load_skills_from_dir(&pi_skills, "user"), &mut skill_map, &mut real_path_set, &mut all_diagnostics, &mut collision_diagnostics);
            }
        }
        // project: walk up to git root collecting .pi/skills and .gray/skills
        let git_root = find_git_root(&resolved_cwd);
        let mut project_roots: Vec<PathBuf> = Vec::new();
        let mut cur = Some(resolved_cwd.clone());
        while let Some(dir) = cur {
            project_roots.push(dir.clone());
            if let Some(root) = &git_root {
                if &dir == root {
                    break;
                }
            }
            cur = dir.parent().map(|p| p.to_path_buf());
            if cur.is_none() {
                break;
            }
            // stop at git root if present
            if let Some(root) = &git_root {
                if cur.as_ref() == Some(root) {
                    project_roots.push(root.clone());
                    break;
                }
            }
        }
        // walk from git root down to cwd so closer dirs win? In pi, project is single dir; we load both .pi and .gray
        for ancestor in project_roots.iter().rev() {
            for cfg in [".pi", ".gray"] {
                let d = ancestor.join(cfg).join("skills");
                do_add(load_skills_from_dir(&d, "project"), &mut skill_map, &mut real_path_set, &mut all_diagnostics, &mut collision_diagnostics);
            }
        }
    }

    // explicit paths handling
    let user_skills_dir = resolved_agent_dir.join("skills");
    let pi_user_skills = resolve_home().map(|h| h.join(".pi").join("agent").join("skills"));
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
            if is_under(&resolved, &user_skills_dir) {
                "user"
            } else if pi_user_skills.as_ref().map(|p| is_under(&resolved, p)).unwrap_or(false) {
                "user"
            } else {
                "path"
            }
        } else {
            "path"
        };
        if meta.is_dir() {
            do_add(load_skills_from_dir(&resolved, source), &mut skill_map, &mut real_path_set, &mut all_diagnostics, &mut collision_diagnostics);
        } else if meta.is_file() && resolved.extension().and_then(|e| e.to_str()) == Some("md") {
            let (skill, diags) = load_skill_from_file(&resolved, source);
            if let Some(s) = skill {
                do_add(LoadSkillsResult { skills: vec![s], diagnostics: diags }, &mut skill_map, &mut real_path_set, &mut all_diagnostics, &mut collision_diagnostics);
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
    LoadSkillsResult { skills, diagnostics: all_diagnostics }
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

