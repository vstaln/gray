use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

const IGNORE_FILE_NAMES: &[&str] = &[".gitignore", ".ignore", ".fdignore"];

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file_path: PathBuf,
    pub base_dir: PathBuf,
    pub disable_model_invocation: bool,

    pub source: String,

    pub args: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
}

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
    if let Some(h) = gray_core::paths::user_home() {
        return Some(h);
    }
    if let Ok(h) = std::env::var("GRAY_HOME") {
        return Some(PathBuf::from(h));
    }
    None
}

pub(crate) fn gray_agent_dir() -> PathBuf {
    if let Ok(gray_home) = std::env::var("GRAY_HOME") {
        return PathBuf::from(gray_home);
    }
    if let Some(home) = resolve_home() {
        let gray = home.join(".gray");
        if gray.exists() {
            return gray;
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

mod load;

pub(crate) use load::load_skills_from_dir_internal;
pub(crate) use load::parse_frontmatter;

pub fn load_skills_from_dir(dir: &Path, source: &str) -> LoadSkillsResult {
    let root = dir.to_path_buf();
    let mut matcher = IgnoreMatcher::new(&root);
    let mut visited = std::collections::HashSet::new();
    load_skills_from_dir_internal(dir, source, true, &mut matcher, &root, &mut visited)
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn format_discovered_skill_row(skill: &Skill) -> String {
    format_skill_row(skill, 100)
}

pub fn format_discovered_skill_row_full(skill: &Skill) -> String {
    format_skill_row(skill, usize::MAX)
}

fn format_skill_row(skill: &Skill, max_desc: usize) -> String {
    let desc: String = skill
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let short = if desc.chars().count() > max_desc {
        format!("{}…", desc.chars().take(max_desc - 1).collect::<String>())
    } else {
        desc
    };
    if short.is_empty() {
        skill.name.clone()
    } else {
        format!("{} — {}", skill.name, short)
    }
}

pub fn rank_skills_for_prompt<'a>(
    skills: &'a [Skill],
    auto_enabled: bool,
    disabled: &BTreeSet<String>,
) -> Vec<&'a Skill> {
    if !auto_enabled {
        return Vec::new();
    }
    const MAX_DESCRIPTION_LEN: usize = 700;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in skills {
        if s.disable_model_invocation || s.name.trim().is_empty() {
            continue;
        }
        if disabled.contains(&s.name) {
            continue;
        }
        if s.description.trim().is_empty() || s.description.len() > MAX_DESCRIPTION_LEN {
            continue;
        }
        if seen.insert(s.name.clone()) {
            out.push(s);
        }
    }
    out
}

pub fn format_skills_for_prompt(
    skills: &[Skill],
    auto_enabled: bool,
    disabled: &BTreeSet<String>,
) -> String {
    const MAX_PROMPT_SKILLS: usize = 12;
    let visible: Vec<&Skill> = rank_skills_for_prompt(skills, auto_enabled, disabled);
    if visible.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "Skills: when a task matches one, `cat` its SKILL.md before acting; paths in it are relative to its directory.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];
    for skill in visible.iter().take(MAX_PROMPT_SKILLS) {
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
    if visible.len() > MAX_PROMPT_SKILLS {
        lines.push(format!(
            "{} more skills omitted from this prompt; the user can select them with /skills <name>.",
            visible.len() - MAX_PROMPT_SKILLS
        ));
    }
    lines.join("\n")
}

pub fn validate_skill_args(skill: &Skill, args: Option<&str>) -> Result<(), String> {
    let raw = args.unwrap_or("").trim();
    if raw.is_empty() || skill.args.is_empty() {
        return Ok(());
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

fn push_root(
    roots: &mut Vec<(PathBuf, &'static str)>,
    dir: PathBuf,
    source: &'static str,
    global_skills: &Path,
) {
    if dir.is_dir() && dir != global_skills {
        roots.push((dir, source));
    }
}

fn claude_disabled_plugins(home: &Path) -> HashSet<String> {
    let mut disabled = HashSet::new();
    for name in ["settings.json", "settings.local.json"] {
        let Ok(raw) = fs::read_to_string(home.join(".claude").join(name)) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let Some(map) = json.get("enabledPlugins").and_then(|v| v.as_object()) else {
            continue;
        };
        for (key, enabled) in map {
            if enabled.as_bool() == Some(false) {
                disabled.insert(key.clone());
            } else {
                disabled.remove(key);
            }
        }
    }
    disabled
}

fn claude_plugin_skill_dirs(home: &Path) -> Vec<PathBuf> {
    let manifest = home.join(".claude/plugins/installed_plugins.json");
    let Ok(raw) = fs::read_to_string(&manifest) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(plugins) = json.get("plugins").and_then(|v| v.as_object()) else {
        return Vec::new();
    };
    let disabled = claude_disabled_plugins(home);
    let mut dirs = Vec::new();
    for (name, installs) in plugins {
        if disabled.contains(name) {
            continue;
        }
        let latest = installs
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let path = e.get("installPath")?.as_str()?;
                let updated = e
                    .get("lastUpdated")
                    .or_else(|| e.get("installedAt"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Some((updated, path))
            })
            .max_by_key(|(updated, _)| *updated);
        let Some((_, install_path)) = latest else {
            continue;
        };
        let skills = Path::new(install_path).join("skills");
        if skills.is_dir() {
            dirs.push(skills);
        }
    }
    dirs
}

fn skill_search_roots(cwd: &Path, agent_dir: &Path) -> Vec<(PathBuf, &'static str)> {
    skill_search_roots_under(cwd, agent_dir, resolve_home().as_deref())
}

fn skill_search_roots_under(
    cwd: &Path,
    agent_dir: &Path,
    home: Option<&Path>,
) -> Vec<(PathBuf, &'static str)> {
    let resolved_cwd = if cwd.as_os_str().is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        cwd.to_path_buf()
    };
    let resolved_agent_dir = agent_dir.to_path_buf();
    let mut roots: Vec<(PathBuf, &'static str)> = Vec::new();

    let global_skills = resolved_agent_dir.join("skills");
    if global_skills.is_dir() {
        roots.push((global_skills.clone(), "user"));
    }

    if std::env::var("GRAY_SKILLS_ONLY").as_deref() == Ok("1") {
        return roots;
    }
    if let Some(home) = home {
        push_root(
            &mut roots,
            home.join(".claude").join("skills"),
            "user",
            &global_skills,
        );

        let config_base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".config"));
        let opencode_dir = config_base.join("opencode");
        let opencode_skills = opencode_dir.join("skills");
        push_root(&mut roots, opencode_skills.clone(), "user", &global_skills);
        if let Ok(entries) = fs::read_dir(&opencode_dir) {
            for entry in entries.flatten() {
                let sub_skills = entry.path().join("skills");
                if sub_skills.is_dir()
                    && sub_skills != opencode_skills
                    && sub_skills != global_skills
                {
                    roots.push((sub_skills, "user"));
                }
            }
        }

        push_root(
            &mut roots,
            home.join(".agents").join("skills"),
            "user",
            &global_skills,
        );

        for dir in claude_plugin_skill_dirs(home) {
            roots.push((dir, "plugin"));
        }
    }

    if !crate::project_trust::is_trusted(&resolved_agent_dir, &resolved_cwd) {
        return roots;
    }

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
        for cfg in [".gray", ".claude", ".opencode", ".agents"] {
            let d = ancestor.join(cfg).join("skills");
            if d.is_dir() {
                roots.push((d, "project"));
            }
        }
    }
    roots
}

pub fn discovery_fingerprint(cwd: &Path) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::UNIX_EPOCH;

    let mut h = DefaultHasher::new();
    for (root, source) in skill_search_roots(cwd, &gray_agent_dir()) {
        root.hash(&mut h);
        source.hash(&mut h);

        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            let mut entries: Vec<_> = fs::read_dir(&dir)
                .map(|r| r.flatten().collect())
                .unwrap_or_default();
            entries.sort_by_key(|a| a.file_name());
            for e in entries {
                let name = e.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with('.') || name_str == "node_modules" {
                    continue;
                }
                name_str.hash(&mut h);
                match e.file_type() {
                    Ok(ft) if ft.is_dir() => {
                        0u8.hash(&mut h);
                        stack.push(e.path());
                    }
                    _ => {
                        1u8.hash(&mut h);
                        match fs::symlink_metadata(e.path()) {
                            Ok(m) => {
                                m.modified()
                                    .ok()
                                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                                    .map(|d| d.as_nanos())
                                    .unwrap_or(0)
                                    .hash(&mut h);
                                m.len().hash(&mut h);
                            }
                            Err(_) => 0u8.hash(&mut h),
                        }
                    }
                }
            }
        }
    }
    h.finish()
}

pub(crate) fn load_skills(cwd: &Path, agent_dir: &Path) -> LoadSkillsResult {
    let mut skill_map: HashMap<String, Skill> = HashMap::new();
    let mut real_path_set: HashSet<PathBuf> = HashSet::new();

    let do_add = |result: LoadSkillsResult,
                  skill_map: &mut HashMap<String, Skill>,
                  real_path_set: &mut HashSet<PathBuf>| {
        for skill in result.skills {
            let real = canonicalize_path(&skill.file_path);
            if real_path_set.contains(&real) {
                continue;
            }
            if skill_map.get(&skill.name).is_none() {
                real_path_set.insert(real);
                skill_map.insert(skill.name.clone(), skill);
            }
        }
    };

    for (dir, source) in skill_search_roots(cwd, agent_dir) {
        do_add(
            load_skills_from_dir(&dir, source),
            &mut skill_map,
            &mut real_path_set,
        );
    }
    let mut skills: Vec<Skill> = skill_map.into_values().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    LoadSkillsResult { skills }
}

pub fn discover_skills(cwd: &Path) -> LoadSkillsResult {
    load_skills(cwd, &gray_agent_dir())
}
