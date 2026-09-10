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
    /// declared invocation args from frontmatter `args:`/`arguments:`;
    /// empty = the skill takes no arguments.
    pub args: Vec<String>,
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

pub(crate) fn find_git_root(start: &Path) -> Option<PathBuf> {
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

fn pathdiff_relative(path: &Path, base: &Path) -> PathBuf {
    // minimal relative; fallback to full path if not under base
    if let Ok(rel) = path.strip_prefix(base) {
        return rel.to_path_buf();
    }
    path.to_path_buf()
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
        "Only load a skill for multi-step or specialized work that genuinely requires its workflow — trivial single-step edits and direct answers never require a skill.".to_string(),
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

// ---------------------------------------------------------------------------
// Invocation-arg validation (Bug1: `/skills:<name> <bogus-args>` silently
// ignored args while `/skills:bogus-name` errored). Skills declare args via
// frontmatter `args:`/`arguments:` (see `load::parse_declared_args`); empty
// means the skill takes no arguments, so any passed arg is an error naming
// the valid args. Callers (REPL `/skills:` expansion, `SkillTool`) must
// surface the Err string locally instead of invoking the model.
// ---------------------------------------------------------------------------

/// Validate free-form invocation args against a skill's declared `args`.
/// `args` is the raw text after the skill name (`None`/blank = no args).
/// Returns `Ok` when no args were passed or every token matches; otherwise
/// `Err` naming the valid args for local display.
pub fn validate_skill_args(skill: &Skill, args: Option<&str>) -> Result<(), String> {
    let raw = args.unwrap_or("").trim();
    if raw.is_empty() {
        return Ok(());
    }
    if skill.args.is_empty() {
        return Err(format!(
            "skill '{}' takes no arguments (valid args: (none)) — got '{}'",
            skill.name, raw
        ));
    }
    let mut unknown: Vec<String> = Vec::new();
    for tok in raw.split_whitespace() {
        for part in tok.split(',') {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            let key = p
                .trim_start_matches('-')
                .split('=')
                .next()
                .unwrap_or("")
                .trim()
                .trim_start_matches('-')
                .trim()
                .to_string();
            if key.is_empty() {
                continue;
            }
            if !skill.args.iter().any(|a| a == &key) {
                unknown.push(key);
            }
        }
    }
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "unknown argument '{}' for skill '{}' (valid args: {})",
            unknown.join(", "),
            skill.name,
            skill.args.join(", ")
        ))
    }
}

// ---------------------------------------------------------------------------
// Top-level loader
// Handles global + project defaults with collision diagnostics.
// ---------------------------------------------------------------------------

fn load_skills(cwd: &Path, agent_dir: &Path) -> LoadSkillsResult {
    let resolved_cwd = if cwd.as_os_str().is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        cwd.to_path_buf()
    };
    let resolved_agent_dir = agent_dir.to_path_buf();

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

    // global
    let global_skills = resolved_agent_dir.join("skills");
    do_add(
        load_skills_from_dir(&global_skills, "user"),
        &mut skill_map,
        &mut real_path_set,
        &mut all_diagnostics,
        &mut collision_diagnostics,
    );
    // P2-2: pi installs land in `<agent_dir>/plugins/pi/<pkg>/`.
    let pi_plugins = resolved_agent_dir.join("plugins").join("pi");
    if pi_plugins.is_dir() && pi_plugins != global_skills {
        do_add(
            load_skills_from_dir(&pi_plugins, "user"),
            &mut skill_map,
            &mut real_path_set,
            &mut all_diagnostics,
            &mut collision_diagnostics,
        );
    }
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
    let mut skills: Vec<Skill> = skill_map.into_values().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    all_diagnostics.extend(collision_diagnostics);
    LoadSkillsResult {
        skills,
        diagnostics: all_diagnostics,
    }
}

/// Discover skills for `cwd` with defaults (global + project).
pub fn discover_skills(cwd: &Path) -> LoadSkillsResult {
    load_skills(cwd, &gray_agent_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_skill(name: &str, args: &[&str]) -> Skill {
        Skill {
            name: name.to_string(),
            description: "test".to_string(),
            file_path: PathBuf::from("/tmp/SKILL.md"),
            base_dir: PathBuf::from("/tmp"),
            disable_model_invocation: false,
            source: "path".to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn no_args_skill_rejects_any_arg() {
        let s = test_skill("deploy", &[]);
        assert!(validate_skill_args(&s, None).is_ok());
        assert!(validate_skill_args(&s, Some("")).is_ok());
        assert!(validate_skill_args(&s, Some("   ")).is_ok());
        let err = validate_skill_args(&s, Some("bogus-args")).unwrap_err();
        assert!(err.contains("deploy"), "names skill: {err}");
        assert!(err.contains("(none)"), "names valid args: {err}");
    }

    #[test]
    fn declared_args_reject_unknown_naming_valid() {
        let s = test_skill("deploy", &["env", "force"]);
        assert!(validate_skill_args(&s, Some("env")).is_ok());
        assert!(validate_skill_args(&s, Some("env force")).is_ok());
        assert!(validate_skill_args(&s, Some("--env --force")).is_ok());
        let err = validate_skill_args(&s, Some("bogus")).unwrap_err();
        assert!(err.contains("bogus"), "names unknown: {err}");
        assert!(err.contains("env"), "names valid: {err}");
        assert!(err.contains("force"), "names valid: {err}");
    }

    #[test]
    fn prompt_block_routes_trivial_work_away_from_skills() {
        let s = test_skill("anything", &[]);
        let out = format_skills_for_prompt(&[s]);
        assert!(
            out.contains("trivial single-step"),
            "routing hint missing: {out}"
        );
        assert!(out.contains("<available_skills>"));
    }

    #[test]
    fn pi_plugin_dir_is_a_discovery_root() {
        // P2-2: `<agent_dir>/plugins/pi/<pkg>/<skill>/SKILL.md` (where pi
        // installs land) must surface via default discovery.
        let agent = tempfile::tempdir().unwrap();
        let dir = agent
            .path()
            .join("plugins")
            .join("pi")
            .join("demo-pkg")
            .join("pi-probe-zzz-skill");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\ndescription: probe skill\n---\nBody",
        )
        .unwrap();
        let res = load_skills(agent.path(), agent.path());
        assert!(
            res.skills.iter().any(|s| s.name == "pi-probe-zzz-skill"),
            "installed pi skill not discovered: {:?}",
            res.skills.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
    }
}
